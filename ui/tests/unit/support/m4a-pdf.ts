/**
 * Ground for the unit tests of the PDF viewer and of the Office and other originals (UI M4a,
 * ruling A2): a source's appearance as the wire carries it, the original's bytes answered through
 * the transport's own fetch (as `byte-read.test.ts` answers them), and a stand-in for PDF.js.
 *
 * PDF.js does not run in happy-dom (no worker, no drawing on a canvas), so a test file replaces
 * the library with the stand-in below (`vi.mock('pdfjs-dist/legacy/build/pdf.mjs', …)`). It
 * records every `getDocument` call with its options and loading task, every page asked, every
 * render with its cancel, every text layer and every clean-up, so a test sees the lifecycle the
 * viewer drives; a test decides when a document opens, fails or draws. What PDF.js itself draws,
 * and where its worker and fonts come from, is proven in the browser (`tests/e2e/m4a-pdf.spec.ts`).
 */
import pdfjsPackage from 'pdfjs-dist/package.json';
import { vi } from 'vitest';
import type { SourceAppearance } from '../../../src/api/generated/types.gen';
import { zSourceAppearance } from '../../../src/api/generated/zod.gen';
import { configureApiClient } from '../../../src/app/api-client';

export const WS = '11111111-1111-4111-8111-111111111111';
export const ITEM = '44444444-4444-4444-8444-444444444444';
export const REVISION = 'a'.repeat(40);
export const OBJECT = 'd'.repeat(64);
export const OBJECT_PATH = `/api/workspaces/${WS}/items/${ITEM}/revisions/${REVISION}/objects/${OBJECT}`;

export const PDF = 'application/pdf';
export const DOCX = 'application/vnd.openxmlformats-officedocument.wordprocessingml.document';

/** A source's appearance, held to the generated schema. */
export function appearanceOf(
  mediaType: string,
  size: number,
  extra: Partial<SourceAppearance> = {},
): SourceAppearance {
  const appearance: SourceAppearance = {
    extraction: {
      corrected: false,
      outcome: { status: 'completed' },
      text_origin: 'converter',
      warnings: [],
    },
    media_type: mediaType,
    metadata: {},
    names: [
      {
        filename: 'annual-report.pdf',
        folder: 'reports',
        observed_at: '2026-01-01T09:00:00.000Z',
        supplied_by: 'ada',
      },
    ],
    object: OBJECT,
    size: String(size),
    ...extra,
  };
  zSourceAppearance.parse(appearance);
  return appearance;
}

// ---------------------------------------------------------------------------------------------
// The original's bytes, through the transport.

/** A byte read the page asked, as the stub saw it. */
export interface ByteRead {
  method: string;
  path: string;
  /** The `Range` header, or null for the whole object. */
  range: string | null;
}

/** How the stub answers a byte read: the bytes, a refusal, or not yet. */
export type ByteAnswer =
  | { kind: 'bytes' }
  | { kind: 'refused'; status: number; code: string; message: string }
  | { kind: 'held'; release: Promise<void> };

/**
 * Answer the object's byte reads with `bytes` (a `Range` with 206 and that part), and hand every
 * other request to the fetch already stubbed (`mockApi`'s, when a test made one). `answer` decides
 * each read in turn. The generated client is given the transport, as the app gives it.
 */
export function answerBytes(
  bytes: Uint8Array,
  answer: (read: ByteRead) => ByteAnswer = () => ({ kind: 'bytes' }),
): ByteRead[] {
  const reads: ByteRead[] = [];
  const before = globalThis.fetch;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const path = new URL(request.url).pathname;
      if (!path.includes('/objects/')) return before(input, init);
      const read = { method: request.method, path, range: request.headers.get('Range') };
      reads.push(read);
      const decided = answer(read);
      if (decided.kind === 'held') await decided.release;
      if (decided.kind === 'refused') {
        return new Response(JSON.stringify({ code: decided.code, message: decided.message }), {
          status: decided.status,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      const asked = /^bytes=(\d+)-(\d+)$/.exec(read.range ?? '');
      if (asked === null) {
        return new Response(bytes.slice(), {
          status: 200,
          headers: { 'Content-Type': 'application/pdf' },
        });
      }
      const start = Number(asked[1]);
      const end = Math.min(Number(asked[2]), bytes.length - 1);
      return new Response(bytes.slice(start, end + 1), {
        status: 206,
        headers: {
          'Content-Type': 'application/octet-stream',
          'Content-Range': `bytes ${start}-${end}/${bytes.length}`,
        },
      });
    }),
  );
  configureApiClient();
  return reads;
}

