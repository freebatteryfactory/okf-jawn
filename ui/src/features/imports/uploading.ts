/**
 * Uploading the chosen files: for each one a slot (`create_upload`), its bytes (`upload_content`)
 * and the finish (`complete_upload`) against its length and SHA-256 (SPEC section 5: "finalized
 * against expected length/hash").
 *
 * The uploads run here, not in the import page, so they go on when the page's components are
 * unmounted. `UPLOADS_AT_ONCE` files are under way at a time; a file that fails keeps its place in
 * the list and the others go on.
 *
 * One try of one file:
 * 1. The file is read once for its hash (`hash.ts`), one file at a time, so one file's content is
 *    in memory at once whatever the number under way.
 * 2. `create_upload` announces the name, the folder, the length (a decimal count) and the hash,
 *    under the entry's slot key. It is asked at the start of every try: a repeat under the same key
 *    opens nothing and answers the slot the service already holds, with the bytes it holds
 *    (`received_bytes`).
 * 3. `upload_content` sends the bytes the slot lacks: the whole file into a fresh slot, the rest
 *    of it into one that holds a part (the service appends). It is skipped when the slot holds
 *    them all. The answer must say that the slot now holds exactly the file's length.
 * 4. `complete_upload` carries the hash, under the entry's finish key. The file counts as uploaded
 *    only when the answer says `complete` and holds exactly the file's length. Anything else is a
 *    failure, never "uploaded".
 *
 * A retry is a new try of the same file on the same slot under the same keys, so nothing is opened
 * or sent twice. The exception is a slot that cannot take this file any more: `complete_upload` or
 * `create_upload` answered `conflict` (what the slot holds, or how it was announced, does not match
 * the file), the slot is gone (`not_found`), its count of bytes could not be read, or its finish
 * was not confirmed. Then the next try reads the file again and opens a new slot under new keys.
 * A slot that says it holds more than the file is one of these through its finish: no bytes are
 * sent, the finish is asked, and the service refuses it. The old slot stays with the service: the
 * client has no operation that lists or removes a slot.
 *
 * For the same reason a slot is opened only when its file's turn comes, never for the whole list
 * up front, so a file removed before its turn leaves nothing behind. Removing a file under way
 * aborts its requests.
 *
 * Progress is written to the list when the whole-percent value changes, not at every report of
 * the browser, so a large list is not redrawn many times a second.
 */
import { completeUpload, createUpload, uploadContent } from '../../api/generated/sdk.gen';
import { bytesSender } from '../../app/upload-transport';
import { classifyFailure } from '../errors/failure';
import { sha256Hex } from './hash';
import { toBeSent, type UploadEntry, type UploadStep, underWay, useUploads } from './upload-list';

/**
 * How many files are under way at once: three.
 *
 * Browsers open at most six connections to one origin over HTTP/1.1, which is what the service on
 * this computer speaks; three leaves the others to the rest of the workspace (autosave, listings,
 * the jobs list), so the page stays usable during a long upload, and still overlaps the short
 * requests of small files.
 */
export const UPLOADS_AT_ONCE = 3;

/** The service answered `upload_content` holding another count than the file's length. */
export class UploadIncomplete extends Error {
  readonly received: number;
  readonly size: number;

  constructor(received: number, size: number) {
    super(`The service holds ${received} of ${size} bytes`);
    this.name = 'UploadIncomplete';
    this.received = received;
    this.size = size;
  }
}

/** `complete_upload` answered, but not that this file's length and hash were verified. */
export class UploadNotConfirmed extends Error {
  constructor() {
    super('The service did not confirm the upload');
    this.name = 'UploadNotConfirmed';
  }
}

/** The service answered the slot with a count of bytes that is not a decimal number. */
export class SlotUnusable extends Error {
  constructor(received: string) {
    super(`The service said the slot holds "${received}" bytes, which is not a count`);
    this.name = 'SlotUnusable';
  }
}

const running = new Map<string, AbortController>();
/** Callers waiting for an entry's try to end (`retryUpload`). */
const waiting = new Map<string, Array<() => void>>();
/** The reading of files for their hash, one after another. */
let reading: Promise<unknown> = Promise.resolve();

const uploads = () => useUploads.getState();
const entryOf = (id: string) => uploads().entries.find((entry) => entry.id === id);

/** A decimal byte count off the wire; null when it is not one. */
function bytesIn(count: string): number | null {
  return /^\d+$/.test(count) ? Number(count) : null;
}

function readInTurn(file: File, signal: AbortSignal): Promise<string> {
  const turn = reading.then(() => {
    if (signal.aborted) throw new DOMException('The upload was removed', 'AbortError');
    return sha256Hex(file);
  });
  reading = turn.catch(() => undefined);
  return turn;
}

