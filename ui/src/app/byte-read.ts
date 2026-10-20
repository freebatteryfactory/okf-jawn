/**
 * The transport's one byte read (ruling A2): an object's bytes, or a range of them, as a `Blob`.
 *
 * Bytes are read through `download_object`, the transport that authorizes an object through an
 * exact item revision and supports HTTP Range (`api/transports.json`), never through `get_object`.
 * The generated `downloadObject` builds the request (its route and method) and is sent through the
 * one configured transport (`api-client.ts` `sendRequest`), so the CSRF rule and the typed
 * failures are the same as every request's: a refusal is the service's `ApiError`, read by
 * `features/errors/failure.ts`. Nothing here calls `fetch`.
 *
 * Time limit. `sendRequest` bounds a non-JSON answer only until it begins. A byte read is bounded,
 * so its whole answer, the bytes included, gets the same `REQUEST_TIMEOUT_MS`: a read whose bytes
 * have not all come by then is aborted and fails as `RequestTimedOut`, which can be retried. The
 * caller's own abort (a cancelled query, a closed tab) ends it as that abort, not as a timeout.
 *
 * A viewer hands the Blob on: an object URL for `<img>` (revoked when the viewer lets it go), or
 * its bytes to PDF.js with `getDocument({ data })`.
 */
import { downloadObject } from '../api/generated/sdk.gen';
import { REQUEST_TIMEOUT_MS, RequestTimedOut } from './api-client';

/** A part of an object: `length` bytes from `offset`. */
export interface ByteRange {
  offset: number;
  length: number;
}

export interface ObjectBytes {
  workspaceId: string;
  itemId: string;
  /** The exact revision the object is authorized through. */
  revision: string;
  /** The object's SHA-256. */
  digest: string;
  /** A part of it; the whole object when absent. */
  range?: ByteRange;
  /** The caller's abort. */
  signal?: AbortSignal;
}

/** The HTTP Range header for a part: inclusive at both ends, as RFC 9110 says. */
function rangeHeader({ offset, length }: ByteRange): string {
  return `bytes=${offset}-${offset + length - 1}`;
}

/** Read the object's bytes, or the part asked; rejects with the transport's typed failures. */
export async function readObjectBytes({
  workspaceId,
  itemId,
  revision,
  digest,
  range,
  signal,
}: ObjectBytes): Promise<Blob> {
  const deadline = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      // Rejected before the abort, so the read ends as a timeout and not as the abort it causes.
      reject(new RequestTimedOut(REQUEST_TIMEOUT_MS));
      deadline.abort();
    }, REQUEST_TIMEOUT_MS);
  });
  try {
    const reading = downloadObject({
      path: { workspace_id: workspaceId, item_id: itemId, revision, digest },
      ...(range === undefined ? {} : { headers: { Range: rangeHeader(range) } }),
      parseAs: 'blob',
      signal: signal === undefined ? deadline.signal : AbortSignal.any([signal, deadline.signal]),
      throwOnError: true,
    });
    const { data } = await Promise.race([reading, timedOut]);
    return data;
  } finally {
    clearTimeout(timer);
  }
}
