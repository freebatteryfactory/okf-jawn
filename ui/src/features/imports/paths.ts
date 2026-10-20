/**
 * What the import page does with a path before anything of it is sent. The service is the
 * authority on what a path may be; the page only refrains from sending what it can see is not
 * what the person meant, and from showing a name that reads otherwise than it is.
 *
 * - (`tidyPath`, in `files.ts`, comes first: a backslash is a path separator.)
 * - `pathProblem`: a path the page does not send. A `..` step, a path that starts at a root (a
 *   leading separator), a path that names a disk drive ("C:"), and a control character (a line
 *   break among them) are each a problem; so is a name or a folder longer than `create_upload`
 *   takes (rulings 6 and 34). Those bounds are in bytes of UTF-8 (`x-max-utf8-bytes` in the
 *   request schema: `MAX_FILENAME_BYTES`, `MAX_FOLDER_BYTES`), so the bytes are counted
 *   (`TextEncoder`), never the characters: a letter with an accent takes two bytes, many others
 *   three or four. A unit test holds the two numbers to the schema.
 * - `collisionKey`: two paths with the same key would be the same place for a person: they are the
 *   same, or differ only in capital and small letters, or only in how an accented letter is
 *   composed (a Mac names files decomposed). The upload list holds back both (`upload-list.ts`).
 * - `shownName`: a name as it is drawn. A character that is not drawn itself and can change how
 *   the rest reads or what it seems to be (a right-to-left override, a zero-width space, a control
 *   character, a blank filler letter) is replaced by a visible mark of its code point, "⟨U+202E⟩".
 *   This covers every format character (`Cf`), so the joiners inside some emoji and inside
 *   Persian, Arabic and Indic words are shown as marks too: a name is listed here to be checked
 *   before it is sent, not to be read as prose. What is sent is the name itself, never this text.
 */
import { type NamedPath, pathOf } from './files';

export type PathProblem =
  | { kind: 'parent' }
  | { kind: 'absolute' }
  | { kind: 'drive' }
  | { kind: 'control' }
  | { kind: 'too-long'; part: 'name' | 'folder'; max: number };

/** The most bytes of UTF-8 of an upload's `filename` (`MAX_UPLOAD_FILENAME_BYTES`). */
export const MAX_FILENAME_BYTES = 1024;

/** The most bytes of UTF-8 of an upload's `relative_path` (`MAX_UPLOAD_RELATIVE_PATH_BYTES`). */
export const MAX_FOLDER_BYTES = 4096;

/** The most bytes each part of a path may have: by default the contract's. */
export interface PathLimits {
  filename: number;
  folder: number;
}

const WIRE_LIMITS: PathLimits = { filename: MAX_FILENAME_BYTES, folder: MAX_FOLDER_BYTES };

const encoder = new TextEncoder();

/** The length of `value` in bytes of UTF-8, as the service counts it. */
export function utf8Bytes(value: string): number {
  return encoder.encode(value).length;
}

/** A control character: C0, delete, or C1. */
function isControl(char: string): boolean {
  const code = char.codePointAt(0) ?? 0;
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f);
}

/** What the page can see is wrong with a path; null when it sees nothing. The first found is said. */
export function pathProblem(
  named: NamedPath,
  limits: PathLimits = WIRE_LIMITS,
): PathProblem | null {
  const path = pathOf(named);
  if (Array.from(path).some(isControl)) return { kind: 'control' };
  if (path.startsWith('/')) return { kind: 'absolute' };
  const steps = path.split('/');
  if (/^[A-Za-z]:$/.test(steps.at(0) ?? '')) return { kind: 'drive' };
  if (steps.includes('..')) return { kind: 'parent' };
  if (utf8Bytes(named.filename) > limits.filename) {
    return { kind: 'too-long', part: 'name', max: limits.filename };
  }
  if (utf8Bytes(named.folder) > limits.folder) {
    return { kind: 'too-long', part: 'folder', max: limits.folder };
  }
  return null;
}

/** Why a path is not sent, as a sentence that follows "Cannot be imported:". */
export function describeProblem(problem: PathProblem): string {
  switch (problem.kind) {
    case 'parent':
      return 'its path has a “..” step, which leads out of the folder it came in.';
    case 'absolute':
      return 'its path starts at the top of a disk or of a network share, not in a folder you chose.';
    case 'drive':
      return 'its path names a disk drive, such as “C:”.';
    case 'control':
      return 'its name holds a control character, such as a line break.';
    case 'too-long':
      return `its ${problem.part === 'name' ? 'name' : 'folder path'} is too long: the service takes at most ${problem.max.toLocaleString('en')} bytes of text, and a letter with an accent takes two, many other characters three or four.`;
  }
}

/** One key for every path that is the same place to a person (see the header). */
export function collisionKey(named: NamedPath): string {
  return pathOf(named).normalize('NFC').toLowerCase();
}

/**
 * Characters that are not drawn themselves: controls, format characters (the bidirectional
 * overrides and isolates, zero-width spaces and joiners, the soft hyphen, the byte-order mark,
 * tags), line and paragraph separators, and the four letters that are drawn as a blank.
 */
const UNSEEN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\u115F\u1160\u3164\uFFA0]/gu;

/** A name as it is drawn: each character that is not drawn itself as a mark of its code point. */
export function shownName(name: string): string {
  return name.replace(UNSEEN, (char) => {
    const code = (char.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, '0');
    return `⟨U+${code}⟩`;
  });
}
