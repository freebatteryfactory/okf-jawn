/**
 * The text and code viewer (UI M4a, ruling A2): a text or code original, as its bytes say.
 *
 * The bytes are read a part at a time through the transport's byte read (`original-text.ts`:
 * `download_object` with a range, never `get_object`), decoded as UTF-8 and shown read-only in
 * CodeMirror (`ReadOnlyText.tsx`), named for the file, with a language where an installed one
 * fits. The page says how much of the original is shown, and offers the rest one part per press;
 * keyboard focus then goes into the text where the new part begins. Bytes that are not UTF-8
 * are shown as the replacement character and marked, and the page says so, apart from a
 * replacement character the file holds itself.
 *
 * A `lines` selection is marked and the caret starts on its first line. The page says how those
 * lines count: a selection names lines of the item's text, which are the original's lines only
 * where the conversion kept them line for line. Lines past what is shown, or past the end, are
 * said to be.
 *
 * The original is offered beside the text the reader shows for it (`OriginalBesideDigest.tsx`).
 */
import { useLayoutEffect, useRef } from 'react';
import type { Selection } from '../../../api/generated/types.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { describeSize } from '../../imports/files';
import type { OriginalProps } from '../types';
import { OriginalBesideDigest } from './OriginalBesideDigest';
import { decodeOriginal, lineCount, useOriginalText } from './original-text';
import { ReadOnlyText, type ReadOnlyTextHandle } from './ReadOnlyText';

const grouped = (count: number) => count.toLocaleString('en');

/** "Line 3" or "Lines 2–3". */
function linesWords(start: number, end: number): string {
  return start === end ? `Line ${grouped(start)}` : `Lines ${grouped(start)}–${grouped(end)}`;
}

/** What the page says of the lines a selection names, against the text shown. */
function linesSaid(selection: Selection, text: string, complete: boolean): string | null {
  if (selection.kind !== 'lines') return null;
  const { start, end } = selection.range;
  const lines = linesWords(start, end);
  const one = start === end;
  if (complete && start > lineCount(text)) {
    const count = lineCount(text);
    return `The original has ${grouped(count)} ${count === 1 ? 'line' : 'lines'}: ${lines.toLowerCase()} ${one ? 'is' : 'are'} not in it.`;
  }
  const shownTo = text.split('\n').length;
  if (!complete && start > shownTo) {
    return `${lines} ${one ? 'is' : 'are'} past the part shown, which ends at line ${grouped(shownTo)}. Show more of the original to reach ${one ? 'it' : 'them'}.`;
  }
  return `${lines} ${one ? 'is' : 'are'} marked. The selection counts the lines of the item’s text, which are the original’s lines only where the conversion kept them line for line.`;
}

/** How much of the original is shown. */
function extentSaid(shown: number, size: number, complete: boolean): string {
  if (complete)
    return `All ${grouped(size)} ${size === 1 ? 'byte' : 'bytes'} of the original are shown.`;
  return `Bytes 1–${grouped(shown)} of ${grouped(size)} of the original are shown (${describeSize(shown)} of ${describeSize(size)}).`;
}

export function TextOriginal({
  workspaceId,
  itemId,
  revision,
  path,
  appearance,
  selection,
}: OriginalProps) {
  const original = useOriginalText({ workspaceId, itemId, revision, appearance });
  const { read, size, shown, complete, parts } = original;
  const decoded = decodeOriginal(parts, complete);
  const label = `Original text of ${path}`;

  // "Show more" puts keyboard focus in the text where the part read begins; a part that failed
  // leaves focus on the button, under the failure's notice.
  const textRef = useRef<HTMLDivElement | null>(null);
  const handle = useRef<ReadOnlyTextHandle | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const awaited = useRef<{ parts: number; at: number } | null>(null);
  const partCount = parts.length;
  const continuing = read.isFetchingNextPage;
  const continuationFailed = read.isFetchNextPageError;
  useLayoutEffect(() => {
    const waiting = awaited.current;
    if (waiting === null) return;
    if (partCount > waiting.parts) {
      awaited.current = null;
      handle.current?.focusAt(waiting.at);
    } else if (continuationFailed && !continuing) {
      awaited.current = null;
      moreRef.current?.focus();
    }
  }, [partCount, continuing, continuationFailed]);
  const showMore = () => {
    if (continuing) return;
    awaited.current = { parts: parts.length, at: decoded.text.length };
    void read.fetchNextPage();
  };

  const reading = `${workspaceId}/${itemId}/${revision}/${appearance.object}`;
  const readFailure = useRetry({
    request: reading,
    failed: read.isError && !read.isFetchNextPageError,
    error: read.error,
    run: () => read.refetch(),
    loaded: () => textRef.current?.querySelector<HTMLElement>('.cm-content'),
  });
  const moreFailure = useRetry({
    request: reading,
    failed: read.isFetchNextPageError,
    error: read.error,
    run: () => read.fetchNextPage(),
    loaded: () => moreRef.current ?? textRef.current?.querySelector<HTMLElement>('.cm-content'),
  });

  if (size === null) {
    return (
      <p role="note" className="notice">
        The service gives the size of the original as “{appearance.size}”, which is not a byte
        count, so the original is not read here.
      </p>
    );
  }
  if (size === 0) return <p>The original is empty: 0 bytes.</p>;

  const lines = linesSaid(selection, decoded.text, complete);
  return (
    <div className="text-original">
      <p>The original of {path}, as text (UTF-8), read-only.</p>
      {read.isPending && !readFailure.retrying && <p role="status">Reading the original…</p>}
      {readFailure.shown && (
        <FailureNotice
          context="Reading the original"
          error={readFailure.error}
          retry={readFailure}
          workspaceId={workspaceId}
        />
      )}
      {parts.length > 0 && (
        <>
          <p>{extentSaid(shown, size, complete)}</p>
          {decoded.replacements > 0 && (
            <p role="note" className="notice">
              {decoded.notUtf8
                ? `Some bytes of the original are not UTF-8: each place is shown as the replacement character “�” and marked (${grouped(decoded.replacements)} in what is shown).`
                : `The original holds the replacement character “�” itself, marked (${grouped(decoded.replacements)} in what is shown).`}
            </p>
          )}
          {lines !== null && (
            <p role="note" className="notice">
              {lines}
            </p>
          )}
          <div ref={textRef}>
            <ReadOnlyText
              label={label}
              text={decoded.text}
              mediaType={appearance.media_type}
              marked={selection.kind === 'lines' ? selection.range : null}
              handle={handle}
            />
          </div>
        </>
      )}
      <p role="status" className="sr-only">
        {continuing && !moreFailure.retrying ? 'Reading more of the original…' : ''}
      </p>
      {read.hasNextPage && (
        <button type="button" ref={moreRef} aria-disabled={continuing} onClick={showMore}>
          {continuing ? 'Reading more…' : 'Show more of the original'}
        </button>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Reading more of the original"
          error={moreFailure.error}
          retry={moreFailure}
          workspaceId={workspaceId}
        />
      )}
      <OriginalBesideDigest
        workspaceId={workspaceId}
        itemId={itemId}
        revision={revision}
        path={path}
        appearance={appearance}
      />
    </div>
  );
}
