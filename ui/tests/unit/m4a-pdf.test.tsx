/**
 * The PDF viewer (UI M4a, ruling A2; SPEC section 7: the human viewer includes PDF): the original's
 * bytes are read through `download_object` (never `get_object`) and handed to PDF.js with
 * `getDocument({ data })`, or, for a large PDF, read in parts as PDF.js asks for them (a range
 * transport over the same byte read). PDF.js gets its worker, standard fonts and CMaps from this
 * origin, from the installed version, with WebAssembly off (the page policy grants no
 * `'wasm-unsafe-eval'`). The pages are drawn one at a time with their text layer, and paged
 * through with buttons and a labelled number field; a `pages` selection opens at its first page
 * and is said. Every page asked is released when it is left, and the loading task (with the
 * document and its worker) is destroyed when the viewer goes, whether it had opened or not.
 *
 * PDF.js is the stand-in of `support/m4a-pdf.ts`; the browser journey draws the real one.
 */
import { QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, configure, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Selection, SourceAppearance } from '../../src/api/generated/types.gen';
import { createQueryClient } from '../../src/app/query-client';
import { PdfViewer } from '../../src/features/viewers/pdf/PdfViewer';
import {
  answerBytes,
  appearanceOf,
  type ByteAnswer,
  type ByteRead,
  GlobalWorkerOptions,
  getDocument,
  ITEM,
  lastTask,
  OBJECT_PATH,
  PDF,
  PDFDataRangeTransport,
  pagesNumbered,
  pdf,
  pdfFailure,
  REVISION,
  resetPdf,
  TextLayer,
  version,
  WS,
} from './support/m4a-pdf';

vi.mock('pdfjs-dist/legacy/build/pdf.mjs', () => import('./support/m4a-pdf'));

// Below the test's own time limit, so a wait that fails says what it waited for.
configure({ asyncUtilTimeout: 4_000 });

const BYTES = new TextEncoder().encode('%PDF-1.7\n% a stand-in for a small PDF\n%%EOF\n');
/** At this size and above, the viewer reads the original in parts. */
const LARGE = 5 * 1024 * 1024;

