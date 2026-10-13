/**
 * The workspace's transport: the CSRF header on every request the server asks it of, and one
 * bounded wait for every request, through the generated client and through the whole app.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { onlineManager } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { getFormSchema, openWorkspace } from '../../src/api/generated/sdk.gen';
import {
  CSRF_COOKIE,
  CSRF_HEADER,
  REQUEST_TIMEOUT_MS,
  RequestTimedOut,
  sendRequest,
  UPLOAD_BYTES_PER_SECOND,
} from '../../src/app/api-client';
import { createQueryClient } from '../../src/app/query-client';
import { classifyFailure, messageFor, shouldRetry } from '../../src/features/errors/failure';
import { fail, mockApi, ok } from './support/api';
import { explorerHandlers, NOTE, OLD, renderAt, WS, workspace } from './support/workspace';

const TOKEN = '0123456789abcdef0123456789abcdef';
const OPEN = '/api/workspaces/open-workspace';
/** The generated OpenAPI document at the repository root. */
const OPENAPI = join(dirname(fileURLToPath(import.meta.url)), '../../../api/openapi.json');

/**
 * What the browser holds as cookies, in the text `document.cookie` reads as. The session cookie is
 * HttpOnly and never appears in it; the browser specs set real cookies.
 */
function browserHolds(cookies: string) {
  vi.spyOn(document, 'cookie', 'get').mockReturnValue(cookies);
}

beforeEach(() => {
  // Making the app's cache is what puts the transport on the generated client.
  createQueryClient();
  browserHolds('');
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  onlineManager.setOnline(true);
});

describe('the CSRF names', () => {
  // The document is large; on a loaded machine reading it has taken longer than the default 5 s.
  it('are the names of their security schemes in api/openapi.json', { timeout: 60_000 }, () => {
    const scheme = z.object({ in: z.string(), name: z.string() });
    const published = z
      .object({
        components: z.object({
          securitySchemes: z.object({ csrfCookie: scheme, csrfHeader: scheme }),
        }),
      })
      .parse(JSON.parse(readFileSync(OPENAPI, 'utf8')));
    const schemes = published.components.securitySchemes;
    expect(schemes.csrfCookie).toEqual({ in: 'cookie', name: CSRF_COOKIE });
    expect(schemes.csrfHeader).toEqual({ in: 'header', name: CSRF_HEADER });
  });
});

describe('the CSRF header', () => {
  it('carries the cookie’s value on an operation', async () => {
    browserHolds(`theme=dark; ${CSRF_COOKIE}=${TOKEN}; lang=en`);
    const api = mockApi({ [OPEN]: () => ok(workspace) });
    await openWorkspace({ body: { workspace_id: WS }, throwOnError: true });
    expect(api.calls.map((call) => call.headers[CSRF_HEADER])).toEqual([TOKEN]);
  });

  it('is on every request the Explorer sends, reads included', async () => {
    browserHolds(`${CSRF_COOKIE}=${TOKEN}`);
    const api = mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await screen.findByRole('heading', { name: 'Sources' });
    const paths = new Set(api.calls.map((call) => call.path));
    expect([...paths].sort()).toEqual([
      '/api/attention/get-attention',
      '/api/items/get-item',
      '/api/items/list-drafts',
      '/api/items/list-items',
      '/api/reads/get-sources',
      '/api/reads/read-item',
      OPEN,
    ]);
    expect(api.calls.filter((call) => call.headers[CSRF_HEADER] !== TOKEN)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('sends the value exactly as the cookie holds it', async () => {
    browserHolds(`${CSRF_COOKIE}=a%41b`);
    const api = mockApi({ [OPEN]: () => ok(workspace) });
    await openWorkspace({ body: { workspace_id: WS }, throwOnError: true });
    expect(api.calls.at(0)?.headers[CSRF_HEADER]).toBe('a%41b');
  });

  it.each([
    ['no cookie at all', ''],
    ['only other cookies', 'theme=dark; lang=en'],
    ['the cookie emptied', `theme=dark; ${CSRF_COOKIE}=`],
    ['a cookie whose name only begins like it', `${CSRF_COOKIE}-old=${TOKEN}`],
    ['a cookie whose name only ends like it', `x-${CSRF_COOKIE}=${TOKEN}`],
    ['a nameless cookie that reads like the name', `${CSRF_COOKIE}x; theme=dark`],
  ])('is not sent with %s', async (_what, cookies) => {
    browserHolds(cookies);
    const api = mockApi({ [OPEN]: () => ok(workspace) });
    await openWorkspace({ body: { workspace_id: WS }, throwOnError: true });
    expect(api.calls).toHaveLength(1);
    expect(api.calls.at(0)?.headers).not.toHaveProperty(CSRF_HEADER);
  });

  it('is not sent on a GET, which the server accepts without it', async () => {
    browserHolds(`${CSRF_COOKIE}=${TOKEN}`);
    const api = mockApi({});
    await getFormSchema({ path: { name: 'type' } });
    expect(api.calls.map((call) => call.path)).toEqual(['/api/schemas/type']);
    expect(api.calls.at(0)?.headers).not.toHaveProperty(CSRF_HEADER);
  });

  it('is not sent to another origin', async () => {
    browserHolds(`${CSRF_COOKIE}=${TOKEN}`);
    const seen: Array<string | null> = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (input: Request) => {
        seen.push(input.headers.get(CSRF_HEADER));
        return new Response('{}', { headers: { 'Content-Type': 'application/json' } });
      }),
    );
    await sendRequest('https://elsewhere.example/api/items/save-draft', { method: 'POST' });
    await sendRequest(`${window.location.origin}/api/items/save-draft`, { method: 'POST' });
    expect(seen).toEqual([null, TOKEN]);
  });
});

/** A `fetch` whose answer the test gives, recording the signal each request was sent with. */
function heldFetch() {
  const signals: AbortSignal[] = [];
  const answers: Array<(response: Response) => void> = [];
  vi.stubGlobal(
    'fetch',
    vi.fn((_input: Request, init?: RequestInit) => {
      const signal = init?.signal;
      if (!(signal instanceof AbortSignal)) throw new Error('the request carries no signal');
      signals.push(signal);
      return new Promise<Response>((resolve, reject) => {
        answers.push(resolve);
        signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
      });
    }),
  );
  return { signals, answers };
}

/** What a promise came to, without waiting for it. */
function outcomeOf(promise: Promise<unknown>) {
  const outcome: { settled: boolean; value: unknown; error: unknown } = {
    settled: false,
    value: undefined,
    error: undefined,
  };
  promise.then(
    (value) => {
      outcome.settled = true;
      outcome.value = value;
    },
    (error: unknown) => {
      outcome.settled = true;
      outcome.error = error;
    },
  );
  return outcome;
}

const json = { 'Content-Type': 'application/json' };

/** A body that begins and never ends. */
function endlessBody(): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode('{"a":'));
    },
  });
}

