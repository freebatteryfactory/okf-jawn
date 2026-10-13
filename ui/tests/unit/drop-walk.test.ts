/**
 * The walk of a drop (`chosenFromDrop`): what the drag carries is asked for in the turn of the
 * drop, and a dropped folder is read to its end within the bounds the page keeps.
 */
import { describe, expect, it } from 'vitest';
import { chosenFromDrop, pathOf } from '../../src/features/imports/files';
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