beforeEach(() => {
  resetPdf();
  getDocument.mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function renderViewer({
  appearance = appearanceOf(PDF, BYTES.length),
  selection = { kind: 'all' },
}: {
  appearance?: SourceAppearance;
  selection?: Selection;
} = {}) {
  const queryClient = createQueryClient();
  const view = render(
    <QueryClientProvider client={queryClient}>
      <PdfViewer
        workspaceId={WS}
        itemId={ITEM}
        revision={REVISION}
        path="reports/annual-report.pdf.md"
        appearance={appearance}
        selection={selection}
      />
    </QueryClientProvider>,
  );
  return { queryClient, ...view };
}

/** The page drawn now, by its text alternative. */
const pageImage = (name: RegExp | string) => screen.findByRole('img', { name });
const status = () => screen.getByRole('status');
const button = (name: string) => screen.getByRole('button', { name });

/** Wait until page `pageNumber` is drawn with its text. */
async function drawn(pageNumber: number, of = 3) {
  await pageImage(new RegExp(`^Page ${pageNumber} of ${of} of annual-report\\.pdf\\. Its text`));
  await waitFor(() => expect(status().textContent).toContain(`Page ${pageNumber} of ${of}`));
}

describe('the bytes of a PDF', () => {
  it('are read once through download_object, whole, and handed to getDocument as data', async () => {
    const reads = answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    expect(reads).toEqual<ByteRead[]>([{ method: 'GET', path: OBJECT_PATH, range: null }]);
    expect(getDocument).toHaveBeenCalledTimes(1);
    const { options } = lastTask();
    expect(options.data).toBeInstanceOf(Uint8Array);
    expect(Array.from(options.data as Uint8Array)).toEqual(Array.from(BYTES));
    expect(options.range).toBeUndefined();
    expect(options.url).toBeUndefined();
  });

  it('give PDF.js its worker, fonts and CMaps from this origin, for its own version, without WebAssembly', async () => {
    answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    const files = new URL(`/pdfjs-${version}/`, window.location.href).href;
    expect(GlobalWorkerOptions.workerSrc).toBe(`${files}pdf.worker.min.js`);
    const { options } = lastTask();
    expect(options).toMatchObject({
      standardFontDataUrl: `${files}standard_fonts/`,
      cMapUrl: `${files}cmaps/`,
      cMapPacked: true,
      wasmUrl: `${files}wasm/`,
      useWasm: false,
      useSystemFonts: false,
      enableXfa: false,
    });
    // Nothing PDF.js is given names another origin.
    for (const value of Object.values(options)) {
      if (typeof value === 'string') {
        expect(new URL(value).origin).toBe(window.location.origin);
      }
    }
  });

  it('are read in parts for a large PDF: a range transport over ranged reads, nothing more', async () => {
    const big = new Uint8Array(LARGE);
    big.set(BYTES);
    const reads = answerBytes(big);
    renderViewer({ appearance: appearanceOf(PDF, LARGE) });
    await waitFor(() => expect(getDocument).toHaveBeenCalledTimes(1));
    const { options } = lastTask();
    expect(options.data).toBeUndefined();
    const range = options.range;
    expect(range).toBeInstanceOf(PDFDataRangeTransport);
    if (!(range instanceof PDFDataRangeTransport)) throw new Error('no range transport');
    expect(range.length).toBe(LARGE);
    expect(options).toMatchObject({ disableAutoFetch: true, disableStream: true });
    // No read before PDF.js asks.
    expect(reads).toEqual([]);
    range.requestDataRange(LARGE - 1000, LARGE);
    await waitFor(() => expect(range.received).toHaveLength(1));
    expect(reads).toEqual<ByteRead[]>([
      { method: 'GET', path: OBJECT_PATH, range: `bytes=${LARGE - 1000}-${LARGE - 1}` },
    ]);
    expect(range.received[0]?.begin).toBe(LARGE - 1000);
    expect(range.received[0]?.chunk?.length).toBe(1000);
    await drawn(1);
    expect(
      document.body.contains(screen.getByText(/read in parts, as the pages shown need them/)),
    ).toBe(true);
  });

  it('say a refused read in the service’s words, and read again on Try again', async () => {
    let refuse = true;
    const reads = answerBytes(
      BYTES,
      (): ByteAnswer =>
        refuse
          ? { kind: 'refused', status: 503, code: 'unavailable', message: 'The store is busy' }
          : { kind: 'bytes' },
    );
    renderViewer();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/^Reading the original:/);
    expect(alert.textContent).toContain('The service said: The store is busy');
    expect(getDocument).not.toHaveBeenCalled();
    refuse = false;
    await userEvent.click(await screen.findByRole('button', { name: 'Try again' }));
    await drawn(1);
    expect(reads.length).toBeGreaterThanOrEqual(2);
  });

  it('say a failed part of a large PDF in words, and open it again on Try again', async () => {
    let refuse = true;
    answerBytes(
      new Uint8Array(LARGE),
      (): ByteAnswer =>
        refuse
          ? { kind: 'refused', status: 503, code: 'unavailable', message: 'The store is busy' }
          : { kind: 'bytes' },
    );
    pdf.holdOpen = true;
    renderViewer({ appearance: appearanceOf(PDF, LARGE) });
    await waitFor(() => expect(getDocument).toHaveBeenCalledTimes(1));
    const first = lastTask();
    const range = first.options.range;
    if (!(range instanceof PDFDataRangeTransport)) throw new Error('no range transport');
    range.requestDataRange(0, 1000);
    expect((await screen.findByRole('alert')).textContent).toMatch(/^Reading the original:/);
    // The document that waited for that part is let go.
    await waitFor(() => expect(first.destroy).toHaveBeenCalled());
    refuse = false;
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(getDocument).toHaveBeenCalledTimes(2));
    lastTask().open();
    await drawn(1);
  });
});

describe('a file PDF.js cannot open', () => {
  it('is said in PDF.js’s words, with the original offered as a download', async () => {
    answerBytes(BYTES);
    pdf.failWith = pdfFailure('InvalidPDFException', 'Invalid PDF structure.');
    renderViewer();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain(
      'Opening the PDF: PDF.js could not read this file as a PDF.',
    );
    expect(alert.textContent).toContain('PDF.js said: Invalid PDF structure.');
    const link = screen.getByRole('link', { name: 'Download the original, annual-report.pdf' });
    expect(link.getAttribute('href')).toBe(OBJECT_PATH);
    expect(link.getAttribute('download')).toBe('annual-report.pdf');
    expect(screen.queryByRole('button', { name: 'Next page' })).toBeNull();
  });

  it('says a password-protected PDF is not shown, and why', async () => {
    answerBytes(BYTES);
    pdf.failWith = pdfFailure('PasswordException', 'No password given');
    renderViewer();
    expect((await screen.findByRole('alert')).textContent).toContain(
      'Opening the PDF: this PDF is protected by a password, which this viewer does not ask for.',
    );
    expect(
      document.body.contains(screen.getByRole('link', { name: /^Download the original/ })),
    ).toBe(true);
  });
});

