/**
 * The original beside its digest (UI M4a, ruling A2; SPEC section 7: "Original and digest may be
 * compared side by side"), with `@codemirror/merge` 6.12.
 *
 * For a text-like original: on the left its text, as its bytes say (`original-text.ts`, the same
 * parts the text viewer reads); on the right the text the reader shows for the item (the digest:
 * `read_item`, `view: 'text'`, the whole item, through `item-text.ts`), named for whose it is: the
 * converter's, a person's correction, or an agent's. Both panes are read-only text boxes, each
 * named, in one merge view that aligns their unchanged lines, scrolls them together and marks
 * what differs (deleted text in the original's pane, inserted text in the other's, a mark in the
 * gutter). The page counts the places that differ, and "Previous difference" and "Next
 * difference" step through them in order, saying which one it is and on which lines, with the
 * caret and keyboard focus in the original's pane there.
 *
 * Unchanged stretches are not collapsed: the merge view's collapsed lines open by a click only,
 * so what they hide would not be reachable from the keyboard.
 *
 * Bounded. Nothing is read until the comparison is asked for. Each side then shows what has been
 * read of it, says how far that is, and is read further only when asked, by its own way: the
 * original by its next byte range, the text by its cursor. The page says that only what is read
 * of each is compared. Keyboard focus goes to the pane that grew, where the new part begins.
 *
 * Offered wherever an original's text exists (the text viewer; a CSV or TSV spreadsheet), and
 * only when the item has text to compare: otherwise the page says why there is none.
 */
import { type Chunk, MergeView } from '@codemirror/merge';
import { EditorView } from '@codemirror/view';
import { type RefObject, useId, useLayoutEffect, useRef, useState } from 'react';
import type {
  ExtractionSummary,
  Revision,
  SourceAppearance,
} from '../../../api/generated/types.gen';
import { extractionSentence } from '../../documents/describe';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { joinedText, useItemText } from './item-text';
import { decodeOriginal, useOriginalText } from './original-text';
import { readOnlyText } from './ReadOnlyText';

/** The converted text is Markdown. */
const TEXT_MEDIA = 'text/markdown';

/** Past this long, a detailed comparison gives way to a rough one rather than hold the page. */
const DIFF_TIMEOUT_MS = 250;

const grouped = (count: number) => count.toLocaleString('en');

/** The name of the pane of the text the reader shows, for whose text it is. */
export function shownTextName(extraction: ExtractionSummary | null | undefined): string {
  if (extraction?.corrected) return 'Corrected text: what the reader shows';
  if (extraction?.text_origin === 'supplied_by_agent') {
    return 'Text supplied by an agent: what the reader shows';
  }
  return 'Converted text: what the reader shows';
}

/** Lines `from`–`to` of a pane in words, or that the place has none there. */
function linesIn(view: EditorView, from: number, to: number, end: number, name: string): string {
  if (from === to) return `no line of ${name}`;
  const first = view.state.doc.lineAt(from).number;
  const last = view.state.doc.lineAt(Math.max(from, Math.min(end, view.state.doc.length))).number;
  return first === last
    ? `line ${grouped(first)} of ${name}`
    : `lines ${grouped(first)}–${grouped(last)} of ${name}`;
}

interface Compared {
  /** The places that differ. */
  count: number;
  /** Every place was compared in detail. */
  precise: boolean;
}

const comparedOf = (chunks: readonly Chunk[]): Compared => ({
  count: chunks.length,
  precise: chunks.every((chunk) => chunk.precise),
});

interface MergeHandle {
  /** Put the caret at `position` of a pane, scroll there, and keyboard focus in it. */
  focusAt: (side: 'a' | 'b', position: number) => void;
}

interface MergePanesProps {
  original: string;
  converted: string;
  originalName: string;
  convertedName: string;
  mediaType: string;
  onCompared: (compared: Compared) => void;
  handle: RefObject<MergeHandle | null>;
}

/** A text that grows is added at its end, so the caret and the scroll stay where they were. */
function showText(view: EditorView, text: string): boolean {
  const shown = view.state.doc.toString();
  if (shown === text) return false;
  view.dispatch({
    changes: text.startsWith(shown)
      ? { from: shown.length, insert: text.slice(shown.length) }
      : { from: 0, to: shown.length, insert: text },
  });
  return true;
}

