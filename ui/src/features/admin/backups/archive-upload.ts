/**
 * Uploading one backup archive, for a restore (ruling C6): a slot of this workspace
 * (`create_upload`, with the archive's name, size and SHA-256), its bytes through the upload
 * transport (`upload_content` with `bytesSender`), and the finish (`complete_upload`) against its
 * length and SHA-256. It is an import's upload (`imports/uploading.ts`) for the one file chosen
 * here, and keeps the same rules, step by step:
 * 1. The file is read once (`imports/hash.ts`): the bytes sent are the bytes hashed. A reading
 *    whose length is not the file's is a file that changed since it was chosen (`FileChanged`),
 *    and nothing is asked for it. A reading whose hash is not the one an earlier try announced is a
 *    file that changed between two tries: that slot was announced for other content, so this try
 *    opens a new one.
 * 2. `create_upload` is asked at the start of every try, under the slot's key: a repeat opens
 *    nothing and answers the slot as it was first answered. So when an earlier try sent the slot
 *    bytes, the slot is read as it stands now (`get_upload`), and the rest is sent from what it
 *    holds.
 * 3. `upload_content` sends the bytes the slot lacks; its answer must say that the slot now holds
 *    exactly the archive's length (`UploadIncomplete`).
 * 4. `complete_upload` carries the hash, under the finish's key. The archive counts as uploaded only
 *    when the answer says `complete` and holds exactly its length (`UploadNotConfirmed`).
 *
 * Keys. A try of the same file goes on under the same keys, so nothing is opened or sent twice;
 * after a failure that leaves the slot unable to take this file (`needsNewSlot`, as for an
 * import: a refused slot or finish, a slot that is gone, unreadable or ahead of what it says, an
 * unconfirmed finish) the next try opens a new slot under new keys. Once the service has confirmed
 * the upload, the next upload is a new one. The keys and the hash are the page's own (the
 * caller's ref); nothing the service answered is kept here: the uploaded archive is the answer of
 * the caller's mutation.
 *
 * What stays with the service. The slot stays until a restore takes it, listed with this
 * workspace's uploads on its Import page, where it can be discarded (`WaitingUploads.tsx`); a slot
 * left for a new one stays there too.
 */
import {
  completeUpload,
  createUpload,
  getUpload,
  uploadContent,
} from '../../../api/generated/sdk.gen';
import { bytesSender } from '../../../app/upload-transport';
import { classifyFailure } from '../../errors/failure';
import { readForUpload } from '../../imports/hash';
import type { UploadStep } from '../../imports/upload-list';
import {
  FileChanged,
  SlotUnusable,
  slotIsAhead,
  UploadIncomplete,
  UploadNotConfirmed,
} from '../../imports/uploading';
import type { ArchiveUploadFailure } from './backup-words';

/** The keys of one file's tries, and what earlier tries left. */
export interface ArchiveKeys {
  slotKey: string;
  finishKey: string;
  /** The SHA-256 an earlier try announced; null before one did. */
  sha256: string | null;
  /** An earlier try sent bytes to the slot these keys name. */
  slotBegan: boolean;
}

/** Keys for a new slot. */
export function freshKeys(): ArchiveKeys {
  return {
    slotKey: crypto.randomUUID(),
    finishKey: crypto.randomUUID(),
    sha256: null,
    slotBegan: false,
  };
}

/** An archive the service holds, complete, with the length and SHA-256 it checked. */
export interface UploadedArchive {
  uploadId: string;
  filename: string;
  size: number;
  sha256: string;
}

/** A try that failed, and where. */
export class ArchiveUploadFailed extends Error {
  readonly failure: ArchiveUploadFailure;

  constructor(failure: ArchiveUploadFailure) {
    super('The archive was not uploaded', { cause: failure.error });
    this.name = 'ArchiveUploadFailed';
    this.failure = failure;
  }
}

/** A decimal byte count off the wire; null when it is not one. */
function bytesIn(count: string): number | null {
  return /^\d+$/.test(count) ? Number(count) : null;
}