// ---------------------------------------------------------------------------------------------
// The stand-in for PDF.js.

/** A page of the stand-in: 600 x 800 at scale 1, with the text `textOf` gives it. */
export class FakePage {
  readonly pageNumber: number;
  readonly renders: FakeRenderTask[] = [];
  readonly cleanup = vi.fn(() => true);
  readonly render = vi.fn((_parameters: { canvas: HTMLCanvasElement | null }) => {
    const task = new FakeRenderTask(this.pageNumber);
    this.renders.push(task);
    if (!pdf.holdRenders) queueMicrotask(() => task.finish());
    return task;
  });
  readonly streamTextContent = vi.fn(() => {
    const items = pdf.textOf(this.pageNumber).map((str) => ({
      str,
      dir: 'ltr',
      transform: [1, 0, 0, 1, 0, 0],
      width: 10,
      height: 10,
      fontName: 'f1',
      hasEOL: false,
    }));
    return new ReadableStream({
      start(controller) {
        controller.enqueue({ items, styles: {}, lang: null });
        controller.close();
      },
    });
  });

  constructor(pageNumber: number) {
    this.pageNumber = pageNumber;
  }

  getViewport({ scale }: { scale: number }) {
    return {
      width: 600 * scale,
      height: 800 * scale,
      scale,
      userUnit: 1,
      rotation: 0,
      rawDims: { pageWidth: 600, pageHeight: 800, pageX: 0, pageY: 0 },
    };
  }
}

/** A render of a page: done when the stand-in finishes it, or cancelled. */
export class FakeRenderTask {
  readonly pageNumber: number;
  readonly promise: Promise<void>;
  state: 'drawing' | 'drawn' | 'cancelled' = 'drawing';
  readonly cancel = vi.fn(() => {
    if (this.state !== 'drawing') return;
    this.state = 'cancelled';
    this.#reject(
      Object.assign(new Error(`Rendering cancelled, page ${this.pageNumber}`), {
        name: 'RenderingCancelledException',
      }),
    );
  });
  #resolve: () => void = () => {};
  #reject: (error: Error) => void = () => {};

  constructor(pageNumber: number) {
    this.pageNumber = pageNumber;
    this.promise = new Promise<void>((resolve, reject) => {
      this.#resolve = resolve;
      this.#reject = reject;
    });
    this.promise.catch(() => undefined);
  }

  finish() {
    if (this.state !== 'drawing') return;
    this.state = 'drawn';
    this.#resolve();
  }
}

/** A document of the stand-in, of `pdf.numPages` pages. */
export class FakeDocument {
  readonly numPages: number;
  readonly getPage = vi.fn(async (pageNumber: number) => {
    const page = new FakePage(pageNumber);
    pdf.pages.push(page);
    return page;
  });

  constructor(numPages: number) {
    this.numPages = numPages;
  }
}

/** A loading task: opened, failed or destroyed. */
export class FakeTask {
  readonly options: Record<string, unknown>;
  readonly promise: Promise<FakeDocument>;
  destroyed = false;
  readonly destroy = vi.fn(async () => {
    this.destroyed = true;
    this.#reject(new Error('Loading aborted'));
  });
  #resolve: (document: FakeDocument) => void = () => {};
  #reject: (error: Error) => void = () => {};

  constructor(options: Record<string, unknown>) {
    this.options = options;
    this.promise = new Promise<FakeDocument>((resolve, reject) => {
      this.#resolve = resolve;
      this.#reject = reject;
    });
    this.promise.catch(() => undefined);
  }

