/**
 * The rules a name must meet before it is sent, in plain words.
 *
 * Two sources state them. The generated client's `zWorkspacePath` gives the shape of a whole path
 * (1 to 4096 characters, segments separated by `/`, none of `\ : < > " | ? *` or a control
 * character). The service's refusals state the rest, segment by segment
 * (`crates/contract/src/identity.rs`, `segment_error`): not `.`, `..` or `.git`; no dot or space
 * at the end; not a name Windows keeps for a device (`CON`, `NUL`, `COM1`, `LPT1` and the like,
 * with or without an extension). The service also wants Unicode NFC: a name is put into NFC
 * before it is sent, not refused, because the person cannot see the difference.
 *
 * Whether a name is taken is the service's to say (it compares without case and normalization):
 * that is a `conflict` answer, shown at the form.
 */
import { zWorkspacePath } from '../../api/generated/zod.gen';

/** The stems Windows reserves, in upper case (`identity.rs`, `RESERVED_STEMS`). */
const RESERVED_STEMS: ReadonlySet<string> = new Set([
  'CON',
  'PRN',
  'AUX',
  'NUL',
  'CONIN$',
  'CONOUT$',
  ...Array.from({ length: 10 }, (_, digit) => `COM${digit}`),
  ...Array.from({ length: 10 }, (_, digit) => `LPT${digit}`),
  ...['¹', '²', '³'].flatMap((digit) => [`COM${digit}`, `LPT${digit}`]),
]);
const RESERVED_CHARACTERS = /[\\:<>"|?*]/;

/** A control character (Unicode `Cc`): U+0000 to U+001F, and U+007F to U+009F. */
function isControl(character: string): boolean {
  const code = character.codePointAt(0) ?? 0;
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f);
}

/** Upper case for the letters a to z only, as the service compares a device name. */
const asciiUpper = (text: string) => text.replace(/[a-z]/g, (letter) => letter.toUpperCase());

/** What is wrong with one segment of a path (a file or folder name); null when nothing is. */
export function segmentProblem(segment: string): string | null {
  if (segment.length === 0) return 'Give it a name.';
  if (segment.includes('/')) return 'A name cannot contain “/”. Choose the folder separately.';
  if (segment === '.' || segment === '..' || asciiUpper(segment) === '.GIT') {
    return `“${segment}” cannot be used as a name.`;
  }
  if (RESERVED_CHARACTERS.test(segment)) {
    return 'A name cannot contain any of \\ : < > " | ? *';
  }
  if ([...segment].some(isControl)) return 'A name cannot contain a control character.';
  if (segment.endsWith('.') || segment.endsWith(' ')) {
    return 'A name cannot end with a dot or a space.';
  }
  const stem = segment.split('.').at(0) ?? segment;
  if (RESERVED_STEMS.has(asciiUpper(stem))) {
    return `“${stem}” is a name Windows keeps for a device, so it cannot be used.`;
  }
  return null;
}

/** What is wrong with a folder path ('' is the top level); null when nothing is. */
export function folderProblem(folder: string): string | null {
  if (folder === '') return null;
  for (const segment of folder.split('/')) {
    const problem = segmentProblem(segment);
    if (problem !== null) {
      return segment.length === 0
        ? 'A folder path has no “/” at its start or end, and no “//”.'
        : `In “${segment}”: ${problem}`;
    }
  }
  return null;
}

/** The path of `name` in `folder` ('' is the top level), in Unicode NFC. */
export function joinPath(folder: string, name: string): string {
  return (folder === '' ? name : `${folder}/${name}`).normalize('NFC');
}

/** A path's folder ('' for the top level) and its last segment. */
export function splitPath(path: string): { folder: string; name: string } {
  const cut = path.lastIndexOf('/');
  return cut < 0
    ? { folder: '', name: path }
    : { folder: path.slice(0, cut), name: path.slice(cut + 1) };
}

/** What is wrong with a whole path by the client's own schema; null when it accepts the path. */
export function pathProblem(path: string): string | null {
  if (zWorkspacePath.safeParse(path).success) return null;
  return path.length > 4096
    ? 'The folder and the name together are longer than 4096 characters.'
    : 'This is not a path the workspace can hold.';
}
