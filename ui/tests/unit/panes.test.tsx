/** Tabs and split panes: keyboard use, roles and names, URL ownership, and no identity copied into the store. */

import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useExplorerStore } from '../../src/features/explorer/store';
import { fail, mockApi, notImplemented } from './support/api';
import { explorerHandlers, NOTE, OTHER, renderAt, WS } from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

async function openTwoTabs() {
  const view = renderAt(`/w/${WS}/i/${NOTE}`);
  await screen.findByRole('tab', { name: 'Meeting notes', selected: true });
  const tree = await screen.findByRole('tree', { name: 'Folders' });
  await userEvent.click(await within(tree).findByRole('treeitem', { name: /Plan/ }));
  await screen.findByRole('tab', { name: 'Plan', selected: true });
  return view;
}

describe('tabs and panes', () => {
  it('names every pane control and resizer for assistive technology', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    expect(await screen.findByRole('tablist', { name: 'Open items' })).toBeTruthy();
    expect(screen.getByRole('region', { name: 'Documents' })).toBeTruthy();
    expect(screen.getByRole('separator', { name: 'Resize the navigation column' })).toBeTruthy();
    expect(screen.getByRole('separator', { name: 'Resize the source panel' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Split right' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Close tab' })).toBeTruthy();
    expect(screen.getByRole('searchbox', { name: 'Search this workspace' })).toBeTruthy();
  });

  it('moves between tabs with the keyboard, and the URL follows the active tab', async () => {
    mockApi(explorerHandlers());
    const { router } = await openTwoTabs();
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`);
    screen.getByRole('tab', { name: 'Plan' }).focus();
    await userEvent.keyboard('{ArrowLeft}');
    expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'Meeting notes' }));
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(await screen.findByRole('tab', { name: 'Meeting notes', selected: true })).toBeTruthy();
    expect(await screen.findByRole('article', { name: 'Meeting notes' })).toBeTruthy();
  });

  it('closes the focused tab with Delete and shows the next one', async () => {
    mockApi(explorerHandlers());
    const { router } = await openTwoTabs();
    screen.getByRole('tab', { name: 'Plan' }).focus();
    await userEvent.keyboard('{Delete}');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(screen.queryByRole('tab', { name: 'Plan' })).toBeNull();
    expect(await screen.findByRole('tab', { name: 'Meeting notes', selected: true })).toBeTruthy();
  });

  it('splits into resizable panes; focusing a pane makes it the one the URL describes', async () => {
    mockApi(explorerHandlers());
    const { router } = await openTwoTabs();
    await userEvent.click(screen.getByRole('button', { name: 'Split right' }));
    const first = await screen.findByRole('region', { name: 'Documents, pane 1 of 2' });
    const second = screen.getByRole('region', { name: 'Documents, pane 2 of 2' });
    expect(screen.getByRole('separator', { name: 'Resize panes 1 and 2' })).toBeTruthy();
    expect(within(second).getByRole('tab', { name: 'Plan', selected: true })).toBeTruthy();
    within(first).getByRole('tab', { name: 'Plan' }).focus();
    await userEvent.keyboard('{ArrowLeft}{Enter}');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(within(first).getByRole('tab', { name: 'Meeting notes', selected: true })).toBeTruthy();
    expect(within(second).getByRole('tab', { name: 'Plan', selected: true })).toBeTruthy();
    within(second).getByRole('tab', { name: 'Plan' }).focus();
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`));
  });

  it('keeps only item references in the Explorer store, never titles, paths or revisions', async () => {
    mockApi(explorerHandlers());
    await openTwoTabs();
    const stored = JSON.stringify(useExplorerStore.getState());
    for (const copied of [
      'Meeting notes',
      'Plan',
      'notes/meeting.md',
      'notes/plan.md',
      'aaaaaaaaaa',
    ]) {
      expect(stored).not.toContain(copied);
    }
    expect(stored).toContain(NOTE);
    expect(stored).toContain(OTHER);
  });
});

describe('failures keep the workspace open', () => {
  it('keeps the open tabs when a not_found sends the reader back to the workspace', async () => {
    const base = explorerHandlers();
    mockApi({
      ...base,
      '/api/reads/read-item': (body) =>
        body.item_id === OTHER
          ? fail(404, 'not_found', 'no such item')
          : (base['/api/reads/read-item']?.(body) ?? notImplemented('read_item')),
    });
    const { router } = await openTwoTabs();
    const article = await screen.findByRole('article', { name: 'Plan' });
    const alert = await within(article).findByRole('alert');
    expect(alert.textContent).toContain('Reading this item: This could not be found.');
    await userEvent.click(within(alert).getByRole('link', { name: 'Go to this workspace' }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}`));
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Plan' })).toBeTruthy();
  });
});

describe('the URL owns the focused pane', () => {
  it('shows no document when the URL names no item, keeping the tab to return to', async () => {
    mockApi(explorerHandlers());
    const { router } = renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(await within(tree).findByRole('treeitem', { name: 'notes' }));
    await userEvent.click(await within(tree).findByRole('treeitem', { name: /Meeting notes/ }));
    expect(await screen.findByRole('article', { name: 'Meeting notes' })).toBeTruthy();
    router.history.back();
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}`));
    await waitFor(() => expect(screen.queryByRole('article')).toBeNull());
    expect(screen.getByRole('tab', { name: 'Meeting notes', selected: false })).toBeTruthy();
    expect(screen.getByText('Choose one of the open items, or open another.')).toBeTruthy();
  });
});
