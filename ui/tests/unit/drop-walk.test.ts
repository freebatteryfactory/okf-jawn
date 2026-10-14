/**
 * The walk of a drop (`chosenFromDrop`): what the drag carries is asked for in the turn of the
 * drop, and a dropped folder is read to its end within the bounds the page keeps.
 */
import { describe, expect, it } from 'vitest';
import {
  chosenFromDrop,
  MAX_DROP_DEPTH,
  MAX_DROP_ENTRIES,
  MAX_DROP_FILES,
  PROGRESS_EVERY,
  pathOf,
} from '../../src/features/imports/files';
import {
  type DroppedEntry,
  droppedFile,
  droppedFolder,
  fileOf,
  unreadableFile,
} from './support/imports';

/** Let a task of the browser's own go by. */
const later = (ms = 0) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** A file whose bytes take a task of the browser to arrive, as a real file's do. */
function slowFile(name: string): DroppedEntry {
  const file = fileOf(name, 'abc');
  return {
    isFile: true,
    isDirectory: false,
    name,
    offered: file,
    file: (found) => {
      setTimeout(() => found(file), 5);
    },
  };
}

/**
 * A drag as a browser hands it to the drop's handler: it answers only until the handler has
 * returned (the next task), after which every item answers nothing.
 */
function emptiedAfterTheTurn(entries: DroppedEntry[]) {
  let open = true;
  const items = entries.map((entry) => ({
    kind: 'file',
    webkitGetAsEntry: () => (open ? entry : null),
    getAsFile: () => (open ? entry.offered : null),
  }));
  setTimeout(() => {
    open = false;
  }, 0);
  return { items } as unknown as DataTransfer;
}

describe('the things of a drop', () => {
  it('are all asked for in the turn of the drop: two files and a folder are all listed', async () => {
    const found = await chosenFromDrop(
      emptiedAfterTheTurn([
        slowFile('one.txt'),
        droppedFolder('docs', [slowFile('a.txt'), slowFile('b.txt')]),
        slowFile('two.txt'),
      ]),
    );
    expect(found.files.map(pathOf).sort()).toEqual([
      'docs/a.txt',
      'docs/b.txt',
      'one.txt',
      'two.txt',
    ]);
  });

  it('lists what a plain drop holds without the wait', async () => {
    const found = await chosenFromDrop(emptiedAfterTheTurn([droppedFile(fileOf('x.txt', 'abc'))]));
    expect(found.files.map(pathOf)).toEqual(['x.txt']);
    await later();
  });
});

/** `count` plain files, named `prefix0`, `prefix1`, … */
const manyFiles = (count: number, prefix = 'f') =>
  Array.from({ length: count }, (_, index) => droppedFile(fileOf(`${prefix}${index}.txt`, 'a')));

/** A drop of one thing, answering at once. */
const dropOfEntries = (entries: DroppedEntry[]) =>
  ({
    items: entries.map((entry) => ({
      kind: 'file',
      webkitGetAsEntry: () => entry,
      getAsFile: () => entry.offered,
    })),
  }) as unknown as DataTransfer;

/** Folders nested `levels` deep (the first is the dropped one, level 0), a file in the last. */
function chain(levels: number): DroppedEntry {
  let inner: DroppedEntry = droppedFile(fileOf('bottom.txt', 'a'));
  for (let level = levels - 1; level >= 0; level -= 1) {
    inner = droppedFolder(`d${level}`, [inner]);
  }
  return inner;
}

describe('the bounds of the walk of a drop', () => {
  it('are the numbers the page was written for', () => {
    expect(MAX_DROP_FILES).toBe(5_000);
    expect(MAX_DROP_DEPTH).toBe(32);
    expect(MAX_DROP_ENTRIES).toBe(20_000);
  });

  it('read exactly the most files and say nothing of more', async () => {
    const found = await chosenFromDrop(
      dropOfEntries([droppedFolder('big', manyFiles(MAX_DROP_FILES))]),
    );
    expect(found.files).toHaveLength(MAX_DROP_FILES);
    expect(found.tooMany).toBe(false);
  });

  it('stop one file over: the first files are listed and the drop says there was more', async () => {
    const found = await chosenFromDrop(
      dropOfEntries([droppedFolder('big', manyFiles(MAX_DROP_FILES + 1))]),
    );
    expect(found.files).toHaveLength(MAX_DROP_FILES);
    expect(found.tooMany).toBe(true);
    expect(found.files.at(0)?.filename).toBe('f0.txt');
  });

  it('count the files of every folder together, not each folder alone', async () => {
    const third = Math.ceil((MAX_DROP_FILES + 1) / 3);
    const found = await chosenFromDrop(
      dropOfEntries([
        droppedFolder('a', manyFiles(third, 'a')),
        droppedFolder('b', manyFiles(third, 'b')),
        droppedFolder('c', manyFiles(third, 'c')),
        ...manyFiles(3, 'loose'),
      ]),
    );
    expect(found.files).toHaveLength(MAX_DROP_FILES);
    expect(found.tooMany).toBe(true);
  });

  it('read a folder at the deepest level and not the one below', async () => {
    const atLimit = await chosenFromDrop(dropOfEntries([chain(MAX_DROP_DEPTH + 1)]));
    expect(atLimit.files.map(pathOf)).toHaveLength(1);
    expect(atLimit.tooDeep).toBe(false);
    const past = await chosenFromDrop(dropOfEntries([chain(MAX_DROP_DEPTH + 2)]));
    expect(past.files).toEqual([]);
    expect(past.tooDeep).toBe(true);
  });
});

