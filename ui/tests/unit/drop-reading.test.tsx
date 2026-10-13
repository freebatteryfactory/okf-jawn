/**
 * The import page while a dropped folder is read, and after it was read to a bound: a polite
 * status says the folder is being read and how many files were found so far, and a drop that
 * holds more than the page reads says so plainly.
 */
import { act, cleanup, configure, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MAX_DROP_DEPTH, MAX_DROP_FILES, PROGRESS_EVERY } from '../../src/features/imports/files';
import { resetUploads } from '../../src/features/imports/uploading';
import { mockApi, ok } from './support/api';
import { type DroppedEntry, dropOf, droppedFile, droppedFolder, fileOf } from './support/imports';
import { explorerHandlers, renderAt, WS, writableWorkspace } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function dropOn(entries: DroppedEntry[]) {
  mockApi(explorerHandlers({ '/api/workspaces/open-workspace': () => ok(writableWorkspace) }));
  renderAt(`/w/${WS}/import`);
  await screen.findByRole('treeitem', { name: 'notes' });
  const target = screen.getByRole('group', { name: 'Choose what to import' });
  const drop = dropOf(entries);
  fireEvent.dragEnter(target, { dataTransfer: drop });
  fireEvent.dragOver(target, { dataTransfer: drop });
  fireEvent.drop(target, { dataTransfer: drop });
}

/** The status under the drop box, where choosing is announced. */
const choiceNote = () =>
  screen.getByRole('group', { name: 'Choose what to import' }).nextElementSibling?.textContent;

const plain = (count: number, prefix = 'f') =>
  Array.from({ length: count }, (_, index) => droppedFile(fileOf(`${prefix}${index}.txt`, 'a')));

describe('while a dropped folder is read', () => {
  it('says so, and how many files were found so far, not at every file', async () => {
    // The last file of the folder is not handed over until the test lets it go.
    let release: () => void = () => undefined;
    const held = fileOf('last.txt', 'a');
    const gated: DroppedEntry = {
      isFile: true,
      isDirectory: false,
      name: 'last.txt',
      offered: held,
      file: (found) => {
        release = () => found(held);
      },
    };
    await dropOn([droppedFolder('docs', [...plain(PROGRESS_EVERY + 10), gated])]);
    await waitFor(() =>
      expect(choiceNote()).toBe(
        `Reading the dropped folder… ${PROGRESS_EVERY} files found so far.`,
      ),
    );
    // Between two reports the words do not change.
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(choiceNote()).toBe(`Reading the dropped folder… ${PROGRESS_EVERY} files found so far.`);
    act(() => release());
    await waitFor(() => expect(choiceNote()).toBe(`${PROGRESS_EVERY + 11} files added.`));
  });

  it('says only that it is reading before the first report', async () => {
    let release: () => void = () => undefined;
    const held = fileOf('only.txt', 'a');
    await dropOn([
      droppedFolder('docs', [
        {
          isFile: true,
          isDirectory: false,
          name: 'only.txt',
          offered: held,
          file: (found) => {
            release = () => found(held);
          },
        },
      ]),
    ]);
    await waitFor(() => expect(choiceNote()).toBe('Reading the dropped folder…'));
    act(() => release());
    await waitFor(() => expect(choiceNote()).toBe('1 file added.'));
  });
});

describe('a drop that holds more than the page reads', () => {
  it('lists the first files and says the folder holds more than is read at once', async () => {
    await dropOn([droppedFolder('big', plain(MAX_DROP_FILES + 1))]);
    await waitFor(() => expect(choiceNote()).toContain('files added.'), { timeout: 30_000 });
    expect(choiceNote()).toBe(
      `${MAX_DROP_FILES} files added. The drop holds more than this page reads at once: only the first 5,000 files are listed, and the rest were not read. Drop the rest in smaller parts.`,
    );
    expect(
      screen.getByRole('heading', { name: `Files to import (${MAX_DROP_FILES})` }),
    ).toBeDefined();
  }, 60_000);

  it('says that folders nested too deeply were not read', async () => {
    let inner: DroppedEntry = droppedFile(fileOf('bottom.txt', 'a'));
    for (let level = MAX_DROP_DEPTH + 1; level >= 0; level -= 1) {
      inner = droppedFolder(`d${level}`, [inner, droppedFile(fileOf(`top${level}.txt`, 'a'))]);
    }
    await dropOn([inner]);
    await waitFor(() => expect(choiceNote()).toContain('added.'));
    expect(choiceNote()).toContain(
      `Folders nested more than ${MAX_DROP_DEPTH} levels deep were not read.`,
    );
  });

  it('says nothing of a bound for a drop within them', async () => {
    await dropOn([droppedFolder('small', plain(3))]);
    await waitFor(() => expect(choiceNote()).toBe('3 files added.'));
  });
});