/** After this failure the slot cannot take the file: the next try opens another. */
function needsNewSlot(failed: ArchiveUploadFailure): boolean {
  const { error, step } = failed;
  if (error instanceof UploadNotConfirmed || error instanceof SlotUnusable) return true;
  if (slotIsAhead(failed)) return true;
  const failure = classifyFailure(error);
  if (failure.kind !== 'api') return false;
  if (failure.error.code === 'not_found') return true;
  return failure.error.code === 'conflict' && step !== 'bytes';
}

export interface ArchiveUpload {
  workspaceId: string;
  file: File;
  /** This file's keys: read at the start of the try, and updated as it goes. */
  keys: { current: ArchiveKeys };
  /** How many bytes of the archive the service holds or has been sent, as the browser reports. */
  onSent: (sent: number) => void;
  signal: AbortSignal;
}

/** One try of the archive's upload. Rejects with `ArchiveUploadFailed`, or an abort. */
export async function uploadArchive({
  workspaceId,
  file,
  keys,
  onSent,
  signal,
}: ArchiveUpload): Promise<UploadedArchive> {
  let step: UploadStep = 'read';
  let resumed = false;
  try {
    const { bytes, sha256 } = await readForUpload(file);
    if (signal.aborted) throw new DOMException('The upload was stopped', 'AbortError');
    if (bytes.size !== file.size) throw new FileChanged(file.size, bytes.size);
    const size = bytes.size;
    const before = keys.current;
    // An earlier try announced a slot for other content: the file changed between the tries.
    const changed = before.sha256 !== null && before.sha256 !== sha256;
    const current = changed ? { ...freshKeys(), sha256 } : { ...before, sha256 };
    resumed = current.slotBegan;
    keys.current = current;

    step = 'slot';
    const { data: opened } = await createUpload({
      body: {
        workspace_id: workspaceId,
        filename: file.name,
        relative_path: '',
        size: String(size),
        sha256,
        idempotency_key: current.slotKey,
      },
      signal,
      throwOnError: true,
    });
    // A repeat answers the slot as it was first answered: what it holds now is read from it.
    const slot = resumed
      ? (
          await getUpload({
            body: { workspace_id: workspaceId, upload_id: opened.id },
            signal,
            throwOnError: true,
          })
        ).data
      : opened;
    const held = bytesIn(slot.received_bytes);
    if (held === null) throw new SlotUnusable(slot.received_bytes);

    if (held < size) {
      step = 'bytes';
      keys.current = { ...keys.current, slotBegan: true };
      onSent(held);
      const rest = held === 0 ? bytes : bytes.slice(held);
      const { data: sent } = await uploadContent({
        path: { workspace_id: workspaceId, upload_id: slot.id },
        body: rest,
        fetch: bytesSender(rest, (gone) => onSent(held + gone)),
        signal,
        throwOnError: true,
      });
      const now = bytesIn(sent.received_bytes);
      if (now !== size) throw new UploadIncomplete(now ?? 0, size);
    }

    step = 'finish';
    onSent(size);
    const { data: finished } = await completeUpload({
      body: {
        workspace_id: workspaceId,
        upload_id: slot.id,
        sha256,
        idempotency_key: current.finishKey,
      },
      signal,
      throwOnError: true,
    });
    if (!finished.complete || bytesIn(finished.received_bytes) !== size) {
      throw new UploadNotConfirmed();
    }
    // Answered: another upload is another slot.
    keys.current = freshKeys();
    return { uploadId: slot.id, filename: file.name, size, sha256 };
  } catch (error) {
    if (signal.aborted) throw error;
    const failure: ArchiveUploadFailure = { error, step, resumed };
    // The service refused the bytes of this try: it kept none of them.
    const refusedBytes = step === 'bytes' && classifyFailure(error).kind === 'api';
    const began = refusedBytes ? resumed : keys.current.slotBegan;
    keys.current = needsNewSlot(failure) ? freshKeys() : { ...keys.current, slotBegan: began };
    throw new ArchiveUploadFailed(failure);
  }
}
