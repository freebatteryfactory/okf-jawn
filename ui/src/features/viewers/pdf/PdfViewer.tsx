/**
 * The PDF viewer (UI M4a, ruling A2; SPEC section 7: the human viewer includes PDF): the original
 * of a source, drawn by PDF.js one page at a time, with its text layer.
 *
 * Its bytes are read through the byte read and opened by PDF.js (`use-pdf-document.ts`): whole
 * and handed over with `getDocument({ data })`, or, for a large PDF, in parts as the pages shown
 * need them, which the viewer says. PDF.js's worker, fonts and CMaps are this build's own files
 * (`pdfjs.ts`, `vite.config.ts`). The document is let go when the viewer goes.
 *
 * Paging. "Previous page" and "Next page" are buttons, and a labelled number field with "Go to
 * page" goes to any page; a polite status says which page of how many is shown, and a page the PDF
 * does not have is said, with the field marked invalid. A button at its end stays focusable and
 * says it does nothing (`aria-disabled`, never `disabled`). The reader's selection decides where
 * the PDF opens (`pages.ts`): a selection of pages at its first page, and the range is said; it
 * opens there again when the reader shows another selection.
 *
 * Failing. A failed read is the transport's typed failure in its words, with "Try again"
 * (`FailureNotice`, `useRetry`); keyboard focus then goes to the first control, or to the viewer
 * itself while the PDF opens. A file PDF.js cannot open is said in PDF.js's words, and the
 * original is offered as a download (`office/OriginalAccess.tsx`); a PDF protected by a password
 * says that this viewer does not ask for one.
 */
import { type FormEvent, type RefObject, useId, useRef, useState } from 'react';
import type { Selection } from '../../../api/generated/types.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import type { FailureMessage } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import { sizeWords } from '../../jobs/ArtifactDownload';
import { OriginalAccess } from '../office/OriginalAccess';
import { originalName } from '../office/original';
import type { OriginalProps } from '../types';
import { PdfPage } from './PdfPage';
import { type Opening, openingOf, pagesWords } from './pages';
import type { PDFDocumentProxy } from './pdfjs';
import { type PdfLibrary, type PdfState, usePdfDocument } from './use-pdf-document';

/** Words for a part of a large PDF answered with another number of bytes than asked. */
const PART_MISMATCH: FailureMessage = {
  title: 'The service answered a part of the original with another number of bytes than asked.',
  advice: 'Try again. If it happens again, the stored original may not match its recorded size.',
  action: { kind: 'retry', label: 'Try again' },
};

/** Words of this place for a failed read, where the general ones would mislead. */
function readWords(error: unknown): FailureMessage | undefined {
  return error instanceof Error && error.name === 'PartMismatch' ? PART_MISMATCH : undefined;
}

export function PdfViewer(props: OriginalProps) {
  const { workspaceId, itemId, revision, appearance, selection } = props;
  const pdf = usePdfDocument({
    workspaceId,
    itemId,
    revision,
    digest: appearance.object,
    size: Number(appearance.size),
  });
  const viewerRef = useRef<HTMLDivElement | null>(null);
  const controlsRef = useRef<HTMLFieldSetElement | null>(null);
  const { shown } = pdf;
  const failure = useRetry({
    request: pdf.request,
    failed: shown.state === 'unread',
    error: shown.state === 'unread' ? shown.error : null,
    run: pdf.again,
    loaded: () => controlsRef.current?.querySelector<HTMLElement>('button') ?? viewerRef.current,
  });
  return (
    // Focusable from script only: where focus goes after a retry while the PDF still opens.
    <div ref={viewerRef} tabIndex={-1} className="pdf-viewer grid gap-3">
      {pdf.inParts && (
        <p className="meta">
          This PDF is large ({sizeWords(appearance.size)}): it is read in parts, as the pages shown
          need them.
        </p>
      )}
      {failure.shown ? (
        <FailureNotice
          context="Reading the original"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
          words={readWords(failure.error)}
        />
      ) : (
        <Shown
          shown={shown}
          file={props}
          selection={selection}
          controlsRef={controlsRef}
          size={appearance.size}
        />
      )}
    </div>
  );
}

function Shown({
  shown,
  file,
  selection,
  controlsRef,
  size,
}: {
  shown: PdfState;
  file: OriginalProps;
  selection: Selection;
  controlsRef: RefObject<HTMLFieldSetElement | null>;
  size: string;
}) {
  switch (shown.state) {
    case 'reading':
      return <p role="status">Reading the original, {sizeWords(size)}…</p>;
    case 'opening':
    // A failed read is shown by the viewer's notice; nothing more is shown here meanwhile.
    case 'unread':
      return <p role="status">Opening the PDF…</p>;
    case 'unreadable':
      return <Unreadable error={shown.error} file={file} />;
    case 'open':
      return (
        <PdfPages
          key={shown.key}
          document={shown.document}
          library={shown.library}
          name={originalName(file)}
          selection={selection}
          controlsRef={controlsRef}
        />
      );
  }
}