describe('a walk past the bound', () => {
  it('reads no further folder and says nothing of one it did not read', async () => {
    const found = await chosenFromDrop(
      dropOfEntries([
        droppedFolder('big', manyFiles(MAX_DROP_FILES + 1)),
        droppedFolder('later', []),
      ]),
    );
    expect(found.tooMany).toBe(true);
    expect(found.emptyFolders).toEqual([]);
    expect(found.unlistedFolders).toEqual([]);
  });
});

/** What a fake tree was asked for: the folders whose entries were asked for, and the entries given. */
interface Reads {
  folders: number;
  entries: number;
}

const noReads = (): Reads => ({ folders: 0, entries: 0 });

/** Entries to a batch, as Chromium's reader gives them. */
const BATCH = 100;

/** A folder that counts what the walk asks of it, in `reads`; its entries come in batches. */
function watchedFolder(name: string, children: DroppedEntry[], reads: Reads): DroppedEntry {
  return {
    isFile: false,
    isDirectory: true,
    name,
    offered: new File([], name),
    createReader: () => {
      reads.folders += 1;
      const rest = [...children];
      return {
        readEntries: (found) => {
          const batch = rest.splice(0, BATCH);
          reads.entries += batch.length;
          found(batch);
        },
      };
    },
  };
}

/**
 * `total` folders and no file: the first, "root", holds groups, and a group holds up to `BATCH`
 * empty folders. Far more folders than a person keeps in a drop, and none of them a file.
 */
function folderTree(total: number, reads: Reads): DroppedEntry {
  const groups: DroppedEntry[] = [];
  let left = total - 1;
  for (let group = 0; left > 0; group += 1) {
    left -= 1;
    const size = Math.min(BATCH, left);
    left -= size;
    const empty = Array.from({ length: size }, (_, index) => watchedFolder(`e${index}`, [], reads));
    groups.push(watchedFolder(`g${group}`, empty, reads));
  }
  return watchedFolder('root', groups, reads);
}

