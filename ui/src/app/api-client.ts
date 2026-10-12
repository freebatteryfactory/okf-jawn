/**
 * The one configuration of the generated client: what every request the workspace sends carries,
 * and how long it may take.
 *
 * CSRF (double submit). The server refuses a cookie-authenticated request whose method is not GET,
 * HEAD or OPTIONS unless its `x-okf-csrf` header holds the value of the `okf-csrf` cookie. Every
 * canonical operation is a POST, so reads need it as well as writes. The header is set here, on
 * each such request to this origin, when the cookie exists. Without the cookie nothing is added
 * and the server's own refusal is what the person sees. The token is not sent to another origin.
 * The two names are held here once; `tests/unit/api-client.test.ts` fails when either differs from
 * its security scheme in `api/openapi.json`.
 *
 * Bounded wait. A request that has not answered after `REQUEST_TIMEOUT_MS` is aborted and fails as
 * `RequestTimedOut`, which reads in plain words and can be retried like any other failure
 * (`features/errors/failure.ts`). For a JSON answer the wait covers the body too; for any other
 * answer (an event stream, a download) it covers the time until the answer begins, so a stream is
 * not cut short. A caller's own abort (a cancelled query) is not a timeout. A request that timed
 * out may still have been carried out: a write is repeated with the idempotency key it had.
 *
 * Limit: the wait is one value for every request, whatever the size of its body.
 */
import { client } from '../api/generated/client.gen';

/** The cookie the server sets at sign-in beside the session cookie (`csrfCookie`). */
export const CSRF_COOKIE = 'okf-csrf';
/** The header that sends the cookie's value back (`csrfHeader`). */
export const CSRF_HEADER = 'x-okf-csrf';

/**
 * How long a request may go unanswered: 30 seconds.
 *
 * Every operation is bounded by design (a read has a byte budget, a body is at most 16 MiB, long
 * work is a job that answers at once), so an answer normally takes well under a second locally
 * and a few seconds hosted. Thirty seconds leaves room for the largest body on an ordinary
 * connection, stays under the 60 seconds after which common reverse proxies give up (so these
 * words are shown, not a proxy's error page), and tells a person watching "Saving…" within half a
 * minute.
 */
export const REQUEST_TIMEOUT_MS = 30_000;

/** Methods the server accepts from a cookie session without the CSRF header. */
const SAFE_METHODS: ReadonlySet<string> = new Set(['GET', 'HEAD', 'OPTIONS']);

/** A request went unanswered for the whole bounded wait and was aborted. */
export class RequestTimedOut extends Error {
  readonly waitedMs: number;

  constructor(waitedMs: number) {
    super(`No answer within ${waitedMs / 1000} seconds`);
    this.name = 'RequestTimedOut';
    this.waitedMs = waitedMs;
  }
}

/** The CSRF cookie's value exactly as the browser holds it; null when there is none. */
export function csrfToken(): string | null {
  for (const pair of document.cookie.split(';')) {
    const cut = pair.indexOf('=');
    if (cut < 0 || pair.slice(0, cut).trim() !== CSRF_COOKIE) continue;
    const value = pair.slice(cut + 1).trim();
    return value.length > 0 ? value : null;
  }
  return null;
}

function needsCsrfHeader(request: Request): boolean {
  // A Request's method is already in upper case, and its URL absolute.
  if (SAFE_METHODS.has(request.method)) return false;
  return new URL(request.url).origin === window.location.origin;
}

function isJson(response: Response): boolean {
  const type = response.headers.get('Content-Type')?.split(';').at(0)?.trim() ?? '';
  return type === 'application/json' || type.endsWith('+json');
}

/** `fetch` for the generated client: the CSRF header, then the bounded wait. */
export async function sendRequest(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const request = input instanceof Request && init === undefined ? input : new Request(input, init);
  if (needsCsrfHeader(request)) {
    const token = csrfToken();
    if (token !== null) request.headers.set(CSRF_HEADER, token);
  }

  const deadline = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      // Rejected before the abort, so the wait ends as a timeout and not as the abort it causes.
      reject(new RequestTimedOut(REQUEST_TIMEOUT_MS));
      deadline.abort();
    }, REQUEST_TIMEOUT_MS);
  });
  try {
    const signal = AbortSignal.any([request.signal, deadline.signal]);
    const response = await Promise.race([globalThis.fetch(request, { signal }), timedOut]);
    if (!isJson(response)) return response;
    // Reading a copy to its end holds the whole body here, so the caller's own read of the answer
    // cannot stall after the wait is over.
    await Promise.race([response.clone().text(), timedOut]);
    return response;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * `sendRequest` as the client's `fetch` option. That option has the type of the platform's
 * `fetch`, which on some runtimes carries more than the call (Bun's has `preconnect`); whatever
 * the platform's carries when this module loads is kept beside the call.
 */
const transport = Object.assign(
  (input: RequestInfo | URL, init?: RequestInit) => sendRequest(input, init),
  globalThis.fetch,
);

/** Put this transport on the generated client. Calling it again changes nothing. */
export function configureApiClient(): void {
  client.setConfig({ fetch: transport });
}