/** A file PDF.js could not open, in its words, and the original as a download. */
function Unreadable({ error, file }: { error: unknown; file: OriginalProps }) {
  const name = error instanceof Error ? error.name : '';
  const title =
    name === 'PasswordException'
      ? 'this PDF is protected by a password, which this viewer does not ask for.'
      : name === 'InvalidPDFException'
        ? 'PDF.js could not read this file as a PDF.'
        : 'PDF.js could not open it.';
  const said =
    name !== 'PasswordException' && error instanceof Error && error.message.length > 0
      ? error.message
      : null;
  return (
    <>
      <div role="alert">
        <p>
          <strong>Opening the PDF: {title}</strong>
        </p>
        {said !== null && (
          <p>
            PDF.js said: <q>{said}</q>
          </p>
        )}
        <p>Its bytes are kept: the original can be downloaded, to open it with another program.</p>
      </div>
      <OriginalAccess {...file} />
    </>
  );
}

/** Why a page typed in the field is not gone to. */
function refusedWords(typed: string, count: number): string {
  const asked = Number(typed);
  if (typed === '') return `Type a page number, from 1 to ${count}.`;
  if (!Number.isInteger(asked))
    return `“${typed}” is not a page number: type one from 1 to ${count}.`;
  return `There is no page ${typed}: this PDF has ${pagesWords(count)}.`;
}

interface PdfPagesProps {
  document: PDFDocumentProxy;
  library: PdfLibrary;
  name: string;
  selection: Selection;
  controlsRef: RefObject<HTMLFieldSetElement | null>;
}

/** The pages of an open PDF, one shown at a time, and the controls that page through them. */
function PdfPages({ document, library, name, selection, controlsRef }: PdfPagesProps) {
  const count = document.numPages;
  const named = JSON.stringify(selection);
  const [opening, setOpening] = useState<{ named: string; at: Opening }>(() => ({
    named,
    at: openingOf(selection, count),
  }));
  const [page, setPage] = useState(opening.at.page);
  const [typed, setTyped] = useState(String(opening.at.page));
  const [refused, setRefused] = useState<string | null>(null);
  // The reader shows another selection: the PDF opens there again.
  if (opening.named !== named) {
    const at = openingOf(selection, count);
    setOpening({ named, at });
    setPage(at.page);
    setTyped(String(at.page));
    setRefused(null);
  }
  const fieldId = useId();
  const refusedId = useId();
  const go = (target: number) => {
    setPage(target);
    setTyped(String(target));
    setRefused(null);
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const asked = typed.trim();
    const target = Number(asked);
    if (asked !== '' && Number.isInteger(target) && target >= 1 && target <= count) go(target);
    else setRefused(asked);
  };
  const first = page <= 1;
  const last = page >= count;
  return (
    <>
      {opening.at.said !== null && <p>{opening.at.said}</p>}
      <fieldset ref={controlsRef} className="m-0 flex flex-wrap items-center gap-2 border-0 p-0">
        <legend className="sr-only">Pages of the PDF</legend>
        <button
          type="button"
          aria-disabled={first}
          onClick={() => {
            if (!first) go(page - 1);
          }}
        >
          Previous page
        </button>
        <button
          type="button"
          aria-disabled={last}
          onClick={() => {
            if (!last) go(page + 1);
          }}
        >
          Next page
        </button>
        {/* The page's own words say a page the PDF does not have: the browser's validation would
            stop the form with a bubble of its own instead (`noValidate`). */}
        <form onSubmit={submit} noValidate className="flex flex-wrap items-center gap-2">
          <label htmlFor={fieldId}>Page number</label>
          <input
            id={fieldId}
            type="number"
            inputMode="numeric"
            min={1}
            max={count}
            value={typed}
            onChange={(event) => setTyped(event.target.value)}
            aria-invalid={refused !== null}
            aria-describedby={refused === null ? undefined : refusedId}
            className="w-20"
          />
          <button type="submit">Go to page</button>
        </form>
        <p role="status" className="meta m-0">
          Page {page} of {count}
        </p>
      </fieldset>
      {refused !== null && (
        <p id={refusedId} role="alert">
          {refusedWords(refused, count)}
        </p>
      )}
      <PdfPage document={document} library={library} pageNumber={page} count={count} name={name} />
    </>
  );
}
