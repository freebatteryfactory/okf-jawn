/**
 * Read a unified text diff, as the service sends it in `FileChange.patch`, into the lines a person
 * is shown: what was removed, what was added, and the unchanged lines around them.
 *
 * The patch is Git's own text for one file: a header (file names, object ids), then hunks, each
 * opened by `@@ -old,count +new,count @@` and followed by lines whose first character says what
 * they are (a space: unchanged; `-`: removed; `+`: added; `\`: a remark about the line above). The
 * header is Git's vocabulary and is not shown; the file's paths come from the `FileChange` itself.
 * Nothing is interpreted beyond that first character: the text of each line is shown as it is.
 *
 * A patch numbers the lines of the whole item file, its properties header included.
 */

export interface PatchLine {
  kind: 'added' | 'removed' | 'same';
  text: string;
}

export interface PatchHunk {
  /** The line of the file this hunk starts at before the change, counting from one. */
  oldStart: number;
  /** The line it starts at after the change. */
  newStart: number;
  lines: PatchLine[];
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

export function readPatch(patch: string): PatchHunk[] {
  const hunks: PatchHunk[] = [];
  let hunk: PatchHunk | null = null;
  const rows = patch.split('\n');
  // A patch ends with a line break: the piece after it is no line.
  if (rows.at(-1) === '') rows.pop();
  for (const row of rows) {
    const opened = HUNK.exec(row);
    if (opened !== null) {
      hunk = { oldStart: Number(opened[1]), newStart: Number(opened[2]), lines: [] };
      hunks.push(hunk);
      continue;
    }
    // Everything before the first hunk is the header.
    if (hunk === null) continue;
    const mark = row.charAt(0);
    const text = row.slice(1);
    if (mark === '+') hunk.lines.push({ kind: 'added', text });
    else if (mark === '-') hunk.lines.push({ kind: 'removed', text });
    else if (mark === '\\') continue;
    else hunk.lines.push({ kind: 'same', text });
  }
  return hunks;
}
