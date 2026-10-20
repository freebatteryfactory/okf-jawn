/**
 * The transport's byte upload: the CSRF header by the same rule as every request, progress, and a
 * wait that is measured from the last movement, never from the start.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { uploadContent } from '../../src/api/generated/sdk.gen';
import {
  CSRF_COOKIE,
  CSRF_HEADER,
  REQUEST_TIMEOUT_MS,
  RequestTimedOut,
} from '../../src/app/api-client';
import { createQueryClient } from '../../src/app/query-client';
import { bytesSender, sendBytes } from '../../src/app/upload-transport';
import { classifyFailure } from '../../src/features/errors/failure';
import { fail, ok } from './support/api';
import { mockTransfers } from './support/transfers';
import { WS } from './support/workspace';

const TOKEN = '0123456789abcdef0123456789abcdef';
const SLOT = '66666666-6666-4666-8666-666666666666';
const PATH = `/api/workspaces/${WS}/uploads/${SLOT}/content`;
const BYTES = new Blob(['twelve bytes']);

const slot = (received: number, complete = false) => ({
  id: SLOT,
  workspace_id: WS,
  upload_path: PATH,
  received_bytes: String(received),
  complete,
});

function browserHolds(cookies: string) {
  vi.spyOn(document, 'cookie', 'get').mockReturnValue(cookies);
}

/** The request the generated client builds for this upload. */
function put(signal?: AbortSignal): Request {
  return new Request(new URL(PATH, window.location.href), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: BYTES,
    ...(signal === undefined ? {} : { signal }),
  });
}

/** How a promise stands now: its outcome once the microtasks queued so far have run. */
async function standing(promise: Promise<unknown>): Promise<unknown> {
  let outcome: unknown = 'pending';
  promise.then(
    () => {
      outcome = 'resolved';
    },
    (error: unknown) => {
      outcome = error;
    },
  );
  for (let tick = 0; tick < 5; tick += 1) await Promise.resolve();
  return outcome;
}