describe('paging through a PDF', () => {
  it('draws one page at a time, with its text layer and a text alternative', async () => {
    answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    const image = screen.getByRole('img', { name: /^Page 1 of 3/ });
    expect(image.tagName).toBe('CANVAS');
    expect(pagesNumbered(1).at(0)?.render).toHaveBeenCalledWith(
      expect.objectContaining({ canvas: image }),
    );
    expect(document.body.contains(screen.getByText('Text of page 1'))).toBe(true);
    expect(pdf.pages.map((page) => page.pageNumber)).toEqual([1]);
  });

  it('says a page with no text of its own', async () => {
    answerBytes(BYTES);
    pdf.textOf = () => [];
    renderViewer();
    expect(
      await pageImage(
        'Page 1 of 3 of annual-report.pdf. This page has no text of its own in the PDF.',
      ),
    ).toBeTruthy();
  });

  it('goes to the next and the previous page with buttons that keep focus at either end', async () => {
    answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    const previous = button('Previous page');
    expect(previous.getAttribute('aria-disabled')).toBe('true');
    expect(previous.hasAttribute('disabled')).toBe(false);
    await userEvent.click(previous);
    expect(status().textContent).toContain('Page 1 of 3');

    await userEvent.click(button('Next page'));
    await drawn(2);
    await userEvent.click(button('Next page'));
    await drawn(3);
    const next = button('Next page');
    expect(next.getAttribute('aria-disabled')).toBe('true');
    next.focus();
    await userEvent.keyboard('{Enter}');
    expect(document.activeElement).toBe(next);
    expect(status().textContent).toContain('Page 3 of 3');
    expect(button('Previous page').getAttribute('aria-disabled')).toBe('false');

    await userEvent.click(button('Previous page'));
    await drawn(2);
    expect(pdf.pages.map((page) => page.pageNumber)).toEqual([1, 2, 3, 2]);
  });

  it('goes to a page typed in the labelled number field, and says a page it does not have', async () => {
    answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    const field = screen.getByRole('spinbutton', { name: 'Page number' });
    expect(field.getAttribute('min')).toBe('1');
    expect(field.getAttribute('max')).toBe('3');
    await userEvent.clear(field);
    await userEvent.type(field, '3{Enter}');
    await drawn(3);
    expect(document.activeElement).toBe(field);

    await userEvent.clear(field);
    await userEvent.type(field, '9');
    await userEvent.click(button('Go to page'));
    expect((await screen.findByRole('alert')).textContent).toContain(
      'There is no page 9: this PDF has 3 pages.',
    );
    expect(status().textContent).toContain('Page 3 of 3');
    expect(field.getAttribute('aria-invalid')).toBe('true');
  });

  it('releases each page it leaves: a drawing in progress is cancelled, and the page cleaned up', async () => {
    answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    pdf.holdRenders = true;
    await userEvent.click(button('Next page'));
    await waitFor(() => expect(pagesNumbered(2).at(0)?.render).toHaveBeenCalled());
    const second = pagesNumbered(2).at(0);
    await userEvent.click(button('Next page'));
    await waitFor(() => expect(pagesNumbered(3).at(0)?.render).toHaveBeenCalled());
    expect(second?.renders.at(0)?.cancel).toHaveBeenCalled();
    expect(second?.cleanup).toHaveBeenCalled();
    expect(pagesNumbered(1).at(0)?.cleanup).toHaveBeenCalled();
    // The cancelled drawing is not reported as a failure.
    expect(screen.queryByRole('alert')).toBeNull();
    const third = pagesNumbered(3).at(0);
    act(() => third?.renders.at(0)?.finish());
    await drawn(3);
  });
});