/** After this failure the slot cannot take the file: the next try opens another. */
function needsNewSlot(error: unknown, step: UploadStep): boolean {
  if (error instanceof UploadNotConfirmed || error instanceof SlotUnusable) return true;
  const failure = classifyFailure(error);
  if (failure.kind !== 'api') return false;
  if (failure.error.code === 'not_found') return true;
  return failure.error.code === 'conflict' && step !== 'bytes';
}

async function run(id: string): Promise<void> {
  const controller = new AbortController();
  running.set(id, controller);
  const { signal } = controller;
  let step: UploadStep = 'read';
  try {
    const entry = entryOf(id);
    if (entry === undefined) return;
    const { file, size, workspaceId } = entry;
    // For the type: only an entry that is `chosen` or `failed` is given a turn, and each has a File.
    if (file === null) return;
    uploads().change(id, { state: 'reading', sent: 0 });
    const sha256 = entry.sha256 ?? (await readInTurn(file, signal));
    // Removed while it was read: nothing is asked for it.
    if (signal.aborted) return;
    uploads().change(id, { sha256 });

    step = 'slot';
    const { data: slot } = await createUpload({
      body: {
        workspace_id: workspaceId,
        filename: entry.filename,
        relative_path: entry.folder,
        size: String(size),
        sha256,
        idempotency_key: entry.slotKey,
      },
      signal,
      throwOnError: true,
    });
    const held = bytesIn(slot.received_bytes);
    if (held === null) throw new SlotUnusable(slot.received_bytes);

    if (held < size) {
      step = 'bytes';
      uploads().change(id, { state: 'sending', sent: held });
      const rest = held === 0 ? file : file.slice(held);
      let shown = Math.floor((100 * held) / size);
      const { data: sent } = await uploadContent({
        path: { workspace_id: workspaceId, upload_id: slot.id },
        body: rest,
        fetch: bytesSender(rest, (gone) => {
          const percent = Math.floor((100 * (held + gone)) / size);
          if (percent === shown) return;
          shown = percent;
          uploads().change(id, { sent: held + gone });
        }),
        signal,
        throwOnError: true,
      });
      const now = bytesIn(sent.received_bytes);
      if (now !== size) throw new UploadIncomplete(now ?? 0, size);
    }

    step = 'finish';
    uploads().change(id, { state: 'finishing', sent: size });
    const { data: finished } = await completeUpload({
      body: {
        workspace_id: workspaceId,
        upload_id: slot.id,
        sha256,
        idempotency_key: entry.finishKey,
      },
      signal,
      throwOnError: true,
    });
    if (!finished.complete || bytesIn(finished.received_bytes) !== size) {
      throw new UploadNotConfirmed();
    }
    uploads().change(id, { state: 'uploaded', failure: null, uploadId: slot.id });
  } catch (error) {
    // Removed by the person: its requests were aborted and the entry is gone.
    if (signal.aborted) return;
    uploads().change(id, {
      state: 'failed',
      failure: { error, step },
      ...(needsNewSlot(error, step)
        ? { slotKey: crypto.randomUUID(), finishKey: crypto.randomUUID(), sha256: null }
        : {}),
    });
  } finally {
    running.delete(id);
    for (const settled of waiting.get(id) ?? []) settled();
    waiting.delete(id);
    pump();
  }
}

/** Start the waiting files, in list order, up to `UPLOADS_AT_ONCE` under way. */
function pump() {
  for (const entry of uploads().entries) {
    if (running.size >= UPLOADS_AT_ONCE) return;
    if (entry.state === 'waiting' && !running.has(entry.id)) void run(entry.id);
  }
}

/**
 * Upload every file of the workspace that is listed and not yet asked for, except those held back
 * because their path collides with another listed file's (`upload-list.ts`).
 */
export function uploadChosen(workspaceId: string) {
  for (const entry of toBeSent(uploads().entries)) {
    if (entry.workspaceId === workspaceId) uploads().change(entry.id, { state: 'waiting' });
  }
  pump();
}

/** Try a failed file again. Settles when that try has ended, however it ended. */
export function retryUpload(id: string): Promise<void> {
  if (entryOf(id)?.state !== 'failed') return Promise.resolve();
  return new Promise((resolve) => {
    waiting.set(id, [...(waiting.get(id) ?? []), resolve]);
    uploads().change(id, { state: 'waiting' });
    pump();
  });
}

/** Take a file off the list; one under way has its requests aborted. */
export function removeUpload(id: string) {
  running.get(id)?.abort();
  uploads().remove([id]);
  pump();
}

/**
 * Leaving the page now would lose work: a file is under way, or one is uploaded and not yet
 * imported (the page alone knows its slot).
 */
export function uploadsAtRisk(): boolean {
  return uploads().entries.some(
    (entry: UploadEntry) => underWay(entry) || entry.state === 'uploaded',
  );
}

/** Abort every upload and empty the list. For tests. */
export function resetUploads() {
  for (const controller of running.values()) controller.abort();
  running.clear();
  waiting.clear();
  reading = Promise.resolve();
  useUploads.setState({ entries: [], started: {} });
}