beforeEach(() => {
  createQueryClient();
  browserHolds('');
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('what the byte upload carries', () => {
  it('carries the CSRF cookie’s value, the method, the media type and the Blob itself', async () => {
    browserHolds(`theme=dark; ${CSRF_COOKIE}=${TOKEN}`);
    const sent = mockTransfers(() => ok(slot(12)));
    const response = await sendBytes(put(), BYTES, () => {});
    expect(sent.transfers).toHaveLength(1);
    const transfer = sent.transfers.at(0);
    expect(transfer?.method).toBe('PUT');
    expect(transfer?.path).toBe(PATH);
    expect(transfer?.headers[CSRF_HEADER]).toBe(TOKEN);
    expect(transfer?.headers['content-type']).toBe('application/octet-stream');
    expect(transfer?.bytes).toBe(BYTES);
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual(slot(12));
  });

  it('carries no CSRF header without the cookie', async () => {
    const sent = mockTransfers(() => ok(slot(12)));
    await sendBytes(put(), BYTES, () => {});
    expect(sent.transfers.at(0)?.headers).not.toHaveProperty(CSRF_HEADER);
  });

  it('does not send the token to another origin', async () => {
    browserHolds(`${CSRF_COOKIE}=${TOKEN}`);
    const sent = mockTransfers(() => ok(slot(12)));
    const elsewhere = new Request(`https://elsewhere.example${PATH}`, {
      method: 'PUT',
      body: BYTES,
    });
    await sendBytes(elsewhere, BYTES, () => {});
    expect(sent.transfers.at(0)?.headers).not.toHaveProperty(CSRF_HEADER);
  });

  it('sends the token once when the request already names the header', async () => {
    browserHolds(`${CSRF_COOKIE}=${TOKEN}`);
    const sent = mockTransfers(() => ok(slot(12)));
    const request = put();
    request.headers.set(CSRF_HEADER, 'stale');
    await sendBytes(request, BYTES, () => {});
    expect(sent.transfers.at(0)?.headers[CSRF_HEADER]).toBe(TOKEN);
  });

  it('goes out through the generated client, which builds the route and reads the answer', async () => {
    browserHolds(`${CSRF_COOKIE}=${TOKEN}`);
    const sent = mockTransfers(() => ok(slot(12)));
    const fetched = vi.fn();
    vi.stubGlobal('fetch', fetched);
    const progress: number[] = [];
    const { data } = await uploadContent({
      path: { workspace_id: WS, upload_id: SLOT },
      body: BYTES,
      fetch: bytesSender(BYTES, (bytes) => progress.push(bytes)),
      throwOnError: true,
    });
    expect(data).toEqual(slot(12));
    expect(sent.transfers.map((transfer) => [transfer.method, transfer.path])).toEqual([
      ['PUT', PATH],
    ]);
    expect(sent.transfers.at(0)?.headers[CSRF_HEADER]).toBe(TOKEN);
    expect(sent.transfers.at(0)?.headers['content-type']).toBe('application/octet-stream');
    expect(sent.transfers.at(0)?.bytes).toBe(BYTES);
    expect(progress).toEqual([12]);
    // Nothing of the upload went through `fetch`.
    expect(fetched).not.toHaveBeenCalled();
    expect(sent.violations).toEqual([]);
  });

  it('gives a refusal to the generated client as the service’s error', async () => {
    mockTransfers(() => fail(413, 'too_large', 'the upload would exceed its announced size'));
    const thrown = await uploadContent({
      path: { workspace_id: WS, upload_id: SLOT },
      body: BYTES,
      fetch: bytesSender(BYTES, () => {}),
      throwOnError: true,
    }).then(
      () => null,
      (error: unknown) => error,
    );
    const failure = classifyFailure(thrown);
    expect(failure.kind === 'api' ? failure.error.code : failure.kind).toBe('too_large');
  });
});

describe('progress', () => {
  it('reports the bytes gone each time the browser reports them', async () => {
    const sent = mockTransfers();
    const progress: number[] = [];
    const upload = sendBytes(put(), BYTES, (bytes) => progress.push(bytes));
    const transfer = sent.transfers.at(0);
    transfer?.progress(4);
    transfer?.progress(9);
    expect(progress).toEqual([4, 9]);
    transfer?.allSent();
    transfer?.answer(ok(slot(12)));
    await upload;
    expect(progress).toEqual([4, 9, 12]);
  });
});

describe('what bounds an upload', () => {
  it('ends an upload that has sent no byte for the wait, as a timeout, and aborts it', async () => {
    vi.useFakeTimers();
    const sent = mockTransfers();
    const upload = sendBytes(put(), BYTES, () => {});
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    expect(await standing(upload)).toBe('pending');
    expect(sent.transfers.at(0)?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const outcome = await standing(upload);
    expect(outcome).toBeInstanceOf(RequestTimedOut);
    expect(classifyFailure(outcome)).toEqual({
      kind: 'timed_out',
      seconds: REQUEST_TIMEOUT_MS / 1000,
    });
    expect(sent.transfers.at(0)?.aborted).toBe(true);
  });

  it('lets an upload take any time while it keeps moving: the wait starts again at each byte', async () => {
    vi.useFakeTimers();
    const sent = mockTransfers();
    const upload = sendBytes(put(), BYTES, () => {});
    const transfer = sent.transfers.at(0);
    // Four times the wait in all, never the whole wait without movement.
    for (let step = 1; step <= 4; step += 1) {
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
      transfer?.progress(step * 2);
    }
    expect(await standing(upload)).toBe('pending');
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    expect(await standing(upload)).toBe('pending');
    await vi.advanceTimersByTimeAsync(1);
    expect(await standing(upload)).toBeInstanceOf(RequestTimedOut);
  });

  it('waits for the answer no longer than the wait after the last byte', async () => {
    vi.useFakeTimers();
    const sent = mockTransfers();
    const upload = sendBytes(put(), BYTES, () => {});
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    sent.transfers.at(0)?.allSent();
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    expect(await standing(upload)).toBe('pending');
    await vi.advanceTimersByTimeAsync(1);
    expect(await standing(upload)).toBeInstanceOf(RequestTimedOut);
    expect(sent.transfers.at(0)?.aborted).toBe(true);
  });

  it('stops the clock once the answer has come', async () => {
    vi.useFakeTimers();
    const sent = mockTransfers();
    const upload = sendBytes(put(), BYTES, () => {});
    sent.transfers.at(0)?.allSent();
    sent.transfers.at(0)?.answer(ok(slot(12)));
    await upload;
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('an upload that ends without an answer', () => {
  it('ends as an abort, not a timeout, when the caller aborts it', async () => {
    const sent = mockTransfers();
    const caller = new AbortController();
    const upload = sendBytes(put(caller.signal), BYTES, () => {});
    caller.abort();
    const outcome = await standing(upload);
    expect(outcome).toBeInstanceOf(DOMException);
    expect(outcome).toHaveProperty('name', 'AbortError');
    expect(sent.transfers.at(0)?.aborted).toBe(true);
  });

  it('sends nothing for a request that is already aborted', async () => {
    const sent = mockTransfers();
    const caller = new AbortController();
    caller.abort();
    const outcome = await standing(sendBytes(put(caller.signal), BYTES, () => {}));
    expect(outcome).toHaveProperty('name', 'AbortError');
    expect(sent.transfers).toHaveLength(0);
  });

  it('fails as "could not be reached" when the connection breaks', async () => {
    const sent = mockTransfers();
    const upload = sendBytes(put(), BYTES, () => {});
    sent.transfers.at(0)?.breakConnection();
    expect(classifyFailure(await standing(upload)).kind).toBe('unreachable');
  });
});
