/**
 * A file dropped outside the import page's drop box never opens in place of the app: the
 * application's root cancels the drag and the drop of files on every route, whatever the folder
 * tree shows. A drag that carries no file is not touched, and nothing but the drop box takes a
 * file.
 *
 * What a browser does with an uncancelled drop of a file (it leaves the page for the file) is the
 * browser's and cannot be run here; what is pinned is that the `dragover` and the `drop` are
 * cancelled, which is what prevents it.
 */
import { act, cleanup, configure, createEvent, fireEvent, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useUploads } from '../../src/features/imports/upload-list';
import { resetUploads } from '../../src/features/imports/uploading';
import { fail, type Handler, mockApi, ok } from './support/api';
import { CREATE, dropOf, droppedFile, fileOf } from './support/imports';
import {
  explorerHandlers,
  listing,
  NOTE,
  renderAt,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const LIST = '/api/items/list-items';
const WORKSPACES = '/api/workspaces/list-workspaces';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const serve = (overrides: Record<string, Handler> = {}) =>
  mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace), ...overrides }));

/** No entry in the tree: react-arborist, whose drag backend cancels drags too, is not mounted. */
const EMPTY_TREE = { [LIST]: () => ok(listing([], [])) };

/**
 * A whole drag over `target`, as a browser delivers one: it enters, moves over, and is let go.
 * Answers whether the page cancelled the move and the drop.
 */
function dragOnto(target: Element, carried: unknown) {
  fireEvent.dragEnter(target, { dataTransfer: carried });
  const over = createEvent.dragOver(target, { dataTransfer: carried });
  fireEvent(target, over);
  const dropped = createEvent.drop(target, { dataTransfer: carried });
  fireEvent(target, dropped);
  return { over: over.defaultPrevented, drop: dropped.defaultPrevented };
}

const files = () => dropOf([droppedFile(fileOf('x.txt', 'abc'))]);
/** What a drag of something of the page carries: no file. */
const notFiles = () => ({
  types: ['text/plain'],
  items: [],
  files: [],
  dropEffect: 'none',
  effectAllowed: 'all',
});
const CANCELLED = { over: true, drop: true };
const UNTOUCHED = { over: false, drop: false };

/** Nothing was listed and nothing was asked of the upload service. */
async function expectNothingTaken(api: ReturnType<typeof mockApi>) {
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
  expect(useUploads.getState().entries).toEqual([]);
  expect(api.callsTo(CREATE)).toEqual([]);
}

describe('a file let go outside the drop box', () => {
  it('is cancelled in an empty workspace, where the first import happens', async () => {
    const api = serve(EMPTY_TREE);
    renderAt(`/w/${WS}/import`);
    const outside = await screen.findByRole('heading', { name: 'Import files' });
    await screen.findByText('This workspace has no folders or items yet.');
    expect(screen.queryByRole('treeitem')).toBeNull();
    expect(dragOnto(outside, files())).toEqual(CANCELLED);
    expect(screen.getByText('No files are listed yet.')).toBeDefined();
    await expectNothingTaken(api);
  });

  it('is cancelled while the tree could not be listed', async () => {
    const api = serve({ [LIST]: () => fail(500, 'internal', 'the listing failed') });
    renderAt(`/w/${WS}/import`);
    const outside = await screen.findByRole('heading', { name: 'Import files' });
    await screen.findByText(/Something went wrong in the service\./);
    expect(screen.queryByRole('treeitem')).toBeNull();
    expect(dragOnto(outside, files())).toEqual(CANCELLED);
    await expectNothingTaken(api);
  });

  it('is cancelled on the list of workspaces', async () => {
    const api = mockApi({ [WORKSPACES]: () => ok({ items: [workspace], next_cursor: null }) });
    renderAt('/');
    const link = await screen.findByRole('link', { name: 'Workshop notes' });
    expect(dragOnto(link, files())).toEqual(CANCELLED);
    expect(dragOnto(document.body, files())).toEqual(CANCELLED);
    await expectNothingTaken(api);
  });

  it('is cancelled in a pane, and the item stays as it was', async () => {
    const api = serve(EMPTY_TREE);
    renderAt(`/w/${WS}/i/${NOTE}`);
    const text = await screen.findByText('Text of Meeting notes');
    expect(dragOnto(text, files())).toEqual(CANCELLED);
    expect(dragOnto(screen.getByRole('tab', { name: 'Meeting notes' }), files())).toEqual(
      CANCELLED,
    );
    expect(screen.getByText('Text of Meeting notes')).toBeDefined();
    await expectNothingTaken(api);
  });

  it('is cancelled on the tree, which takes no file', async () => {
    const api = serve();
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    const row = await screen.findByRole('treeitem', { name: 'notes' });
    expect(dragOnto(row, files())).toEqual(CANCELLED);
    expect(screen.getByText('No files are listed yet.')).toBeDefined();
    await expectNothingTaken(api);
  });
});

describe('a drag that carries no file', () => {
  it('is not touched by the page: it is not cancelled where nothing takes it', async () => {
    const api = serve(EMPTY_TREE);
    renderAt(`/w/${WS}/import`);
    const outside = await screen.findByRole('heading', { name: 'Import files' });
    await screen.findByText('This workspace has no folders or items yet.');
    expect(dragOnto(outside, notFiles())).toEqual(UNTOUCHED);
    await expectNothingTaken(api);
  });

  it('is not taken by the drop box: nothing is listed, said or marked, and no upload starts', async () => {
    const api = serve(EMPTY_TREE);
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    await screen.findByText('This workspace has no folders or items yet.');
    const box = screen.getByRole('group', { name: 'Choose what to import' });
    // As a row of the folder tree dragged onto the box: an entry of the page, not a file.
    expect(dragOnto(box, notFiles())).toEqual(UNTOUCHED);
    expect(box.className).toBe('drop-target');
    await expectNothingTaken(api);
    expect(screen.queryByText(/added to the list|added\./)).toBeNull();
  });

  it('while a drag of files over the box is cancelled, marks the box and sets the pointer to copy', async () => {
    serve(EMPTY_TREE);
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    const box = screen.getByRole('group', { name: 'Choose what to import' });
    // The drag's own effect is left for the page to set, as in a browser.
    const { dropEffect: _unset, ...carried } = files();
    const over = createEvent.dragOver(box, { dataTransfer: carried });
    fireEvent(box, over);
    expect(over.defaultPrevented).toBe(true);
    expect(box.className).toBe('drop-target drop-target-over');
    // The pointer says the files are taken here.
    expect(over instanceof DragEvent ? over.dataTransfer?.dropEffect : null).toBe('copy');
  });
});

describe('the root guard once the page is gone', () => {
  // happy-dom ignores the capture flag when it removes a listener, so what a browser would do is
  // pinned by what the guard asks for: it removes each listener it added, in the same phase.
  it('removes both listeners in the phase it added them in', async () => {
    serve(EMPTY_TREE);
    const added = vi.spyOn(window, 'addEventListener');
    const removed = vi.spyOn(window, 'removeEventListener');
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    cleanup();
    for (const type of ['dragover', 'drop']) {
      const add = added.mock.calls.find(([name]) => name === type);
      const remove = removed.mock.calls.find(([name]) => name === type);
      expect(add?.[2]).toEqual({ capture: true });
      expect(remove?.[1]).toBe(add?.[1]);
      expect(remove?.[2]).toEqual({ capture: true });
    }
  });
});
