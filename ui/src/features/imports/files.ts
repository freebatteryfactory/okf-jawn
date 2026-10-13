/**
 * The files a person chose, each with the folder it came in: the relative path is preserved (SPEC
 * section 5: "Multi-file and folder upload preserves relative paths").
 *
 * Three ways in, and the folder each gives:
 * - the file picker: files with no folder;
 * - the folder picker: the browser names each file's path from the chosen folder down
 *   (`webkitRelativePath`, "reports/2025/a.pdf"), so the folder is everything before the name;
 * - a drop of files and folders: each dropped thing is asked for its entry (the browsers' entry
 *   API, `webkitGetAsEntry`), and a folder is walked here, every directory read until the browser
 *   answers an empty batch, each file named by the folders above it.
 *
 * What a drop can hold that is not a file to send, and what becomes of it (`Dropped`):
 * - a file the browser names and cannot hand over (a dangling link, a file that is gone): it costs
 *   only itself. It is returned by its path as unreadable, to be listed as such and never sent;
 *   the walk goes on with the next entry;
 * - a folder with nothing in it: returned by its path, to be said; nothing is kept of it, since
 *   only files are uploaded;
 * - a folder whose entries the browser could not list, or not all of them: returned by its path;
 *   what was read of it before the failure is kept;
 * - a folder is not returned as a file: a browser also offers a dropped folder as a File of its
 *   name (`getAsFile`), and that File is used only for a dropped thing whose entry says it is a
 *   file, or that has no entry at all.
 *
 * Uppy's `getDroppedFiles` did the walk before. It gives up the whole walk of a dropped folder at
 * the first file it cannot read and then lists the folder itself as a file, so it is not used any
 * more.
 *
 * The name is the file's own, exactly; the folder is exactly the path the browser gave, with `/`
 * between its parts. Nothing is read from a file here.
 */

/** A file's place: its own name, and the folder it came in. */
export interface NamedPath {
  /** The file's own name. */
  filename: string;
  /** The folder it came in, relative to what was chosen; empty for a file chosen by itself. */
  folder: string;
}

export interface ChosenFile extends NamedPath {
  file: File;
}

/** A file's path as it is shown and kept: its folder, then its name. */
export function pathOf(chosen: NamedPath): string {
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

/**
 * A drag that carries files from the computer: a browser lists "Files" among its types, during the
 * drag as at the drop. A drag of something of the page itself (a row of the tree, a selection) does
 * not.
 */
export function carriesFiles(transfer: DataTransfer | null): boolean {
  return transfer !== null && Array.from(transfer.types).includes('Files');
}

export interface Dropped {
  files: ChosenFile[];
  /** Files the browser named and could not hand over, each by its path. */
  unreadable: NamedPath[];
  /** Folders of the drop, at any depth, that hold nothing. */
  emptyFolders: string[];
  /** Folders whose entries could not be listed, or not all of them. */
  unlistedFolders: string[];
}

const isFileEntry = (entry: FileSystemEntry): entry is FileSystemFileEntry => entry.isFile;
const isFolderEntry = (entry: FileSystemEntry): entry is FileSystemDirectoryEntry =>
  entry.isDirectory;

function fileOfEntry(entry: FileSystemFileEntry): Promise<File> {
  return new Promise((resolve, reject) => entry.file(resolve, reject));
}

/**
 * The entries of a folder. A browser gives them in batches and says the end with an empty one;
 * `complete` is false when a batch failed, with what was read before it.
 */
async function entriesOf(
  folder: FileSystemDirectoryEntry,
): Promise<{ entries: FileSystemEntry[]; complete: boolean }> {
  const reader = folder.createReader();
  const entries: FileSystemEntry[] = [];
  for (;;) {
    let batch: FileSystemEntry[];
    try {
      batch = await new Promise<FileSystemEntry[]>((resolve, reject) =>
        reader.readEntries(resolve, reject),
      );
    } catch {
      return { entries, complete: false };
    }
    if (batch.length === 0) return { entries, complete: true };
    entries.push(...batch);
  }
}

/**
 * Walk one entry of a drop into `found`. `offered` is the File the browser offers for a thing
 * dropped by itself: used when the entry is a file that cannot be handed over as an entry (a link
 * to a file, in Chromium), never for a folder.
 */
async function walk(
  entry: FileSystemEntry,
  folder: string,
  found: Dropped,
  offered: File | null,
): Promise<void> {
  if (isFileEntry(entry)) {
    try {
      const file = await fileOfEntry(entry);
      found.files.push({ file, filename: file.name, folder });
    } catch {
      if (offered !== null) found.files.push({ file: offered, filename: offered.name, folder });
      else found.unreadable.push({ filename: entry.name, folder });
    }
    return;
  }
  if (!isFolderEntry(entry)) return;
  const path = folder === '' ? entry.name : `${folder}/${entry.name}`;
  const { entries, complete } = await entriesOf(entry);
  if (!complete) found.unlistedFolders.push(path);
  else if (entries.length === 0) found.emptyFolders.push(path);
  // One after another: what fails inside one entry is that entry's own.
  for (const child of entries) await walk(child, path, found, null);
}

/** The files of a drop, every dropped folder walked to its end. */
export async function chosenFromDrop(transfer: DataTransfer): Promise<Dropped> {
  const found: Dropped = { files: [], unreadable: [], emptyFolders: [], unlistedFolders: [] };
  // What the drag carries is asked for in this same turn: a browser empties it once the drop's
  // handler has returned.
  // (A thing that is no file, such as dragged text, has neither an entry nor a File.)
  const carried = Array.from(transfer.items, (item) => ({
    entry: item.webkitGetAsEntry(),
    offered: item.getAsFile(),
  }));
  for (const { entry, offered } of carried) {
    if (entry !== null) await walk(entry, '', found, offered);
    // No entry: the browser knows the thing only as a File (a drag a page made, an older browser).
    else if (offered !== null) {
      found.files.push({ file: offered, filename: offered.name, folder: '' });
    }
  }
  return found;
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
