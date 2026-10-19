/**
 * Read a unified text diff, as the service sends it in `FileChange.patch`, into the lines a person
 * is shown: what was removed, what was added, and the unchanged lines around them.
 *
 * The patch is Git's own text for one file: a header (file names, object ids), then hunks, each
 * opened by `@@ -old,count +new,count @@` and followed by lines whose first character says what
 * they are (a space: unchanged; `-`: removed; `+`: added; `\`: a remark about the line above). The
 * header is Git's vocabulary and is not shown; the file's paths come from the `FileChange` itself.
 * Nothing is interpreted beyond that first character: the text of each line is shown as it is,
 * without the carriage return of a file with Windows line ends.
 *
 * What is read is bounded, because a patch is not: at most `room` lines are kept and the rest are
 * counted, and a line longer than `MAX_LINE_CHARS` is cut and marked as cut. A caller may ask
 * for a later window and retain the complete long lines for bounded, explicit inspection.
 *
 * A patch numbers the lines of the whole item file, its properties header included.
 */

/** The most lines of one conflict's diff put on the page. */
export const MAX_DIFF_LINES = 2_000;
/** The most characters of one line put on the page. */
export const MAX_LINE_CHARS = 2_000;

export interface PatchLine {
  kind: 'added' | 'removed' | 'same';
  text: string;
  /** The line was longer than `MAX_LINE_CHARS`; `text` is its beginning. */
  cut?: true;
  /** Only requested by an inspectable comparison: the source for successive bounded chunks. */
  whole?: string;
}

export interface PatchHunk {
  /** The line of the file this hunk starts at before the change, counting from one. */
  oldStart: number;
  /** The line it starts at after the change. */
  newStart: number;
  lines: PatchLine[];
}

export interface ReadPatch {
  hunks: PatchHunk[];
  /** How many lines are in `hunks`. */
  shown: number;
  /** How many lines the patch holds outside this window, before it or after it. */
  more: number;
}

export interface PatchWindow {
  /** How many diff lines precede this window, not counting headers or remarks. */
  start?: number;
  /** Retain long lines for inspection; their initial `text` stays bounded. */
  inspectLongLines?: boolean;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;
const KIND = { '+': 'added', '-': 'removed' } as const;

export function readPatch(
  patch: string,
  room: number = MAX_DIFF_LINES,
  { start = 0, inspectLongLines = false }: PatchWindow = {},
): ReadPatch {
  const hunks: PatchHunk[] = [];
  let hunk: PatchHunk | null = null;
  let inHunk = false;
  let shown = 0;
  let more = 0;
  let skipped = 0;
  let oldLine = 0;
  let newLine = 0;
  const rows = patch.split('\n');
  // A patch ends with a line break: the piece after it is no line.
  if (rows.at(-1) === '') rows.pop();
  for (const raw of rows) {
    const row = raw.endsWith('\r') ? raw.slice(0, -1) : raw;
    const opened = HUNK.exec(row);
    if (opened !== null) {
      inHunk = true;
      hunk = null;
      oldLine = Number(opened[1]);
      newLine = Number(opened[2]);
      if (shown < room && skipped >= start) {
        hunk = { oldStart: oldLine, newStart: newLine, lines: [] };
        hunks.push(hunk);
      }
      continue;
    }
    // Everything before the first hunk is the header; a remark is about the line above.
    const mark = row.charAt(0);
    if (!inHunk || mark === '\\') continue;
    const kind = mark === '+' || mark === '-' ? KIND[mark] : 'same';
    if (skipped < start) {
      skipped += 1;
      more += 1;
    } else if (shown >= room) {
      more += 1;
    } else {
      // A window may begin within a hunk; its displayed start is the line reached, not the
      // hunk's original start. Removed lines advance only the old file, added only the new.
      if (hunk === null) {
        hunk = { oldStart: oldLine, newStart: newLine, lines: [] };
        hunks.push(hunk);
      }
      const text = row.slice(1);
      hunk.lines.push(
        text.length > MAX_LINE_CHARS
          ? {
              kind,
              text: text.slice(0, MAX_LINE_CHARS),
              cut: true,
              ...(inspectLongLines ? { whole: text } : {}),
            }
          : { kind, text },
      );
      shown += 1;
    }
    if (kind !== 'added') oldLine += 1;
    if (kind !== 'removed') newLine += 1;
  }
  return { hunks, shown, more };
}
