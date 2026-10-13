/**
 * Ground for the import tests: files as a browser hands them over, a small upload service that
 * keeps the rules of the real one, and contract-valid jobs.
 *
 * The upload service follows `crates/storage/src/uploads.rs`, which the tests do not run:
 * - `create_upload` is unique on its idempotency key: a repeat opens nothing and answers the slot
 *   as it stands, with the bytes it holds;
 * - `upload_content` appends; a body that would pass the announced size appends nothing and is
 *   `too_large`; a complete slot takes no more bytes (`conflict`);
 * - `complete_upload` is `conflict` unless the slot holds exactly the announced length, and unless
 *   the SHA-256 of what it holds is the one sent (and the one announced). The hash is computed
 *   here from the bytes that arrived, so a wrong hash or wrong bytes are refused, not believed.
 * A test scripts the awkward answers: each scripted step replaces one answer, in order.
 */
import { vi } from 'vitest';
import type { Job, Upload } from '../../../src/api/generated/types.gen';
import {
  zCancelJobRequest,
  zCompleteUploadRequest,
  zCreateUploadRequest,
  zGetAttentionRequest,
  zGetAttentionResponse,
  zGetJobRequest,
  zJob,
  zListJobsRequest,
  zListJobsResponse,
  zRetryJobRequest,
  zStartImportRequest,
  zUpload,
} from '../../../src/api/generated/zod.gen';
import { CONTRACT, fail, type Handler, type Json, ok, type Reply } from './api';
import type { Transfer, TransferHandler } from './transfers';
import { HEAD, WS } from './workspace';

export const CREATE = '/api/imports/create-upload';
export const COMPLETE = '/api/imports/complete-upload';
export const START = '/api/imports/start-import';
export const JOBS = '/api/imports/list-jobs';
export const JOB = '/api/imports/get-job';
export const CANCEL = '/api/imports/cancel-job';
export const RETRY = '/api/imports/retry-job';
export const ATTENTION = '/api/attention/get-attention';

Object.assign(CONTRACT, {
  [CREATE]: { request: zCreateUploadRequest, response: zUpload },
  [COMPLETE]: { request: zCompleteUploadRequest, response: zUpload },
  [START]: { request: zStartImportRequest, response: zJob },
  [JOBS]: { request: zListJobsRequest, response: zListJobsResponse },
  [JOB]: { request: zGetJobRequest, response: zJob },
  [CANCEL]: { request: zCancelJobRequest, response: zJob },
  [RETRY]: { request: zRetryJobRequest, response: zJob },
  [ATTENTION]: { request: zGetAttentionRequest, response: zGetAttentionResponse },
});

/** SHA-256 of "abc" and of nothing: published test vectors, not computed by the code under test. */
export const SHA_ABC = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
export const SHA_EMPTY = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';

/**
 * A file as a picker hands it over. `relativePath` is what a folder picker names
 * (`webkitRelativePath`): the path from the chosen folder down, ending in the file's name.
 */
export function fileOf(name: string, content: string, relativePath = ''): File {
  const file = new File([content], name, { lastModified: 1_700_000_000_000 });
  Object.defineProperty(file, 'webkitRelativePath', { value: relativePath });
  return file;
}

/** A file whose length the browser reports as `size`, whatever it holds: nothing reads that much. */
export function fileOfSize(name: string, size: number): File {
  const file = fileOf(name, 'abc');
  Object.defineProperty(file, 'size', { value: size });
  return file;
}

/** A dropped file, as the browser's entry API names it. */
export function droppedFile(file: File) {
  return {
    isFile: true,
    isDirectory: false,
    name: file.name,
    file: (found: (file: File) => void) => found(file),
  };
}

/**
 * A dropped folder. Its reader gives its entries in batches, as browsers do, and an empty batch at
 * the end; `unreadable` makes it fail instead.
 */
export function droppedFolder(name: string, children: unknown[], unreadable = false) {
  return {
    isFile: false,
    isDirectory: true,
    name,
    createReader: () => {
      const batches = [children.slice(0, 1), children.slice(1)].filter((batch) => batch.length > 0);
      return {
        readEntries: (found: (entries: unknown[]) => void, failed: (error: Error) => void) => {
          if (unreadable) failed(new Error('The folder could not be read'));
          else found(batches.shift() ?? []);
        },
      };
    },
  };
}

/** What a drop carries: one item for each thing dropped. */
export function dropOf(entries: unknown[]) {
  return {
    items: entries.map((entry) => ({ webkitGetAsEntry: () => entry, getAsFile: () => null })),
    files: [],
  };
}

interface Slot {
  id: string;
  key: string;
  filename: string;
  relativePath: string;
  expected: number;
  announced: string | null;
  parts: Blob[];
  complete: boolean;
}

