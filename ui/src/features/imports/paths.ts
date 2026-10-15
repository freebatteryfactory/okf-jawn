/**
 * What the import page does with a path before anything of it is sent. The service is the
 * authority on what a path may be; the page only refrains from sending what it can see is not
 * what the person meant, and from showing a name that reads otherwise than it is.
 *
 * - (`tidyPath`, in `files.ts`, comes first: a backslash is a path separator.)
 * - `pathProblem`: a path the page does not send. A `..` step, a path that starts at a root (a
 *   leading separator), a path that names a disk drive ("C:"), and a control character (a line
 *   break among them) are each a problem; so is a name or a folder longer than the generated
 *   request schema of `create_upload` allows. That schema states the contract's bounds in UTF-8
 *   bytes (`x-max-utf8-bytes`), which the generated zod schema does not carry, so nothing is
 *   found too long here today and the service refuses a longer name; counting the bytes on the
 *   page is UI part 2's.
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
import type { z } from 'zod';
import { zCreateUploadRequest } from '../../api/generated/zod.gen';
import { type NamedPath, pathOf } from './files';

export type PathProblem =
  | { kind: 'parent' }
  | { kind: 'absolute' }
  | { kind: 'drive' }
  | { kind: 'control' }
  | { kind: 'too-long'; part: 'name' | 'folder'; max: number };

/** The length each part of a path may have, as schemas: by default the generated request's own. */
export interface PathLimits {
  filename: z.ZodType;
  folder: z.ZodType;
}

const WIRE_LIMITS: PathLimits = {
  filename: zCreateUploadRequest.shape.filename,
  folder: zCreateUploadRequest.shape.relative_path,
};

/** A control character: C0, delete, or C1. */
function isControl(char: string): boolean {
  const code = char.codePointAt(0) ?? 0;
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f);
}

/** The largest length the schema allows, when the value is over it; null when it is not. */
function overLength(schema: z.ZodType, value: string): number | null {
  const checked = schema.safeParse(value);
  if (checked.success) return null;
  for (const issue of checked.error.issues) {
    if (issue.code === 'too_big' && typeof issue.maximum === 'number') return issue.maximum;
  }
  return null;
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
  const nameMax = overLength(limits.filename, named.filename);
  if (nameMax !== null) return { kind: 'too-long', part: 'name', max: nameMax };
  const folderMax = overLength(limits.folder, named.folder);
  if (folderMax !== null) return { kind: 'too-long', part: 'folder', max: folderMax };
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
      return `its ${problem.part === 'name' ? 'name' : 'folder path'} is longer than the service accepts (${problem.max} characters at most).`;
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
