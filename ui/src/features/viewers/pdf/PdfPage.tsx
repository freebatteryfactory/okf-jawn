/**
 * One page of a PDF, drawn by PDF.js on a canvas with its text layer over it (UI M4a, ruling A2).
 *
 * The page fits the width it is given (never wider than at 150 %), at the screen's pixel ratio,
 * kept under the largest canvas a browser draws. The canvas is one image to assistive technology,
 * named by its page and the file; the text layer holds the page's own text, transparent over the
 * drawing, to select, to find and to read aloud. A page with no text of its own (a scan) says so in
 * that name.
 *
 * Letting go. A page belongs to one effect run: when another page is shown, the width changes or
 * the viewer goes, its drawing in progress is cancelled, its text layer is cancelled and emptied,
 * and the page is cleaned up (PDF.js frees its resources once no drawing holds them). A cancelled
 * drawing is not a failure; a drawing that failed is said in PDF.js's words.
 */
import { useEffect, useRef, useState } from 'react';
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from './pdfjs';
import './text-layer.css';
import { useElementWidth } from './use-element-width';
import type { PdfLibrary } from './use-pdf-document';

/** The largest scale a page is drawn at, however wide its place. */
const MOST_SCALE = 1.5;
/** The scale of a page whose place has not been measured. */
const UNMEASURED_SCALE = 1;
/** Canvas pixels a browser reliably draws (Safari's limit is 16,777,216). */
const MOST_PIXELS = 16_777_216;

export interface PdfPageProps {
  document: PDFDocumentProxy;
  library: PdfLibrary;
  pageNumber: number;
  /** Pages in the document. */
  count: number;
  /** The original's file name, for the page's text alternative. */
  name: string;
}

type Drawn = { key: string; hasText: boolean } | { key: string; error: unknown };

/** A cancelled drawing or text layer is not a failure. */
function cancelled(error: unknown): boolean {
  return (
    error instanceof Error &&
    (error.name === 'RenderingCancelledException' || error.name === 'AbortException')
  );
}

export function PdfPage({ document, library, pageNumber, count, name }: PdfPageProps) {
  const place = useRef<HTMLDivElement | null>(null);
  const sheetRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const layerRef = useRef<HTMLDivElement | null>(null);
  const width = useElementWidth(place);
  const key = `${pageNumber} ${width}`;
  const [drawn, setDrawn] = useState<Drawn | null>(null);

  useEffect(() => {
    const sheet = sheetRef.current;
    const canvas = canvasRef.current;
    const layer = layerRef.current;
    if (sheet === null || canvas === null || layer === null) return;
    let over = false;
    let page: PDFPageProxy | null = null;
    let drawing: RenderTask | null = null;
    let text: InstanceType<PdfLibrary['TextLayer']> | null = null;
    void (async () => {
      try {
        const asked = await document.getPage(pageNumber);
        page = asked;
        if (over) {
          asked.cleanup();
          return;
        }
        const natural = asked.getViewport({ scale: 1 });
        const scale = width > 0 ? Math.min(MOST_SCALE, width / natural.width) : UNMEASURED_SCALE;
        const viewport = asked.getViewport({ scale });
        const ratio = Math.min(
          window.devicePixelRatio || 1,
          Math.sqrt(MOST_PIXELS / (viewport.width * viewport.height)),
        );
        canvas.width = Math.floor(viewport.width * ratio);
        canvas.height = Math.floor(viewport.height * ratio);
        canvas.style.width = `${Math.floor(viewport.width)}px`;
        canvas.style.height = `${Math.floor(viewport.height)}px`;
        sheet.style.setProperty('--total-scale-factor', String(viewport.scale * viewport.userUnit));
        layer.replaceChildren();
        drawing = asked.render({
          canvas,
          viewport,
          transform: ratio === 1 ? undefined : [ratio, 0, 0, ratio, 0, 0],
        });
        const laid = new library.TextLayer({
          textContentSource: asked.streamTextContent(),
          container: layer,
          viewport,
        });
        text = laid;
        await Promise.all([drawing.promise, laid.render()]);
        if (!over) setDrawn({ key, hasText: laid.textDivs.length > 0 });
      } catch (error) {
        if (!over && !cancelled(error)) setDrawn({ key, error });
      }
    })();
    return () => {
      over = true;
      drawing?.cancel();
      text?.cancel();
      page?.cleanup();
      layer.replaceChildren();
    };
  }, [document, library, pageNumber, width, key]);

  const mine = drawn !== null && drawn.key === key ? drawn : null;
  const of = `Page ${pageNumber} of ${count} of ${name}.`;
  const label =
    mine === null
      ? `${of} It is being drawn.`
      : 'error' in mine
        ? `${of} PDF.js could not draw it.`
        : mine.hasText
          ? `${of} Its text is laid over the drawing, to select and to read aloud.`
          : `${of} This page has no text of its own in the PDF.`;
  return (
    <div ref={place} className="pdf-place">
      {mine !== null && 'error' in mine && (
        <div role="alert">
          <p>
            <strong>Drawing page {pageNumber}: PDF.js could not draw it.</strong>
          </p>
          {mine.error instanceof Error && mine.error.message.length > 0 && (
            <p>
              PDF.js said: <q>{mine.error.message}</q>
            </p>
          )}
        </div>
      )}
      <div ref={sheetRef} className="pdf-page">
        <canvas ref={canvasRef} role="img" aria-label={label} />
        <div ref={layerRef} className="textLayer" />
      </div>
    </div>
  );
}