async function sha256Of(content: Blob): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', await content.arrayBuffer());
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

type Scripted<Input> = (
  input: Input,
  usual: () => Reply | Promise<Reply>,
) => Reply | Promise<Reply>;

export interface UploadService {
  handlers: Record<string, Handler>;
  transfer: TransferHandler;
  /** Every slot opened, in order. */
  slots: Slot[];
  /** The bytes a slot holds, as text. */
  held: (slot: Slot | undefined) => Promise<string>;
  /** Take a transfer's bytes as the service usually does, and say what it answers. */
  accept: (transfer: Transfer) => Reply;
  /** Answers that replace the usual one, in order, for each step. */
  script: {
    create: Array<Scripted<Json>>;
    /** Returning nothing leaves the transfer open for the test to drive. */
    put: Array<(transfer: Transfer, usual: () => Reply) => Reply | Promise<Reply> | undefined>;
    complete: Array<Scripted<Json>>;
  };
}

export function uploadService(): UploadService {
  const slots: Slot[] = [];
  const view = (slot: Slot): Upload => ({
    id: slot.id,
    workspace_id: WS,
    upload_path: `/api/workspaces/${WS}/uploads/${slot.id}/content`,
    received_bytes: String(slot.parts.reduce((sum, part) => sum + part.size, 0)),
    complete: slot.complete,
  });
  const accept = (transfer: Transfer): Reply => {
    const slot = slots.find((candidate) => transfer.path.includes(candidate.id));
    if (slot === undefined) return fail(404, 'not_found', 'no such upload');
    if (slot.complete) return fail(409, 'conflict', 'this upload is already complete');
    const holds = slot.parts.reduce((sum, part) => sum + part.size, 0);
    if (holds + transfer.bytes.size > slot.expected) {
      return fail(
        413,
        'too_large',
        'the upload would exceed its announced size or the limit; nothing was appended',
      );
    }
    slot.parts.push(transfer.bytes);
    return ok(view(slot));
  };
  const service: UploadService = {
    slots,
    held: (slot) => new Blob(slot?.parts ?? []).text(),
    accept,
    script: { create: [], put: [], complete: [] },
    handlers: {
      [CREATE]: (body) => {
        const usual = () => {
          const known = slots.find((slot) => slot.key === body.idempotency_key);
          if (known !== undefined) return ok(view(known));
          const slot: Slot = {
            id: `77777777-7777-4777-8777-${String(slots.length + 1).padStart(12, '0')}`,
            key: String(body.idempotency_key),
            filename: String(body.filename),
            relativePath: String(body.relative_path),
            expected: Number(body.size),
            announced: typeof body.sha256 === 'string' ? body.sha256 : null,
            parts: [],
            complete: false,
          };
          slots.push(slot);
          return ok(view(slot));
        };
        return (service.script.create.shift() ?? ((_body, proceed) => proceed()))(body, usual);
      },
      [COMPLETE]: (body) => {
        const usual = async () => {
          const slot = slots.find((candidate) => candidate.id === body.upload_id);
          if (slot === undefined) return fail(404, 'not_found', 'no such upload');
          const content = new Blob(slot.parts);
          if (content.size !== slot.expected) {
            return fail(
              409,
              'conflict',
              `received ${content.size} bytes of the ${slot.expected} announced`,
            );
          }
          const digest = await sha256Of(content);
          if (digest !== body.sha256 || (slot.announced !== null && slot.announced !== digest)) {
            return fail(409, 'conflict', 'the digest differs from the one announced');
          }
          slot.complete = true;
          return ok(view(slot));
        };
        return (service.script.complete.shift() ?? ((_body, proceed) => proceed()))(body, usual);
      },
    },
    transfer: (transfer) => {
      const scripted = service.script.put.shift();
      return scripted === undefined ? accept(transfer) : scripted(transfer, () => accept(transfer));
    },
  };
  return service;
}

export const JOB_ID = '88888888-8888-4888-8888-888888888888';
/** The revision an import made. */
export const IMPORTED = 'f'.repeat(40);

export function jobOf(state: Job['state'], extra: Partial<Job> = {}): Job {
  return {
    id: JOB_ID,
    kind: 'import',
    state,
    attempt: 1,
    progress: state === 'succeeded' ? 100 : 0,
    item_ids: [],
    warnings: [],
    workspace_id: WS,
    ...extra,
  };
}

/** `Date.now` and timers under the test's control, with the page shown. */
export function pageShown(shown: boolean) {
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue(shown ? 'visible' : 'hidden');
  document.dispatchEvent(new Event('visibilitychange', { bubbles: true }));
}

export { HEAD };