describe('the walk of a tree of folders that hold no files', () => {
  it('stops at the most entries it visits, lists what it found and says there was more', async () => {
    const reads = noReads();
    const found = await chosenFromDrop(
      dropOfEntries([
        folderTree(MAX_DROP_ENTRIES + 2_000, reads),
        watchedFolder('later', [], reads),
      ]),
    );
    // Every folder visited was read, and none after the budget was spent: not the rest of the
    // tree, and not the folder dropped after it.
    expect(reads.folders).toBe(MAX_DROP_ENTRIES);
    expect(found.tooMany).toBe(true);
    expect(found.tooDeep).toBe(false);
    expect(found.files).toEqual([]);
    expect(found.emptyFolders.at(0)).toBe('root/g0/e0');
    expect(found.emptyFolders.length).toBeGreaterThan(0);
    expect(found.emptyFolders.length).toBeLessThan(MAX_DROP_ENTRIES);
    expect(found.emptyFolders).not.toContain('later');
  }, 20_000);

  it('read a tree of exactly the budget to its end and say nothing of more', async () => {
    const reads = noReads();
    const found = await chosenFromDrop(dropOfEntries([folderTree(MAX_DROP_ENTRIES, reads)]));
    expect(reads.folders).toBe(MAX_DROP_ENTRIES);
    expect(found.tooMany).toBe(false);
    expect(found.emptyFolders.at(-1)).toBeDefined();
  }, 20_000);

  it('read no folder dropped after the budget is spent and say there was more', async () => {
    const reads = noReads();
    const found = await chosenFromDrop(
      dropOfEntries([folderTree(MAX_DROP_ENTRIES, reads), watchedFolder('later', [], reads)]),
    );
    // The tree used the budget up to its last folder, and it was read in full (nothing of it was
    // refused): it is the folder after it that the budget stops, and only the budget says so.
    expect(reads.folders).toBe(MAX_DROP_ENTRIES);
    expect(found.tooMany).toBe(true);
    expect(found.emptyFolders).not.toContain('later');
  }, 20_000);

  it('stop one folder over the budget and say there was more', async () => {
    const reads = noReads();
    const found = await chosenFromDrop(dropOfEntries([folderTree(MAX_DROP_ENTRIES + 1, reads)]));
    expect(reads.folders).toBe(MAX_DROP_ENTRIES);
    expect(found.tooMany).toBe(true);
  }, 20_000);

  it('does not call a folder empty when the budget is spent on the folder itself', async () => {
    const reads = noReads();
    const last = watchedFolder('last', [watchedFolder('inside', [], reads)], reads);
    const found = await chosenFromDrop(
      dropOfEntries([folderTree(MAX_DROP_ENTRIES - 1, reads), last]),
    );
    // "last" is the 20,000th entry: it is read, it holds something, and there is no room for it.
    expect(reads.folders).toBe(MAX_DROP_ENTRIES);
    expect(found.tooMany).toBe(true);
    expect(found.emptyFolders).not.toContain('last');
  }, 20_000);

  it('read no more of a folder than the walk may still visit', async () => {
    const reads = noReads();
    const wide = Array.from({ length: MAX_DROP_FILES - 1 }, (_, index) =>
      watchedFolder(`w${index}`, [], reads),
    );
    // 17,000 folders spend most of the budget; the 4,999 of "wide" are more than is left, and
    // the page has no use for the entries of it that it will not visit.
    const found = await chosenFromDrop(
      dropOfEntries([folderTree(17_000, reads), watchedFolder('wide', wide, reads)]),
    );
    expect(found.tooMany).toBe(true);
    expect(reads.folders).toBe(MAX_DROP_ENTRIES);
    expect(reads.entries).toBeLessThanOrEqual(MAX_DROP_ENTRIES);
  }, 20_000);
});

describe('the budget of entries', () => {
  /** A tree of `folders` folders and, beside it, a folder of `files` files: all of them counted. */
  const mixed = (folders: number, files: number) =>
    dropOfEntries([folderTree(folders, noReads()), droppedFolder('files', manyFiles(files, 'm'))]);

  it('counts files and folders together: one entry over, the last file is not read', async () => {
    // 17,000 folders, the folder "files" and 3,000 files in it: one more than the budget.
    const found = await chosenFromDrop(mixed(17_000, 3_000));
    expect(found.tooMany).toBe(true);
    expect(found.files).toHaveLength(2_999);
  }, 20_000);

  it('counts files and folders together: exactly the budget is read to its end', async () => {
    const found = await chosenFromDrop(mixed(16_999, 3_000));
    expect(found.tooMany).toBe(false);
    expect(found.files).toHaveLength(3_000);
  }, 20_000);

  it('bounds the files that could not be read as it bounds the rest', async () => {
    const folders = Array.from({ length: 5 }, (_, index) =>
      droppedFolder(
        `u${index}`,
        Array.from({ length: MAX_DROP_FILES }, (_, file) => unreadableFile(`x${file}`)),
      ),
    );
    const found = await chosenFromDrop(dropOfEntries(folders));
    // 25,000 files that cannot be read: none of them counts as a file, so only the budget stops
    // the walk, after four folders (one entry each) and the files of them it still may visit.
    expect(found.tooMany).toBe(true);
    expect(found.files).toEqual([]);
    expect(found.unreadable).toHaveLength(MAX_DROP_ENTRIES - 4);
  }, 20_000);
});

describe('the progress of a walk', () => {
  it('is told when a folder is begun, and every PROGRESS_EVERY files, not each file', async () => {
    const told: number[] = [];
    await chosenFromDrop(
      dropOfEntries([droppedFolder('docs', manyFiles(PROGRESS_EVERY * 2 + 10))]),
      (found) => told.push(found),
    );
    expect(told).toEqual([0, PROGRESS_EVERY, PROGRESS_EVERY * 2]);
  });

  it('is not told for files dropped by themselves', async () => {
    const told: number[] = [];
    await chosenFromDrop(dropOfEntries(manyFiles(PROGRESS_EVERY + 5)), (found) => told.push(found));
    expect(told).toEqual([PROGRESS_EVERY]);
  });
});
