/**
 * The transport's one byte read (ruling A2): an object's bytes, or a range of them, read through
 * `download_object` (never `get_object`) as a `Blob`, with the transport's time limit and its typed
 * failures. A viewer hands the Blob on: an object URL for `<img>`, `getDocument({ data })` for
 * PDF.js. The request goes through the generated client's `fetch` (the transport), so nothing
 * here calls `fetch` itself.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApiClient, REQUEST_TIMEOUT_MS, RequestTimedOut } from '../../src/app/api-client';
import { readObjectBytes } from '../../src/app/byte-read';
import { classifyFailure } from '../../src/features/errors/failure';

const WS = '11111111-1111-4111-8111-111111111111';
const ITEM = '22222222-2222-4222-8222-222222222222';
const REVISION = 'a'.repeat(40);
const DIGEST = 'b'.repeat(64);
const OBJECT_PATH = `/api/workspaces/${WS}/items/${ITEM}/revisions/${REVISION}/objects/${DIGEST}`;

interface Seen {
  method: string;
  path: string;
  range: string | null;
}

/**
 * A fetch that answers `respond` and records what was asked. Like `fetch`, it heeds the signal
 * its call is given over the request's own.
 */
function stubFetch(
  respond: (request: Request, signal: AbortSignal) => Response | Promise<Response>,
): Seen[] {
  const seen: Seen[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      seen.push({
        method: request.method,
        path: new URL(request.url).pathname,
        range: request.headers.get('Range'),
      });
      return respond(request, init?.signal ?? request.signal);
    }),
  );
  configureApiClient();
  return seen;
}

const object = { workspaceId: WS, itemId: ITEM, revision: REVISION, digest: DIGEST };

beforeEach(() => {
  vi.useRealTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('reading an object’s bytes (A2)', () => {
  it('asks download_object with GET and answers the bytes as a Blob', async () => {
    const bytes = new Uint8Array([37, 80, 68, 70, 45]);
    const seen = stubFetch(
      () => new Response(bytes, { status: 200, headers: { 'Content-Type': 'application/pdf' } }),
    );
    const blob = await readObjectBytes(object);
    expect(blob).toBeInstanceOf(Blob);
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(bytes);
    expect(seen).toEqual([{ method: 'GET', path: OBJECT_PATH, range: null }]);
  });

  it('asks a range when given one, inclusive as HTTP says, and takes the partial answer', async () => {
    const seen = stubFetch(
      () =>
        new Response(new Uint8Array([1, 2, 3, 4]), {
          status: 206,
          headers: {
            'Content-Type': 'application/octet-stream',
            'Content-Range': 'bytes 10-13/100',
          },
        }),
    );
    const blob = await readObjectBytes({ ...object, range: { offset: 10, length: 4 } });
    expect(blob.size).toBe(4);
    expect(seen.at(0)?.range).toBe('bytes=10-13');
  });

  it('fails with the service’s typed error when it refuses', async () => {
    stubFetch(
      () =>
        new Response(JSON.stringify({ code: 'forbidden', message: 'not yours' }), {
          status: 403,
          headers: { 'Content-Type': 'application/json' },
        }),
    );
    const thrown = await readObjectBytes(object).then(
      () => null,
      (error: unknown) => error,
    );
    const failure = classifyFailure(thrown);
    expect(failure.kind).toBe('api');
    expect(failure.kind === 'api' && failure.error.code).toBe('forbidden');
  });

  it('fails as RequestTimedOut when the bytes have not all come within the time limit', async () => {
    vi.useFakeTimers();
    // Headers come at once; the body never ends.
    stubFetch(
      () =>
        new Response(new ReadableStream({ start() {} }), {
          status: 200,
          headers: { 'Content-Type': 'application/octet-stream' },
        }),
    );
    const reading = readObjectBytes(object).then(
      () => null,
      (error: unknown) => error,
    );
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS + 1);
    expect(await reading).toBeInstanceOf(RequestTimedOut);
  });

  it('ends as the caller’s abort, not as a timeout, when the caller stops it', async () => {
    stubFetch(
      (_request, signal) =>
        new Promise<Response>((_resolve, reject) => {
          // As `fetch` does: an abort before the call, or during it, rejects.
          const abort = () => reject(new DOMException('aborted', 'AbortError'));
          if (signal.aborted) abort();
          else signal.addEventListener('abort', abort);
        }),
    );
    const stop = new AbortController();
    const reading = readObjectBytes({ ...object, signal: stop.signal }).then(
      () => null,
      (error: unknown) => error,
    );
    stop.abort();
    const thrown = await reading;
    expect(thrown).not.toBeInstanceOf(RequestTimedOut);
    expect(thrown instanceof DOMException && thrown.name).toBe('AbortError');
  });
});
