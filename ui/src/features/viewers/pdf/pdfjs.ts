/**
 * PDF.js as the PDF viewer uses it (UI M4a, ruling A2): the one module that imports the library.
 * The viewer loads it only when it shows a PDF (`import('./pdfjs')`), so no other page carries it.
 *
 * Which build. The legacy build (`pdfjs-dist/legacy/build/pdf.mjs`), and its worker: the desktop
 * launcher shows the workspace in the system WebKit (SPEC section 2), which can be older than the
 * modern build needs (it calls `Map.prototype.getOrInsertComputed` and `Math.sumPrecise`).
 *
 * Where its files come from. `vite.config.ts` puts the installed distribution's worker, standard
 * fonts, CMaps and image-decoder fallbacks in the build under `pdfjs-<version>/`; they are named
 * here from the library's own `version`, as absolute URLs of the page's origin. What the page
 * policy (`crates/server/src/assets.rs` `page_headers`) allows, and how PDF.js keeps to it:
 * - the worker is a module worker started from its own file of this origin (`worker-src 'self'`).
 *   PDF.js starts one for each document (`GlobalWorkerOptions.workerSrc`) and ends it when that
 *   document's loading task is destroyed. Only a worker of another origin would be wrapped in a
 *   `blob:` script, which the policy refuses; this one never is.
 * - the worker fetches the standard fonts and the CMaps from this origin (`connect-src 'self'`).
 *   `useSystemFonts: false`: a font the PDF does not embed is always the distribution's, so a page
 *   is drawn the same on every computer and nothing is looked up among the system's fonts.
 * - a font the PDF embeds is loaded from its bytes with the Font Loading API (`FontFace`), which
 *   no `font-src` governs (a browser without that API would get `data:` fonts in a `<style>`,
 *   which `font-src data:` and `style-src 'unsafe-inline'` allow).
 * - no WebAssembly (`useWasm: false`): compiling it needs `'wasm-unsafe-eval'`, which the policy
 *   does not grant. The script fallbacks of the image decoders (`wasm/*_nowasm_fallback.js`, a
 *   module import of this origin, `script-src 'self'`) decode JPEG 2000 and JBIG2 images instead,
 *   and PostScript functions are interpreted. ICC colour profiles have no fallback: their colours
 *   are drawn in the profile's alternate colour space.
 * - nothing is evaluated from text: this version of PDF.js has no `eval` path left.
 * - the text layer places its spans with style properties set from script (CSSOM), which needs no
 *   policy at all.
 * - XFA forms are off, and no annotation layer or scripting is built: annotations are drawn into
 *   the page as their appearance, and a PDF's own JavaScript never runs.
 *
 * Reading in parts. A small PDF is read whole and handed over as `data`. A large one is read as
 * PDF.js asks for its parts: `ObjectParts` is the library's range transport, each part one
 * ranged read through `download_object`, with the transport's time limit for each part rather
 * than for the whole file. Nothing is fetched ahead of what the pages shown need
 * (`disableAutoFetch`, with `disableStream`, as the library requires for it). A part that fails is
 * reported to the viewer, which lets that document go: PDF.js would otherwise wait for it forever.
 */
import {
  GlobalWorkerOptions,
  getDocument,
  PDFDataRangeTransport,
  type PDFDocumentLoadingTask,
  TextLayer,
  version,
} from 'pdfjs-dist/legacy/build/pdf.mjs';
import type { ByteRange } from '../../../app/byte-read';

export type {
  PDFDocumentLoadingTask,
  PDFDocumentProxy,
  PDFPageProxy,
  RenderTask,
} from 'pdfjs-dist/legacy/build/pdf.mjs';
export { TextLayer };

/** Bytes PDF.js asks for in one part of a large PDF. */
export const PART_BYTES = 512 * 1024;

/** Where this build serves PDF.js's own files, for the version of the library that runs. */
function filesOfThisBuild(): string {
  return new URL(`${import.meta.env.BASE_URL}pdfjs-${version}/`, document.baseURI).href;
}

/** What every document is opened with (see the header). */
function options() {
  const files = filesOfThisBuild();
  // Set where it is used: the worker each document starts is this build's.
  GlobalWorkerOptions.workerSrc = `${files}pdf.worker.min.js`;
  return {
    standardFontDataUrl: `${files}standard_fonts/`,
    cMapUrl: `${files}cmaps/`,
    cMapPacked: true,
    wasmUrl: `${files}wasm/`,
    useWasm: false,
    useSystemFonts: false,
    enableXfa: false,
  } satisfies Parameters<typeof getDocument>[0];
}

/** Open a PDF from all its bytes. PDF.js takes the buffer over (it is moved to the worker). */
export function openBytes(data: Uint8Array): PDFDocumentLoadingTask {
  return getDocument({ ...options(), data });
}

/** Read one part of the original. */
export type ReadPart = (range: ByteRange, signal: AbortSignal) => Promise<Blob>;

/** The service answered a part with another number of bytes than were asked. */
export class PartMismatch extends Error {
  readonly asked: number;
  readonly answered: number;

  constructor(asked: number, answered: number) {
    super(`The service answered ${answered} bytes for a part of ${asked}.`);
    this.name = 'PartMismatch';
    this.asked = asked;
    this.answered = answered;
  }
}

/** The library's range transport over ranged reads of the original (see the header). */
class ObjectParts extends PDFDataRangeTransport {
  readonly #read: ReadPart;
  readonly #failed: (error: unknown) => void;
  readonly #stop = new AbortController();

  constructor(length: number, read: ReadPart, failed: (error: unknown) => void) {
    super(length, null);
    this.#read = read;
    this.#failed = failed;
  }

  override requestDataRange(begin: number, end: number): void {
    const signal = this.#stop.signal;
    const length = Math.min(end, this.length) - begin;
    this.#read({ offset: begin, length }, signal)
      .then((part) => part.arrayBuffer())
      .then((buffer) => {
        if (signal.aborted) return;
        if (buffer.byteLength !== length) throw new PartMismatch(length, buffer.byteLength);
        this.onDataRange(begin, new Uint8Array(buffer));
      })
      .catch((error: unknown) => {
        if (!signal.aborted) this.#failed(error);
      });
  }

  /** PDF.js is done with the document: the reads in flight are stopped. */
  override abort(): void {
    this.#stop.abort();
  }
}

/** Open a large PDF of `length` bytes, read in parts as PDF.js asks; `failed` hears a lost part. */
export function openParts(
  length: number,
  read: ReadPart,
  failed: (error: unknown) => void,
): PDFDocumentLoadingTask {
  return getDocument({
    ...options(),
    range: new ObjectParts(length, read, failed),
    rangeChunkSize: PART_BYTES,
    disableAutoFetch: true,
    disableStream: true,
  });
}

/** A failure of PDF.js, by the name its exceptions carry. */
export function pdfjsFailure(error: unknown): string | null {
  return error instanceof Error ? error.name : null;
}
