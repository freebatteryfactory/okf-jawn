/**
 * The files a person chose, each with the folder it came in: the relative path is preserved (SPEC
 * section 5: "Multi-file and folder upload preserves relative paths").
 *
 * Three ways in, and the folder each gives:
 * - the file picker: files with no folder;
 * - the folder picker: the browser names each file's path from the chosen folder down
 *   (`webkitRelativePath`, "reports/2025/a.pdf"), so the folder is everything before the name;
 * - a drop of files and folders: the dropped entries are walked with Uppy's `getDroppedFiles`
 *   (`@uppy/core/utils`, the one part of Uppy used here), which reads every directory to its end
 *   and names each file's path the same way. An entry it could not read is counted, never dropped
 *   without a word.
 *
 * The name is the file's own, exactly; the folder is exactly the path the browser gave, with `/`
 * between its parts. Nothing is read from a file here.
 */
import { getDroppedFiles } from '@uppy/core/utils';

export interface ChosenFile {
  file: File;
  /** The file's own name. */
  filename: string;
  /** The folder it came in, relative to what was chosen; empty for a file chosen by itself. */
  folder: string;
}

/** A file's path as it is shown and kept: its folder, then its name. */
export function pathOf(chosen: { folder: string; filename: string }): string {
  return chosen.folder === '' ? chosen.filename : `${chosen.folder}/${chosen.filename}`;
}

/** The folder of a relative path that ends in the file's name; empty when it names no folder. */
function folderOf(relativePath: string): string {
  const cut = relativePath.lastIndexOf('/');
  return cut < 0 ? '' : relativePath.slice(0, cut);
}

/** The files of a picker (`<input type="file">`), with the folders a folder picker names. */
export function chosenFromPicker(files: Iterable<File>): ChosenFile[] {
  return Array.from(files, (file) => ({
    file,
    filename: file.name,
    folder: folderOf(file.webkitRelativePath),
  }));
}

/** The path Uppy's walk put on a dropped file: absent for a file dropped by itself. */
function droppedPath(file: File): string {
  const path: unknown = Reflect.get(file, 'relativePath');
  // The walk writes "folder/name"; a leading slash, which its own description shows, is not part
  // of a relative path.
  return typeof path === 'string' ? path.replace(/^\/+/, '') : '';
}

export interface Dropped {
  files: ChosenFile[];
  /** Files or folders of the drop that could not be read. */
  unreadable: number;
}

/**
 * A drag that carries files from the computer: a browser lists "Files" among its types, during the
 * drag as at the drop. A drag of something of the page itself (a row of the tree, a selection) does
 * not.
 */
export function carriesFiles(transfer: DataTransfer | null): boolean {
  return transfer !== null && Array.from(transfer.types).includes('Files');
}

/** The files of a drop, every dropped folder walked to its end. */
export async function chosenFromDrop(transfer: DataTransfer): Promise<Dropped> {
  let unreadable = 0;
  const files = await getDroppedFiles(transfer, {
    logDropError: () => {
      unreadable += 1;
    },
  });
  return {
    files: files.map((file) => ({
      file,
      filename: file.name,
      folder: folderOf(droppedPath(file)),
    })),
    unreadable,
  };
}

const UNITS = ['KB', 'MB', 'GB', 'TB'] as const;

/** A byte count in the units a file manager shows; the exact count belongs beside it. */
export function describeSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} ${bytes === 1 ? 'byte' : 'bytes'}`;
  let value = bytes;
  let unit: (typeof UNITS)[number] = 'KB';
  for (const next of UNITS) {
    value /= 1024;
    unit = next;
    if (value < 1024) break;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${unit}`;
}