/**
 * The two panes in one merge view, created when both sides have a first part, and kept: a side
 * read further grows in place. The steps through the differences live here, beside the panes.
 */
function MergePanes({
  original,
  converted,
  originalName,
  convertedName,
  mediaType,
  onCompared,
  handle,
}: MergePanesProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const mergeRef = useRef<MergeView | null>(null);
  const first = useRef({ original, converted, originalName, convertedName, mediaType });
  const report = useRef(onCompared);
  useLayoutEffect(() => {
    report.current = onCompared;
  });
  // Which place the steps are at, and what the page says of it.
  const [at, setAt] = useState<number | null>(null);
  const [where, setWhere] = useState('');
  const [count, setCount] = useState(0);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (host === null) return;
    const start = first.current;
    const merge = new MergeView({
      a: {
        doc: start.original,
        extensions: readOnlyText(start.originalName, start.mediaType, null),
      },
      b: { doc: start.converted, extensions: readOnlyText(start.convertedName, TEXT_MEDIA, null) },
      parent: host,
      highlightChanges: true,
      gutter: true,
      diffConfig: { scanLimit: 500, timeout: DIFF_TIMEOUT_MS },
    });
    mergeRef.current = merge;
    setCount(merge.chunks.length);
    report.current(comparedOf(merge.chunks));
    return () => {
      mergeRef.current = null;
      merge.destroy();
    };
  }, []);
  useLayoutEffect(() => {
    const merge = mergeRef.current;
    if (merge === null || !showText(merge.a, original)) return;
    setCount(merge.chunks.length);
    report.current(comparedOf(merge.chunks));
  }, [original]);
  useLayoutEffect(() => {
    const merge = mergeRef.current;
    if (merge === null || !showText(merge.b, converted)) return;
    setCount(merge.chunks.length);
    report.current(comparedOf(merge.chunks));
  }, [converted]);
  useLayoutEffect(() => {
    handle.current = {
      focusAt: (side, position) => {
        const view = mergeRef.current?.[side];
        if (view === undefined) return;
        const place = Math.min(position, view.state.doc.length);
        view.dispatch({
          selection: { anchor: place },
          effects: EditorView.scrollIntoView(place, { y: 'start' }),
        });
        view.focus();
      },
    };
    return () => {
      handle.current = null;
    };
  }, [handle]);

  const step = (by: 1 | -1) => {
    const merge = mergeRef.current;
    if (merge === null) return;
    const chunks = merge.chunks;
    const next = at === null ? (by === 1 ? 0 : chunks.length - 1) : at + by;
    const chunk = chunks[next];
    if (chunk === undefined) return;
    setAt(next);
    setWhere(
      `Difference ${grouped(next + 1)} of ${grouped(chunks.length)}: ${linesIn(merge.a, chunk.fromA, chunk.toA, chunk.endA, 'the original')}; ${linesIn(merge.b, chunk.fromB, chunk.toB, chunk.endB, 'the shown text')}.`,
    );
    merge.a.dispatch({
      selection: { anchor: chunk.fromA },
      effects: EditorView.scrollIntoView(chunk.fromA, { y: 'center' }),
    });
    merge.b.dispatch({ effects: EditorView.scrollIntoView(chunk.fromB, { y: 'center' }) });
    merge.a.focus();
  };
  const atFirst = at === null || at <= 0;
  const atLast = at !== null && at >= count - 1;

  return (
    <>
      {count > 0 && (
        <p>
          <button
            type="button"
            aria-disabled={atFirst && at !== null}
            onClick={() => {
              if (!(atFirst && at !== null)) step(-1);
            }}
          >
            Previous difference
          </button>{' '}
          <button
            type="button"
            aria-disabled={atLast}
            onClick={() => {
              if (!atLast) step(1);
            }}
          >
            Next difference
          </button>
        </p>
      )}
      <p role="status">{where}</p>
      <div
        ref={hostRef}
        className="[&_.cm-mergeView]:max-h-[32rem] [&_.cm-mergeView]:overflow-auto [&_.cm-mergeView]:border [&_.cm-mergeView]:border-[color:GrayText]"
      />
    </>
  );
}