  open(numPages = pdf.numPages) {
    if (!this.destroyed) this.#resolve(new FakeDocument(numPages));
  }

  fail(error: Error) {
    this.#reject(error);
  }
}

/** A text layer of the stand-in: it lays out the text it is given as spans in its container. */
export class TextLayer {
  static readonly cleanup = vi.fn();
  readonly container: HTMLElement;
  readonly cancel = vi.fn();
  readonly #source: ReadableStream<{ items: { str: string }[] }>;
  readonly #divs: HTMLElement[] = [];

  constructor({
    textContentSource,
    container,
  }: {
    textContentSource: ReadableStream<{ items: { str: string }[] }>;
    container: HTMLElement;
  }) {
    this.#source = textContentSource;
    this.container = container;
    pdf.textLayers.push(this);
  }

  readonly render = vi.fn(async () => {
    const reader = this.#source.getReader();
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      for (const item of value.items) {
        const span = document.createElement('span');
        span.textContent = item.str;
        this.#divs.push(span);
        this.container.append(span);
      }
    }
  });

  get textDivs(): HTMLElement[] {
    return this.#divs;
  }
}

/** The range transport PDF.js offers: it records the parts it was given. */
export class PDFDataRangeTransport {
  readonly length: number;
  readonly initialData: Uint8Array | null;
  readonly received: { begin: number; chunk: Uint8Array | null }[] = [];

  constructor(length: number, initialData: Uint8Array | null) {
    this.length = length;
    this.initialData = initialData;
  }

  onDataRange(begin: number, chunk: Uint8Array | null) {
    this.received.push({ begin, chunk });
  }

  requestDataRange(_begin: number, _end: number) {
    throw new Error('Abstract method PDFDataRangeTransport.requestDataRange');
  }

  abort() {}
}

export const GlobalWorkerOptions = { workerSrc: '' };

/** What the stand-in did, and how it behaves; `resetPdf` before each test. */
export const pdf = {
  /** Pages of each document opened. */
  numPages: 3,
  /** Documents are opened by the test (`task.open()`), not at once. */
  holdOpen: false,
  /** Renders are finished by the test (`render.finish()`), not at once. */
  holdRenders: false,
  /** Each opening fails with this. */
  failWith: null as Error | null,
  /** The text of each page. */
  textOf: (pageNumber: number): string[] => [`Text of page ${pageNumber}`],
  tasks: [] as FakeTask[],
  pages: [] as FakePage[],
  textLayers: [] as TextLayer[],
};

export function resetPdf() {
  pdf.numPages = 3;
  pdf.holdOpen = false;
  pdf.holdRenders = false;
  pdf.failWith = null;
  pdf.textOf = (pageNumber) => [`Text of page ${pageNumber}`];
  pdf.tasks = [];
  pdf.pages = [];
  pdf.textLayers = [];
  GlobalWorkerOptions.workerSrc = '';
  TextLayer.cleanup.mockClear();
}

export const getDocument = vi.fn((options: Record<string, unknown>) => {
  const task = new FakeTask(options);
  pdf.tasks.push(task);
  if (!pdf.holdOpen) {
    queueMicrotask(() => {
      if (pdf.failWith === null) task.open();
      else task.fail(pdf.failWith);
    });
  }
  return task;
});

/** A failure of PDF.js, by its name, as its exceptions carry one. */
export function pdfFailure(name: string, message: string): Error {
  return Object.assign(new Error(message), { name });
}

/** The version the stand-in reports: the installed one, as the real library does. */
export const version: string = pdfjsPackage.version;

/** The last loading task. */
export function lastTask(): FakeTask {
  const task = pdf.tasks.at(-1);
  if (task === undefined) throw new Error('no document was opened');
  return task;
}

/** The pages asked for page `pageNumber`, in order. */
export function pagesNumbered(pageNumber: number): FakePage[] {
  return pdf.pages.filter((page) => page.pageNumber === pageNumber);
}
