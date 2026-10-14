/**
 * The folder tree's drag backend is scoped to the tree's own box on every mount, a mount whose
 * root listing is already cached included (leave the workspace for "/" and come back). Left on the
 * window the backend cancels every text drag on the page, a text drop in the draft editor among
 * them. A drag of files is still cancelled, by the root guard, wherever it lands.
 */
import { act, cleanup, configure, createEvent, fireEvent, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mockApi, ok } from './support/api';
import { explorerHandlers, renderAt, WS, writableWorkspace } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function transfer(types: string[]) {
  return { types, items: [], files: [], dropEffect: 'none', effectAllowed: 'all' };
}

/** Whether a drag over and a drop on the page body were cancelled. */
function cancelled(types: string[]): { over: boolean; drop: boolean } {
  const target = document.body;
  fireEvent.dragEnter(target, { dataTransfer: transfer(types) });
  const over = createEvent.dragOver(target, { dataTransfer: transfer(types) });
  fireEvent(target, over);
  const drop = createEvent.drop(target, { dataTransfer: transfer(types) });
  fireEvent(target, drop);
  return { over: over.defaultPrevented, drop: drop.defaultPrevented };
}

async function mounted() {
  mockApi(explorerHandlers({ '/api/workspaces/open-workspace': () => ok(writableWorkspace) }));
  const view = renderAt(`/w/${WS}`);
  await screen.findAllByRole('treeitem');
  return view;
}

/** Leave the workspace for "/" and return, so the tree mounts with its listing cached. */
async function remounted() {
  const { router } = await mounted();
  await act(async () => {
    await router.navigate({ to: '/' as never });
  });
  expect(screen.queryAllByRole('treeitem')).toHaveLength(0);
  await act(async () => {
    await router.navigate({ to: `/w/${WS}` as never });
  });
  await screen.findAllByRole('treeitem');
}

describe('the folder tree drag backend', () => {
  it('leaves a text drag outside the tree alone on a fresh mount', async () => {
    await mounted();
    expect(cancelled(['text/plain'])).toEqual({ over: false, drop: false });
  });

  it('leaves a text drag outside the tree alone on a mount with the listing cached', async () => {
    await remounted();
    expect(cancelled(['text/plain'])).toEqual({ over: false, drop: false });
  });

  it('still cancels a drag of files outside the tree, on either mount', async () => {
    await mounted();
    expect(cancelled(['Files'])).toEqual({ over: true, drop: true });
    cleanup();
    await remounted();
    expect(cancelled(['Files'])).toEqual({ over: true, drop: true });
  });

  it('takes no drag of its own entries: a row is not draggable', async () => {
    await remounted();
    for (const row of screen.getAllByRole('treeitem')) {
      expect(row.getAttribute('draggable')).not.toBe('true');
    }
  });

  it('keeps keyboard focus on the row it was on once the tree has settled', async () => {
    await remounted();
    const row = screen.getAllByRole('treeitem')[0];
    if (row === undefined) throw new Error('no tree row');
    act(() => row.focus());
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.activeElement).toBe(row);
    expect(row.isConnected).toBe(true);
  });
});