export interface OriginalBesideDigestProps {
  workspaceId: string;
  itemId: string;
  revision: Revision;
  path: string;
  appearance: SourceAppearance;
}

/** The comparison itself: both sides read, the panes, how far each is read, and the steps. */
function Comparison({
  id,
  workspaceId,
  itemId,
  revision,
  path,
  appearance,
}: OriginalBesideDigestProps & { id: string }) {
  const original = useOriginalText({ workspaceId, itemId, revision, appearance });
  const { request, read } = useItemText(workspaceId, itemId, revision, { kind: 'all' });
  const decoded = decodeOriginal(original.parts, original.complete);
  const pages = read.data?.pages ?? [];
  const converted = joinedText(pages);
  const convertedComplete = pages.length > 0 && !read.hasNextPage;
  const originalName = `Original: ${path}`;
  const convertedName = shownTextName(pages[0]?.extraction);
  const [compared, setCompared] = useState<Compared | null>(null);
  const size = original.size ?? 0;

  // Reading further moves keyboard focus into the pane that grew, where its new part begins; a
  // part that failed leaves focus on its button, under the failure's notice.
  const handle = useRef<MergeHandle | null>(null);
  const originalMoreRef = useRef<HTMLButtonElement | null>(null);
  const convertedMoreRef = useRef<HTMLButtonElement | null>(null);
  const awaited = useRef<{ side: 'a' | 'b'; parts: number; at: number } | null>(null);
  const originalParts = original.parts.length;
  const convertedParts = pages.length;
  const failedMore = original.read.isFetchNextPageError || read.isFetchNextPageError;
  const fetchingMore = original.read.isFetchingNextPage || read.isFetchingNextPage;
  useLayoutEffect(() => {
    const waiting = awaited.current;
    if (waiting === null) return;
    const parts = waiting.side === 'a' ? originalParts : convertedParts;
    if (parts > waiting.parts) {
      awaited.current = null;
      handle.current?.focusAt(waiting.side, waiting.at);
    } else if (failedMore && !fetchingMore) {
      awaited.current = null;
      (waiting.side === 'a' ? originalMoreRef : convertedMoreRef).current?.focus();
    }
  }, [originalParts, convertedParts, failedMore, fetchingMore]);
  const readMore = (side: 'a' | 'b') => {
    if (side === 'a') {
      if (original.read.isFetchingNextPage) return;
      awaited.current = { side, parts: originalParts, at: decoded.text.length };
      void original.read.fetchNextPage();
    } else {
      if (read.isFetchingNextPage) return;
      awaited.current = { side, parts: convertedParts, at: converted.length };
      void read.fetchNextPage();
    }
  };

  const panesRef = useRef<HTMLDivElement | null>(null);
  const pane = () => panesRef.current?.querySelector<HTMLElement>('.cm-content');
  const originalFailure = useRetry({
    request: `original ${workspaceId}/${itemId}/${revision}/${appearance.object}`,
    failed: original.read.isError && !original.read.isFetchNextPageError,
    error: original.read.error,
    run: () => original.read.refetch(),
    loaded: pane,
  });
  const convertedFailure = useRetry({
    request: JSON.stringify(request),
    failed: read.isError && !read.isFetchNextPageError,
    error: read.error,
    run: () => read.refetch(),
    loaded: pane,
  });
  const originalMoreFailure = useRetry({
    request: `more of the original ${workspaceId}/${itemId}/${revision}/${appearance.object}`,
    failed: original.read.isFetchNextPageError,
    error: original.read.error,
    run: () => original.read.fetchNextPage(),
    loaded: pane,
  });
  const convertedMoreFailure = useRetry({
    request: `more of ${JSON.stringify(request)}`,
    failed: read.isFetchNextPageError,
    error: read.error,
    run: () => read.fetchNextPage(),
    loaded: pane,
  });

  const ready = (original.size === 0 || original.parts.length > 0) && pages.length > 0;
  const bothComplete = original.complete && convertedComplete;
  const partsWord = (parts: number) =>
    parts === 1 ? 'The first part' : `The first ${grouped(parts)} parts`;

  return (
    <section id={id} aria-label="The original beside its converted text" className="mt-3">
      {!ready &&
        (original.read.isPending || read.isPending) &&
        !originalFailure.retrying &&
        !convertedFailure.retrying && (
          <p role="status">Reading the original and the text the reader shows…</p>
        )}
      {originalFailure.shown && (
        <FailureNotice
          context="Reading the original"
          error={originalFailure.error}
          retry={originalFailure}
          workspaceId={workspaceId}
        />
      )}
      {convertedFailure.shown && (
        <FailureNotice
          context="Reading the converted text"
          error={convertedFailure.error}
          retry={convertedFailure}
          workspaceId={workspaceId}
        />
      )}
      {ready && (
        <div ref={panesRef}>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <p className="m-0 font-semibold">{originalName}</p>
              <p className="meta m-0">
                {original.complete
                  ? `All ${grouped(size)} bytes of the original are compared.`
                  : `Bytes 1–${grouped(original.shown)} of ${grouped(size)} of the original are compared.`}
              </p>
            </div>
            <div>
              <p className="m-0 font-semibold">{convertedName}</p>
              <p className="meta m-0">
                {convertedComplete
                  ? `All the converted text is compared${pages.length > 1 ? `, in ${grouped(pages.length)} parts` : ''}.`
                  : `${partsWord(pages.length)} of the converted text ${pages.length === 1 ? 'is' : 'are'} compared; more can be read.`}
              </p>
            </div>
          </div>
          {compared !== null && (
            <p role="status">
              {compared.count === 0
                ? bothComplete
                  ? 'No difference: the converted text is the original’s text.'
                  : 'No difference in what is read of each.'
                : `${grouped(compared.count)} ${compared.count === 1 ? 'place differs' : 'places differ'}.`}
              {compared.precise
                ? ''
                : ' Some stretches were compared roughly: a difference there may look wider than it is.'}
            </p>
          )}
          {!bothComplete && (
            <p role="note" className="notice">
              Only what is read of each is compared: what is not read yet shows as a difference at
              the end.
            </p>
          )}
          <MergePanes
            original={decoded.text}
            converted={converted}
            originalName={originalName}
            convertedName={convertedName}
            mediaType={appearance.media_type}
            onCompared={setCompared}
            handle={handle}
          />
          <p>
            {original.read.hasNextPage && (
              <button
                type="button"
                ref={originalMoreRef}
                aria-disabled={original.read.isFetchingNextPage}
                onClick={() => readMore('a')}
              >
                {original.read.isFetchingNextPage ? 'Reading more…' : 'Read more of the original'}
              </button>
            )}{' '}
            {read.hasNextPage && (
              <button
                type="button"
                ref={convertedMoreRef}
                aria-disabled={read.isFetchingNextPage}
                onClick={() => readMore('b')}
              >
                {read.isFetchingNextPage ? 'Reading more…' : 'Read more of the converted text'}
              </button>
            )}
          </p>
          {originalMoreFailure.shown && (
            <FailureNotice
              context="Reading more of the original"
              error={originalMoreFailure.error}
              retry={originalMoreFailure}
              workspaceId={workspaceId}
            />
          )}
          {convertedMoreFailure.shown && (
            <FailureNotice
              context="Reading more of the converted text"
              error={convertedMoreFailure.error}
              retry={convertedMoreFailure}
              workspaceId={workspaceId}
            />
          )}
        </div>
      )}
    </section>
  );
}

/** "Compare the original with its converted text", and the comparison once it is asked for. */
export function OriginalBesideDigest(props: OriginalBesideDigestProps) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const { extraction } = props.appearance;
  if (extraction.text_origin === 'none') {
    return (
      <p className="meta">
        There is no converted text to compare the original with:{' '}
        {extractionSentence(extraction.outcome.status)}
      </p>
    );
  }
  return (
    <div className="mt-3">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen(!open)}
      >
        Compare the original with its converted text
      </button>
      {open && <Comparison id={id} {...props} />}
    </div>
  );
}
