/**
 * The one way a change between two versions is put on the page, wherever it is shown: the files
 * of a Snapshot's conflict (`drafts/ConflictNotice.tsx`), Changes (`Changes.tsx`, shared with MCP
 * Apps), a proposal and a Rewind's preview.
 *
 * Each file says what happened to it in words (added, changed, moved with both of its paths,
 * deleted), then its changed lines with the unchanged lines around them, read by the one patch
 * reader (`drafts/patch.ts`), which leaves Git's patch header out. Added and removed lines are
 * `ins` and `del`, each also marked with a sign and, for a screen reader, a word, so the
 * difference does not rest on color. A file that is not text says so.
 *
 * The lines are bounded over all the files of one comparison together: at most `MAX_DIFF_LINES`
 * go on the page, the rest are counted and the count is said. Inspectable comparisons can move
 * that window and read a long line in bounded chunks without dropping any of its content.
 */
import { useState } from 'react';
import type { z } from 'zod';
import type { zFileChange } from '../../api/generated/zod.gen';
import { keyed } from '../../lib/keys';
import {
  MAX_DIFF_LINES,
  MAX_LINE_CHARS,
  type PatchHunk,
  type PatchLine,
  type PatchWindow,
  readPatch,
} from '../drafts/patch';

/** One changed file, as an answer of the service carries it (a comparison, a conflict). */
export type FileChange = z.infer<typeof zFileChange>;

/** What happened to a file, in words. */
export const CHANGE_KIND = {
  added: 'Added',
  modified: 'Changed',
  moved: 'Moved',
  removed: 'Deleted',
} as const;

/** The paths of a changed file: both of them when it moved. */
export function paths(change: FileChange): string {
  const from = change.old_path ?? null;
  const to = change.new_path ?? null;
  if (from !== null && to !== null && from !== to) return `${from}, now ${to}`;
  return to ?? from ?? 'this item';
}

/** The mark after a line that was cut, in words: it is read out as well as seen. */
const CUT = ' … (a long line, cut here)';

/** One file of a comparison, read for the page. */
export interface ReadFile {
  change: FileChange;
  hunks: PatchHunk[];
  /** Lines this file holds outside the current window; they are not unchanged text. */
  hiddenLines: number;
}

/**
 * The files of one comparison as the page shows them: so many lines over all of them, at most
 * `room`, and how many lines precede and follow the window. The same reader serves the first
 * window and every later one; there is no unbounded rendering mode.
 */
export function readChanges(
  changes: readonly FileChange[],
  room: number = MAX_DIFF_LINES,
  { start = 0, inspectLongLines = false }: PatchWindow = {},
): { files: ReadFile[]; before: number; more: number } {
  const files: ReadFile[] = [];
  let left = room;
  let skip = start;
  let before = 0;
  let more = 0;
  for (const change of changes) {
    const read = readPatch(change.patch, left, { start: skip, inspectLongLines });
    // `shown + more` is every diff line in this file, including any skipped before the window.
    const skipped = Math.min(skip, read.shown + read.more);
    files.push({ change, hunks: read.hunks, hiddenLines: read.more });
    skip -= skipped;
    before += skipped;
    left -= read.shown;
    more += read.more - skipped;
  }
  return { files, before, more };
}

/** React keys for the files of one comparison. */
export function keyedFiles(files: readonly ReadFile[]) {
  return keyed(files, ({ change }) => JSON.stringify([change.old_path, change.new_path]));
}

export function ChangedFile({ change, hunks, hiddenLines }: ReadFile) {
  return (
    <div className="changed-file">
      <p>
        <strong>{CHANGE_KIND[change.kind]}:</strong> {paths(change)}
      </p>
      {change.binary ? (
        <p>This file is not text, so its lines cannot be compared.</p>
      ) : hunks.length === 0 && hiddenLines > 0 ? (
        <p>Its changed lines are outside this part of the comparison.</p>
      ) : hunks.length === 0 ? (
        <p>No line of its text changed.</p>
      ) : (
        keyed(hunks, (hunk) => `${hunk.oldStart}:${hunk.newStart}`).map(({ key, value: hunk }) => (
          <div key={key} className="hunk">
            <p className="meta">From line {hunk.newStart} of the file</p>
            <pre className="patch-lines">
              {keyed(hunk.lines, (line) => `${line.kind}:${line.text}`).map(
                ({ key: lineKey, value: line }) =>
                  line.kind === 'added' ? (
                    <ins key={lineKey}>
                      <span className="sr-only">Added: </span>
                      <span aria-hidden="true">+ </span>
                      <LineText line={line} />
                      {'\n'}
                    </ins>
                  ) : line.kind === 'removed' ? (
                    <del key={lineKey}>
                      <span className="sr-only">Removed: </span>
                      <span aria-hidden="true">− </span>
                      <LineText line={line} />
                      {'\n'}
                    </del>
                  ) : (
                    <span key={lineKey}>
                      {'  '}
                      <LineText line={line} />
                      {'\n'}
                    </span>
                  ),
              )}
            </pre>
          </div>
        ))
      )}
    </div>
  );
}

/** Ordinary lines keep their existing rendering; inspectable long lines have bounded chunks. */
function LineText({ line }: { line: PatchLine }) {
  return line.whole === undefined ? (
    <>
      {line.text}
      {line.cut ? CUT : ''}
    </>
  ) : (
    <LongLine text={line.whole} />
  );
}

/** A long source line is always available, but at most MAX_LINE_CHARS are rendered at once. */
function LongLine({ text }: { text: string }) {
  const [reading, setReading] = useState<{ text: string; start: number; earlier: number[] }>({
    text,
    start: 0,
    earlier: [],
  });
  // If the comparison's source changes, no chunk from its previous text is used for the new one.
  const current = reading.text === text ? reading : { text, start: 0, earlier: [] };
  const { start, earlier } = current;
  let end = Math.min(text.length, start + MAX_LINE_CHARS);
  // A valid Unicode character must not become two replacement characters across chunk edges.
  const last = text.charCodeAt(end - 1);
  const next = text.charCodeAt(end);
  if (last >= 0xd800 && last <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) end -= 1;
  const more = end < text.length;
  return (
    <>
      {text.slice(start, end)}
      {more ? CUT : ''}
      <span className="meta">
        {' '}
        Characters {start + 1} to {end} of {text.length}.{' '}
        <button
          type="button"
          aria-disabled={earlier.length === 0}
          onClick={() => {
            const previous = earlier.at(-1);
            if (previous === undefined) return;
            setReading({ text, start: previous, earlier: earlier.slice(0, -1) });
          }}
        >
          Earlier in this line
        </button>{' '}
        <button
          type="button"
          aria-disabled={!more}
          onClick={() => {
            if (more) setReading({ text, start: end, earlier: [...earlier, start] });
          }}
        >
          Continue this line
        </button>
      </span>
    </>
  );
}

/** What was left out of a comparison too long to show whole; nothing when nothing was. */
export function MoreLines({ more }: { more: number }) {
  if (more <= 0) return null;
  return (
    <p className="meta">
      and {more.toLocaleString('en')} more {more === 1 ? 'line' : 'lines'}: this change is too long
      to show whole.
    </p>
  );
}
