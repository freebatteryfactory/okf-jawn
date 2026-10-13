/**
 * A stand-in for XMLHttpRequest in unit tests of the byte upload. happy-dom's own would open a real
 * connection, and the mocked `fetch` never sees a request sent this way.
 *
 * It does what a browser's does for the parts the upload uses, in the same order, and no more
 * kindly: a header set before `open`, or a second `send`, throws as the real one does; a header set
 * twice is sent combined, as the real one sends it; the body must be the Blob itself; after an
 * abort or an answer the transfer is over and nothing more is reported. Events are delivered
 * through the `on…` properties only, which is all the upload assigns. The real engines are covered
 * by the browser specs.
 *
 * A test drives each transfer: it reports bytes gone, answers, or breaks the connection. A handler
 * that returns a reply answers by itself, after reporting every byte gone.
 */
import { vi } from 'vitest';
import { zUpload } from '../../../src/api/generated/zod.gen';
import type { Reply } from './api';

export interface Transfer {
  method: string;
  path: string;
  /** The request's headers, names in lower case. */
  headers: Record<string, string>;
  /** The body, as it was given to `send`. */
  bytes: Blob;
  /** The page aborted it. */
  aborted: boolean;
  /** The transfer is over: answered, broken or aborted. */
  over: boolean;
  /** The browser reports that `sent` bytes have gone. */
  progress: (sent: number) => void;
  /** Every byte has gone; the answer is awaited. */
  allSent: () => void;
  /** The service answers. */
  answer: (reply: Reply) => void;
  /** The connection breaks. */
  breakConnection: () => void;
}

/** What the service does with a transfer; a returned reply is its answer. */
export type TransferHandler = (
  transfer: Transfer,
) => Reply | Promise<Reply> | undefined | Promise<undefined>;

export interface MockedTransfers {
  transfers: Transfer[];
  /** An answer the contract does not allow; a test asserts this stays empty. */
  violations: string[];
}

export function mockTransfers(handler: TransferHandler = () => undefined): MockedTransfers {
  const transfers: Transfer[] = [];
  const violations: string[] = [];

  class FakeXhr {
    upload: {
      onprogress: ((event: { loaded: number; total: number }) => void) | null;
      onload: (() => void) | null;
    } = { onprogress: null, onload: null };
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    onabort: (() => void) | null = null;
    status = 0;
    statusText = '';
    responseText = '';
    #state: 'unsent' | 'opened' | 'sent' | 'done' = 'unsent';
    #method = '';
    #path = '';
    #headers = new Map<string, string>();
    #transfer: Transfer | null = null;

    open(method: string, url: string) {
      this.#method = method.toUpperCase();
      this.#path = new URL(url, window.location.href).pathname;
      this.#headers.clear();
      this.#state = 'opened';
    }

    setRequestHeader(name: string, value: string) {
      if (this.#state !== 'opened') {
        throw new DOMException('A header is set between open and send', 'InvalidStateError');
      }
      const key = name.toLowerCase();
      const held = this.#headers.get(key);
      this.#headers.set(key, held === undefined ? value : `${held}, ${value}`);
    }

    getResponseHeader(name: string): string | null {
      return this.#state === 'done' && this.status > 0 && name.toLowerCase() === 'content-type'
        ? 'application/json'
        : null;
    }

    send(body: unknown) {
      if (this.#state !== 'opened') {
        throw new DOMException('A request is sent once, after open', 'InvalidStateError');
      }
      if (!(body instanceof Blob)) throw new TypeError('The upload sends the Blob itself');
      this.#state = 'sent';
      const end = () => {
        this.#state = 'done';
        transfer.over = true;
      };
      const transfer: Transfer = {
        method: this.#method,
        path: this.#path,
        headers: Object.fromEntries(this.#headers),
        bytes: body,
        aborted: false,
        over: false,
        progress: (sent) => {
          if (!transfer.over) this.upload.onprogress?.({ loaded: sent, total: body.size });
        },
        allSent: () => {
          if (transfer.over) return;
          this.upload.onprogress?.({ loaded: body.size, total: body.size });
          this.upload.onload?.();
        },
        answer: (reply) => {
          if (transfer.over) return;
          if (reply.status === 200 && !zUpload.safeParse(reply.body).success) {
            violations.push(`${transfer.path} response is not an Upload`);
          }
          end();
          this.status = reply.status;
          this.responseText = JSON.stringify(reply.body);
          this.onload?.();
        },
        breakConnection: () => {
          if (transfer.over) return;
          end();
          this.onerror?.();
        },
      };
      this.#transfer = transfer;
      transfers.push(transfer);
      void Promise.resolve(handler(transfer)).then((reply) => {
        if (reply === undefined) return;
        transfer.allSent();
        transfer.answer(reply);
      });
    }

    abort() {
      const transfer = this.#transfer;
      if (this.#state !== 'sent' || transfer === null) return;
      this.#state = 'done';
      transfer.over = true;
      transfer.aborted = true;
      this.onabort?.();
    }
  }

  vi.stubGlobal('XMLHttpRequest', FakeXhr);
  return { transfers, violations };
}
