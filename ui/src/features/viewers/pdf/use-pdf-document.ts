/**
 * A PDF original, read and opened with PDF.js, and let go again (UI M4a, ruling A2).
 *
 * Reading. A PDF below `LARGE_PDF_BYTES` is read whole, once, through the byte read
 * (`app/byte-read.ts`: `download_object`, never `get_object`), as a query of its own: its key is
 * the generated one for that object at that exact revision, it is never stale (an object at a
 * revision does not change), and it is let go as soon as no viewer shows it (`gcTime: 0`), so a
 * closed viewer holds no copy. Its bytes are handed to PDF.js with `getDocument({ data })`. A
 * larger PDF is read in parts, each a ranged read as PDF.js asks for it (`pdfjs.ts`
 * `openParts`), so the first page is shown without waiting for the whole file, and each part
 * stays within the transport's time limit.
 *
 * Letting go. The document belongs to one effect run: when the viewer goes (its tab is closed),
 * or shows another object or revision, the run's loading task is destroyed, whether it had opened
 * or not. That destroys the document and its pages and ends its worker; the parts in flight are
 * stopped. A run whose read is still in flight when it ends never opens a document. Once the task
 * is gone, PDF.js's shared text-layer measuring is cleared (`TextLayer.cleanup`).
 *
 * Failing. A failed read is the transport's typed failure (`unread`), which can be tried again:
 * the whole read is asked again; for a PDF read in parts, the document that waited for the lost
 * part is let go and a new one opened. A file PDF.js cannot open (`unreadable`) is not tried
 * again: the same bytes would fail the same way.
 */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { downloadObjectQueryKey } from '../../../api/generated/@tanstack/react-query.gen';
import { readObjectBytes } from '../../../app/byte-read';
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from './pdfjs';

/** The library module, once loaded. */
export type PdfLibrary = typeof import('./pdfjs');

/** At this size and above, a PDF is read in parts. */
export const LARGE_PDF_BYTES = 4 * 1024 * 1024;

export interface PdfSource {
  workspaceId: string;
  itemId: string;
  revision: string;
  /** The object's SHA-256. */
  digest: string;
  /** Its size in bytes. */
  size: number;
}

export type PdfState =
  /** Its bytes are being read. */
  | { state: 'reading' }
  /** A read of its bytes failed. */
  | { state: 'unread'; error: unknown }
  /** PDF.js is opening it. */
  | { state: 'opening' }
  /** PDF.js could not open it. */
  | { state: 'unreadable'; error: unknown }
  /** Open: `key` names this document, for the parts of the page that follow it. */
  | { state: 'open'; key: string; document: PDFDocumentProxy; library: PdfLibrary };

/** What an effect run settled on. */
type Settled = Extract<PdfState, { state: 'unread' | 'unreadable' | 'open' }>;

export interface PdfDocument {
  shown: PdfState;
  /** It is read in parts. */
  inParts: boolean;
  /** What is asked, for `useRetry`: the object at its revision. */
  request: string;
  /** Read it again after a failed read; settles once that has answered. */
  again: () => Promise<unknown>;
}

/** Let a loading task go: its document, pages and worker; then the shared text-layer measuring. */
function release(task: PDFDocumentLoadingTask, library: PdfLibrary) {
  void task
    .destroy()
    .catch(() => undefined)
    .finally(() => library.TextLayer.cleanup());
}

export function usePdfDocument({
  workspaceId,
  itemId,
  revision,
  digest,
  size,
}: PdfSource): PdfDocument {
  const inParts = size >= LARGE_PDF_BYTES;
  const path = { workspace_id: workspaceId, item_id: itemId, revision, digest };
  const whole = useQuery({
    queryKey: [{ ...downloadObjectQueryKey({ path })[0], _read: 'whole' }],
    queryFn: ({ signal }) => readObjectBytes({ workspaceId, itemId, revision, digest, signal }),
    enabled: !inParts,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
  });
  const blob = whole.data;
  // A PDF read in parts is opened again after a lost part; a whole read, when its bytes come again.
  const [attempt, setAttempt] = useState(0);
  const request = `${workspaceId} ${itemId} ${revision} ${digest}`;
  const key = `${request} ${size} ${inParts ? `parts ${attempt}` : `whole ${whole.dataUpdatedAt}`}`;
  const [settled, setSettled] = useState<{ key: string; state: Settled } | null>(null);
  /** Retries waiting for the next run to settle. */
  const waiting = useRef<(() => void)[]>([]);

  useEffect(() => {
    if (!inParts && blob === undefined) return;
    let over = false;
    let task: PDFDocumentLoadingTask | null = null;
    let library: PdfLibrary | null = null;
    const stop = new AbortController();
    const settle = (state: Settled) => {
      if (over) return;
      setSettled({ key, state });
      for (const resolve of waiting.current.splice(0)) resolve();
    };
    const end = () => {
      over = true;
      stop.abort();
      if (task !== null && library !== null) release(task, library);
      task = null;
    };
    void (async () => {
      try {
        const loaded = await import('./pdfjs');
        const data = blob === undefined ? null : new Uint8Array(await blob.arrayBuffer());
        if (over) return;
        library = loaded;
        task =
          data === null
            ? loaded.openParts(
                size,
                (range, signal) =>
                  readObjectBytes({
                    workspaceId,
                    itemId,
                    revision,
                    digest,
                    range,
                    signal: AbortSignal.any([signal, stop.signal]),
                  }),
                (error) => {
                  // A part that never comes: PDF.js would wait for it forever, so this document is
                  // let go and the failure said.
                  settle({ state: 'unread', error });
                  end();
                },
              )
            : loaded.openBytes(data);
        const document = await task.promise;
        settle({ state: 'open', key, document, library: loaded });
      } catch (error) {
        settle({ state: 'unreadable', error });
      }
    })();
    return end;
  }, [inParts, blob, key, size, workspaceId, itemId, revision, digest]);

  const mine = settled !== null && settled.key === key ? settled.state : null;
  const shown: PdfState =
    !inParts && whole.isError
      ? { state: 'unread', error: whole.error }
      : !inParts && blob === undefined
        ? { state: 'reading' }
        : (mine ?? { state: 'opening' });
  const again = (): Promise<unknown> => {
    if (!inParts) return whole.refetch();
    return new Promise<void>((resolve) => {
      waiting.current.push(resolve);
      setAttempt((count) => count + 1);
    });
  };
  return { shown, inParts, request, again };
}
