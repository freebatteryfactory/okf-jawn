/**
 * The walk of a drop (`chosenFromDrop`): what the drag carries is asked for in the turn of the
 * drop, and a dropped folder is read to its end within the bounds the page keeps.
 */
import { describe, expect, it } from 'vitest';
import {
  chosenFromDrop,
  MAX_DROP_DEPTH,
  MAX_DROP_FILES,
  PROGRESS_EVERY,
  pathOf,
} from '../../src/features/imports/files';
import { type DroppedEntry, droppedFile, droppedFolder, fileOf } from './support/imports';

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
