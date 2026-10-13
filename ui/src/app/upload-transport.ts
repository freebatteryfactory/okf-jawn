/**
 * The part of the transport that sends a file's bytes: one request whose progress can be seen,
 * bounded by that progress and not by a total time.
 *
 * `sendRequest` (`api-client.ts`) sends through `fetch`, which says nothing while a body is being
 * sent, and whose one bounded wait would cut a large file off after 30 seconds. So the generated
 * `uploadContent` is given `bytesSender` as its `fetch`: the generated client still builds the
 * request (the route, the method, the media type) and reads the answer, and the request is sent
 * with XMLHttpRequest, which reports how many bytes have gone.
 *
 * It is the same transport in what a request carries. The CSRF header follows the one rule of
 * `csrfHeaderFor`: the upload is cookie-authenticated like every other request. The caller's abort
 * ends the request as an abort.
 *
 * What bounds an upload. It may take as long as it keeps moving. It fails as `RequestTimedOut` when
 * no byte has been sent for `REQUEST_TIMEOUT_MS`, and when no answer has come within
 * `REQUEST_TIMEOUT_MS` of the last byte sent: the JSON wait's value, measured from the last
 * progress instead of from the start. A browser reports the last byte as progress too, so the
 * wait for the answer starts there. The answer is complete when the request settles, so the wait
 * covers the answer's body too.
 *
 * The bytes are handed to the browser as the Blob itself, which reads them as it sends them; they
 * are not copied into memory here. The Request the generated client built names the same Blob as
 * its body and is used for its route, method and headers only.
 */
import { CSRF_HEADER, csrfHeaderFor, REQUEST_TIMEOUT_MS, RequestTimedOut } from './api-client';

/** The answer of a settled request, as the generated client reads one. */
function answerOf(xhr: XMLHttpRequest): Response {
  const type = xhr.getResponseHeader('Content-Type');
  const bodiless = xhr.status === 204 || xhr.status === 205 || xhr.status === 304;
  return new Response(bodiless ? null : xhr.responseText, {
    status: xhr.status,
    statusText: xhr.statusText,
    headers: type === null ? {} : { 'Content-Type': type },
  });
}

/**
 * Send `bytes` as the body of `request`. `onProgress` is told how many bytes have gone, each time
 * the browser reports it. Resolves with the answer, whatever its status; rejects when the request
 * was aborted, stalled, or could not be sent.
 */
export function sendBytes(
  request: Request,
  bytes: Blob,
  onProgress: (sent: number) => void,
): Promise<Response> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open(request.method, request.url);
    const headers = new Headers(request.headers);
    const token = csrfHeaderFor(request.method, request.url);
    if (token !== null) headers.set(CSRF_HEADER, token);
    headers.forEach((value, name) => {
      xhr.setRequestHeader(name, value);
    });

    let timer: ReturnType<typeof setTimeout> | undefined;
    let settled = false;
    const settle = (outcome: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      request.signal.removeEventListener('abort', abort);
      outcome();
    };
    // Each sign of movement starts the wait again; an upload that stops moving is ended.
    const moved = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        // Settled before the abort, so the wait ends as a timeout and not as the abort it causes.
        settle(() => reject(new RequestTimedOut(REQUEST_TIMEOUT_MS)));
        xhr.abort();
      }, REQUEST_TIMEOUT_MS);
    };
    const aborted = () => reject(new DOMException('The upload was aborted', 'AbortError'));
    function abort() {
      settle(aborted);
      xhr.abort();
    }

    xhr.upload.onprogress = (event) => {
      moved();
      onProgress(event.loaded);
    };
    xhr.onload = () =>
      settle(() => {
        try {
          resolve(answerOf(xhr));
        } catch (error) {
          reject(error);
        }
      });
    xhr.onerror = () => settle(() => reject(new TypeError('The upload could not be sent')));
    xhr.onabort = () => settle(aborted);

    if (request.signal.aborted) {
      abort();
      return;
    }
    request.signal.addEventListener('abort', abort, { once: true });
    moved();
    xhr.send(bytes);
  });
}

/**
 * `sendBytes` as the generated client's `fetch` option for one upload. That option has the type of
 * the platform's `fetch`, which on some runtimes carries more than the call (see `api-client.ts`).
 */
export function bytesSender(bytes: Blob, onProgress: (sent: number) => void): typeof fetch {
  return Object.assign(
    (input: RequestInfo | URL, init?: RequestInit) =>
      sendBytes(
        input instanceof Request && init === undefined ? input : new Request(input, init),
        bytes,
        onProgress,
      ),
    globalThis.fetch,
  );
}