describe('the bounded wait', () => {
  it('fails a request that never answers after the wait, not before, and aborts it', async () => {
    vi.useFakeTimers();
    const { signals } = heldFetch();
    const outcome = outcomeOf(sendRequest('/api/items/save-draft', { method: 'POST' }));
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    expect(outcome.settled).toBe(false);
    expect(signals.at(0)?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(outcome.error).toBeInstanceOf(RequestTimedOut);
    expect(signals.at(0)?.aborted).toBe(true);
  });

  it.each(['application/json', 'application/json; charset=utf-8', 'application/schema+json'])(
    'fails a JSON answer (%s) whose body never ends',
    async (type) => {
      vi.useFakeTimers();
      const { answers, signals } = heldFetch();
      const outcome = outcomeOf(sendRequest('/api/items/save-draft', { method: 'POST' }));
      answers.at(0)?.(new Response(endlessBody(), { headers: { 'Content-Type': type } }));
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
      expect(outcome.settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(outcome.error).toBeInstanceOf(RequestTimedOut);
      expect(signals.at(0)?.aborted).toBe(true);
    },
  );

  it('hands over an answer that is not JSON once it begins, and never cuts it', async () => {
    vi.useFakeTimers();
    const { answers, signals } = heldFetch();
    const outcome = outcomeOf(sendRequest('/api/workspaces/w/events', { method: 'GET' }));
    const stream = new Response(endlessBody(), {
      headers: { 'Content-Type': 'text/event-stream' },
    });
    answers.at(0)?.(stream);
    await vi.advanceTimersByTimeAsync(0);
    expect(outcome.value).toBe(stream);
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS * 2);
    expect(signals.at(0)?.aborted).toBe(false);
  });

  describe('allows for what is sent', () => {
    const post = (body: BodyInit, type: string) =>
      sendRequest('/api/items/save-draft', {
        method: 'POST',
        body,
        headers: { 'Content-Type': type },
      });

    it('waits one more second for each full 64 KiB of a JSON body, and no longer', async () => {
      vi.useFakeTimers();
      heldFetch();
      const outcome = outcomeOf(post('x'.repeat(2 * UPLOAD_BYTES_PER_SECOND), 'application/json'));
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS + 1_999);
      expect(outcome.settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(outcome.error).toBeInstanceOf(RequestTimedOut);
      expect(messageFor(classifyFailure(outcome.error)).advice).toBe(
        'No answer came within 32 seconds. Try again.',
      );
    });

    it('counts bytes, and gives a body under 64 KiB no allowance', async () => {
      vi.useFakeTimers();
      heldFetch();
      // One byte short of 64 KiB: exactly the bounded wait.
      const small = outcomeOf(post('x'.repeat(UPLOAD_BYTES_PER_SECOND - 1), 'application/json'));
      // Half as many characters, two bytes each: 64 KiB, so one more second.
      const wide = outcomeOf(post('é'.repeat(UPLOAD_BYTES_PER_SECOND / 2), 'application/json'));
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
      expect(small.settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(small.error).toBeInstanceOf(RequestTimedOut);
      expect(wide.settled).toBe(false);
      await vi.advanceTimersByTimeAsync(999);
      expect(wide.settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(wide.error).toBeInstanceOf(RequestTimedOut);
    });

    it('gives a body that is not JSON no allowance', async () => {
      vi.useFakeTimers();
      heldFetch();
      const outcome = outcomeOf(
        post('x'.repeat(4 * UPLOAD_BYTES_PER_SECOND), 'application/octet-stream'),
      );
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS);
      expect(outcome.error).toBeInstanceOf(RequestTimedOut);
    });
  });

  it('passes on an answer that came in time, and stops its clock', async () => {
    vi.useFakeTimers();
    const { answers, signals } = heldFetch();
    const outcome = outcomeOf(sendRequest('/api/items/save-draft', { method: 'POST' }));
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    answers.at(0)?.(new Response('{"saved":true}', { status: 409, headers: json }));
    await vi.advanceTimersByTimeAsync(0);
    const response = outcome.value;
    if (!(response instanceof Response)) throw new Error('no answer was passed on');
    expect(response.status).toBe(409);
    expect(await response.json()).toEqual({ saved: true });
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS);
    expect(signals.at(0)?.aborted).toBe(false);
  });

  it('does not call the caller’s own abort a timeout', async () => {
    vi.useFakeTimers();
    heldFetch();
    const caller = new AbortController();
    const outcome = outcomeOf(
      sendRequest('/api/items/get-item', { method: 'POST', signal: caller.signal }),
    );
    caller.abort();
    await vi.advanceTimersByTimeAsync(0);
    expect(outcome.settled).toBe(true);
    expect(outcome.error).not.toBeInstanceOf(RequestTimedOut);
    expect(outcome.error).toMatchObject({ name: 'AbortError' });
    expect(vi.getTimerCount()).toBe(0);
  });

  it('reads in plain words, can be retried, and is not repeated without the person', () => {
    const timedOut = new RequestTimedOut(REQUEST_TIMEOUT_MS);
    const failure = classifyFailure(timedOut);
    expect(failure).toEqual({ kind: 'timed_out', seconds: 30 });
    expect(messageFor(failure)).toEqual({
      title: 'The workspace service did not answer in time.',
      advice: 'No answer came within 30 seconds. Try again.',
      action: { kind: 'retry', label: 'Try again' },
    });
    expect(shouldRetry(0, timedOut)).toBe(false);
    // What it is told apart from: a service that was not reached is still tried once more.
    expect(shouldRetry(0, new TypeError('Failed to fetch'))).toBe(true);
  });
});

describe('through the app', () => {
  it('ends "Trying again…" when the retry never answers', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let answer: 'fail' | 'never' = 'fail';
    mockApi(
      explorerHandlers({
        '/api/items/get-item': () =>
          answer === 'fail'
            ? fail(500, 'internal', 'the store is unwell')
            : new Promise<never>(() => {}),
      }),
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    const retry = await screen.findByRole('button', { name: 'Try again' });
    answer = 'never';
    act(() => retry.focus());
    fireEvent.click(retry);
    await screen.findByRole('button', { name: 'Trying again…' });

    await act(() => vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1_000));
    expect(screen.getByRole('button', { name: 'Trying again…' })).toBeDefined();
    await act(() => vi.advanceTimersByTimeAsync(1_000));

    const again = await screen.findByRole('button', { name: 'Try again' });
    expect(screen.queryByRole('button', { name: 'Trying again…' })).toBeNull();
    expect(
      screen.getByText(
        /Reading this item’s properties: The workspace service did not answer in time\./,
      ),
    ).toBeDefined();
    await waitFor(() => expect(document.activeElement).toBe(again));
  });

  it('sends requests while the browser reports no network: the service may be on this computer', async () => {
    onlineManager.setOnline(false);
    const api = mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    expect(api.callsTo(OPEN)).toHaveLength(1);
  });

  it('sends what a person asks for while the browser reports no network (a mutation)', async () => {
    const api = mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const latest = await screen.findByRole('button', { name: 'Read the latest version' });
    const asked = () =>
      api
        .callsTo('/api/reads/read-item')
        .filter((body) => JSON.stringify(body.at).includes('latest'));
    expect(asked()).toHaveLength(0);
    onlineManager.setOnline(false);
    fireEvent.click(latest);
    await waitFor(() => expect(asked()).toHaveLength(1));
  });
});