describe('a selection of pages', () => {
  it('opens at its first page, and says the range', async () => {
    answerBytes(BYTES);
    renderViewer({ selection: { kind: 'pages', range: { start: 2, end: 3 } } });
    await drawn(2);
    expect(
      document.body.contains(
        screen.getByText('The reader shows pages 2–3: the PDF opened at page 2.'),
      ),
    ).toBe(true);
    expect(pdf.pages.map((page) => page.pageNumber)).toEqual([2]);
  });

  it('opens at the page of a region', async () => {
    answerBytes(BYTES);
    renderViewer({
      selection: {
        kind: 'region',
        region: {
          page_no: 3,
          bbox: { l: 0, t: 0, r: 10, b: 10, coord_origin: 'top_left' },
          page_size: { width: 600, height: 800 },
        },
      },
    });
    await drawn(3);
    expect(
      screen.getByText('The reader shows a region on page 3: the PDF opened at page 3.'),
    ).toBeTruthy();
  });

  it('beyond the PDF’s pages, opens at page 1 and says so', async () => {
    answerBytes(BYTES);
    renderViewer({ selection: { kind: 'pages', range: { start: 7, end: 8 } } });
    await drawn(1);
    expect(
      screen.getByText(
        'The reader shows pages 7–8, but this PDF has 3 pages: it opened at page 1.',
      ),
    ).toBeTruthy();
  });

  it('of lines, opens at page 1 and says that pages do not show lines', async () => {
    answerBytes(BYTES);
    renderViewer({ selection: { kind: 'lines', range: { start: 3, end: 9 } } });
    await drawn(1);
    expect(
      screen.getByText(
        'The reader shows lines 3–9 of the text, which a PDF’s pages do not mark: it opened at page 1.',
      ),
    ).toBeTruthy();
  });

  it('opens again at the first page of another selection the reader shows, in the same document', async () => {
    answerBytes(BYTES);
    const { rerender, queryClient } = renderViewer();
    await drawn(1);
    rerender(
      <QueryClientProvider client={queryClient}>
        <PdfViewer
          workspaceId={WS}
          itemId={ITEM}
          revision={REVISION}
          path="reports/annual-report.pdf.md"
          appearance={appearanceOf(PDF, BYTES.length)}
          selection={{ kind: 'pages', range: { start: 3, end: 3 } }}
        />
      </QueryClientProvider>,
    );
    await drawn(3);
    expect(screen.getByText('The reader shows page 3: the PDF opened at page 3.')).toBeTruthy();
    // The same document: nothing was read or opened again.
    expect(getDocument).toHaveBeenCalledTimes(1);
  });
});

describe('letting the PDF go', () => {
  it('destroys the loading task, cancels the drawing and cleans up the page when the viewer is closed', async () => {
    answerBytes(BYTES);
    const { unmount } = renderViewer();
    await drawn(1);
    pdf.holdRenders = true;
    await userEvent.click(button('Next page'));
    await waitFor(() => expect(pagesNumbered(2).at(0)?.render).toHaveBeenCalled());
    const task = lastTask();
    const page = pagesNumbered(2).at(0);
    const layer = pdf.textLayers.at(-1);
    unmount();
    expect(task.destroy).toHaveBeenCalledTimes(1);
    expect(page?.renders.at(0)?.cancel).toHaveBeenCalled();
    expect(page?.cleanup).toHaveBeenCalled();
    expect(layer?.cancel).toHaveBeenCalled();
    // Once the task is gone, the shared text-layer measuring is cleared.
    await waitFor(() => expect(TextLayer.cleanup).toHaveBeenCalled());
  });

  it('destroys a loading task that has not opened yet, and draws nothing after', async () => {
    answerBytes(BYTES);
    pdf.holdOpen = true;
    const { unmount } = renderViewer();
    await waitFor(() => expect(getDocument).toHaveBeenCalledTimes(1));
    const task = lastTask();
    unmount();
    expect(task.destroy).toHaveBeenCalledTimes(1);
    task.open();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(pdf.pages).toEqual([]);
  });

  it('never opens a document whose read is still in flight when the viewer is closed', async () => {
    let release: () => void = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    answerBytes(BYTES, () => ({ kind: 'held', release: held }));
    const { unmount } = renderViewer();
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/Reading the original/),
    );
    unmount();
    release();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(getDocument).not.toHaveBeenCalled();
  });

  it('lets the old document go when another revision is shown', async () => {
    answerBytes(BYTES);
    const { rerender, queryClient } = renderViewer();
    await drawn(1);
    const first = lastTask();
    rerender(
      <QueryClientProvider client={queryClient}>
        <PdfViewer
          workspaceId={WS}
          itemId={ITEM}
          revision={'b'.repeat(40)}
          path="reports/annual-report.pdf.md"
          appearance={appearanceOf(PDF, BYTES.length)}
          selection={{ kind: 'all' }}
        />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(getDocument).toHaveBeenCalledTimes(2));
    expect(first.destroy).toHaveBeenCalled();
    await drawn(1);
  });
});

describe('the controls', () => {
  it('are buttons and a labelled number field in a named group, reached with Tab', async () => {
    answerBytes(BYTES);
    renderViewer();
    await drawn(1);
    const group = screen.getByRole('group', { name: 'Pages of the PDF' });
    const order = [
      within(group).getByRole('button', { name: 'Previous page' }),
      within(group).getByRole('button', { name: 'Next page' }),
      within(group).getByRole('spinbutton', { name: 'Page number' }),
      within(group).getByRole('button', { name: 'Go to page' }),
    ];
    await userEvent.tab();
    for (const control of order) {
      expect(document.activeElement).toBe(control);
      await userEvent.tab();
    }
  });
});
