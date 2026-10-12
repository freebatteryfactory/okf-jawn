/** The Explorer shell: workspace list, lazily listed tree, and deep links that reopen an item and selection. */

import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fail, mockApi, notImplemented, ok } from './support/api';
import {
  HEAD,
  itemDocument,
  listing,
  NOTE,
  noteSummary,
  otherSummary,
  readResponse,
  renderAt,
  sourceSummary,
  sourcesOfNote,
  WS,
  workspace,
} from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const LIST = '/api/items/list-items';
const READ = '/api/reads/read-item';

function workspaceHandlers() {
  return {
    '/api/workspaces/open-workspace': () => ok(workspace),
    [LIST]: (body: Record<string, unknown>) =>
      body.folder === 'notes'
        ? ok(listing([], [noteSummary, otherSummary]))
        : ok(listing(['notes'], [sourceSummary])),
    '/api/items/get-item': (body: Record<string, unknown>) =>
      ok(itemDocument(body.item_id === NOTE ? noteSummary : otherSummary, { owner: 'Ada' })),
    [READ]: (body: Record<string, unknown>) => {
      const item = body.item_id === NOTE ? noteSummary : otherSummary;
      const selection = body.selection;
      const lines =
        typeof selection === 'object' && selection !== null && 'kind' in selection
          ? selection.kind
          : 'all';
      return ok(
        readResponse(
          item,
          lines === 'lines' ? 'Only lines two and three' : `# ${item.title}\n\nWhole text`,
        ),
      );
    },
    '/api/reads/get-sources': () => ok(sourcesOfNote),
  };
}

describe('workspace list', () => {
  it('lists the workspaces the server returns and opens one', async () => {
    const api = mockApi({
      '/api/workspaces/list-workspaces': () => ok({ items: [workspace], next_cursor: null }),
      ...workspaceHandlers(),
    });
    const { router } = renderAt('/');
    const link = await screen.findByRole('link', { name: 'Workshop notes' });
    await userEvent.click(link);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}`));
    expect(await screen.findByRole('heading', { level: 1, name: 'Workshop notes' })).toBeTruthy();
    expect(api.callsTo('/api/workspaces/open-workspace')).toEqual([{ workspace_id: WS }]);
    expect(api.violations).toEqual([]);
  });

  it('reads a NotImplemented listing as not available, never as an empty list', async () => {
    mockApi({ '/api/workspaces/list-workspaces': () => notImplemented('list_workspaces') });
    renderAt('/');
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Not available in this build.');
    expect(screen.queryByText(/You have no workspaces yet/)).toBeNull();
    expect(screen.queryByRole('list')).toBeNull();
  });
});

describe('folder tree', () => {
  it('lists the root first and a folder only when it is expanded, pinned to the head', async () => {
    const api = mockApi(workspaceHandlers());
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const folder = await within(tree).findByRole('treeitem', { name: 'notes' });
    const source = within(tree).getByRole('treeitem', { name: /Annual report/ });
    expect(source.textContent).toContain('report');
    expect(source.textContent).toContain('partly converted');
    expect(folder.getAttribute('aria-expanded')).toBe('false');
    expect(source.hasAttribute('aria-expanded')).toBe(false);
    expect(api.callsTo(LIST).map((body) => body.folder)).toEqual(['']);

    await userEvent.click(folder);
    const note = await within(tree).findByRole('treeitem', {
      name: /Meeting notes, type meeting-note/,
    });
    expect(note).toBeTruthy();
    expect(folder.getAttribute('aria-expanded')).toBe('true');
    expect(api.callsTo(LIST).map((body) => body.folder)).toEqual(['', 'notes']);
    for (const body of api.callsTo(LIST)) {
      expect(body.at).toEqual({ kind: 'revision', revision: HEAD });
      expect(body.page).toEqual({ limit: 200, cursor: null });
    }
    expect(api.violations).toEqual([]);
  });

  it('gives every virtualized row its place among its siblings (WAI-ARIA tree)', async () => {
    mockApi(workspaceHandlers());
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const folder = await within(tree).findByRole('treeitem', { name: 'notes' });
    const place = (row: HTMLElement) =>
      `${row.getAttribute('aria-level')}:${row.getAttribute('aria-posinset')}/${row.getAttribute('aria-setsize')}`;
    expect(place(folder)).toBe('1:1/2');
    expect(place(within(tree).getByRole('treeitem', { name: /Annual report/ }))).toBe('1:2/2');
    await userEvent.click(folder);
    const note = await within(tree).findByRole('treeitem', { name: /Meeting notes/ });
    expect(place(note)).toBe('2:1/2');
    expect(place(within(tree).getByRole('treeitem', { name: /Plan/ }))).toBe('2:2/2');
  });

  it('pages a long folder through the listing cursor', async () => {
    const api = mockApi({
      ...workspaceHandlers(),
      [LIST]: (body) => {
        const page = body.page;
        const cursor =
          typeof page === 'object' && page !== null && 'cursor' in page ? page.cursor : null;
        return cursor === 'next-page'
          ? ok(listing([], [otherSummary]))
          : ok(listing([], [noteSummary], 'next-page'));
      },
    });
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const more = await within(tree).findByRole('treeitem', { name: 'Show more entries' });
    expect(within(tree).queryByRole('treeitem', { name: /Plan/ })).toBeNull();
    await userEvent.click(more);
    expect(await within(tree).findByRole('treeitem', { name: /Plan/ })).toBeTruthy();
    expect(within(tree).queryByRole('treeitem', { name: 'Show more entries' })).toBeNull();
    const cursors = api.callsTo(LIST).map((body) => (body.page as { cursor: unknown }).cursor);
    expect(cursors).toEqual([null, 'next-page']);
  });

  it('opens an item from the keyboard and shows it in a tab', async () => {
    mockApi(workspaceHandlers());
    const { router } = renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await within(tree).findByRole('treeitem', { name: 'notes' });
    // Focusing the tree focuses its first entry, the folder.
    tree.focus();
    await userEvent.keyboard('{ArrowRight}');
    await within(tree).findByRole('treeitem', { name: /Meeting notes/ });
    await userEvent.keyboard('{ArrowDown}');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(await screen.findByRole('tab', { name: 'Meeting notes' })).toBeTruthy();
    expect(await screen.findByText('Whole text')).toBeTruthy();
  });

  it('shows a failed folder listing in plain words inside the tree, with its retry', async () => {
    let attempts = 0;
    mockApi({
      ...workspaceHandlers(),
      [LIST]: (body) => {
        if (body.folder !== 'notes') return ok(listing(['notes'], []));
        attempts += 1;
        return attempts === 1
          ? fail(500, 'internal', 'storage stumbled')
          : ok(listing([], [noteSummary]));
      },
    });
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(await within(tree).findByRole('treeitem', { name: 'notes' }));
    const failed = await within(tree).findByRole(
      'treeitem',
      { name: /Could not list this folder: Something went wrong in the service/ },
      { timeout: 3000 },
    );
    expect(failed.getAttribute('aria-label')).toBe(
      'Could not list this folder: Something went wrong in the service. The service said: “storage stumbled”. Press Enter to try again.',
    );
    await userEvent.click(failed);
    expect(await within(tree).findByRole('treeitem', { name: /Meeting notes/ })).toBeTruthy();
  });

  it('offers no retry in a failed folder row the failure does not allow, and keeps the server words', async () => {
    const api = mockApi({
      ...workspaceHandlers(),
      [LIST]: (body) =>
        body.folder === 'notes' ? notImplemented('list_items') : ok(listing(['notes'], [])),
    });
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(await within(tree).findByRole('treeitem', { name: 'notes' }));
    const failed = await within(tree).findByRole('treeitem', {
      name: /Could not list this folder: Not available in this build/,
    });
    expect(failed.getAttribute('aria-label')).toBe(
      'Could not list this folder: Not available in this build. The service said: “list_items is not implemented in this build”.',
    );
    const before = api.callsTo(LIST).length;
    failed.focus();
    await userEvent.keyboard('{Enter}');
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(api.callsTo(LIST).length).toBe(before);
  });
});

describe('deep links', () => {
  it('reopens the same item and selection, revealing it in the tree', async () => {
    const api = mockApi(workspaceHandlers());
    const selection = { kind: 'lines', range: { start: 2, end: 3 } };
    const { router } = renderAt(
      `/w/${WS}/i/${NOTE}?sel=${encodeURIComponent(JSON.stringify(selection))}`,
    );
    expect(await screen.findByText('Only lines two and three')).toBeTruthy();
    expect(screen.getByRole('note', { name: '' }).textContent).toContain('Showing lines 2–3.');
    expect(api.callsTo(READ)[0]?.selection).toEqual(selection);
    expect(await screen.findByRole('tab', { name: 'Meeting notes', selected: true })).toBeTruthy();
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const row = await within(tree).findByRole('treeitem', { name: /Meeting notes/ });
    await waitFor(() => expect(row.getAttribute('aria-selected')).toBe('true'));
    expect(
      within(tree).getByRole('treeitem', { name: 'notes' }).getAttribute('aria-expanded'),
    ).toBe('true');

    await userEvent.click(screen.getByRole('button', { name: 'Show the whole item' }));
    expect(await screen.findByText('Whole text')).toBeTruthy();
    expect(router.state.location.search).toEqual({});
    expect(api.violations).toEqual([]);
  });

  it('reads at the exact revision a link cites, and says it is not the latest', async () => {
    const api = mockApi(workspaceHandlers());
    const old = 'b'.repeat(40);
    renderAt(`/w/${WS}/i/${NOTE}?rev=${old}`);
    expect(
      await screen.findByText(/which a citation named; it is not the latest version/),
    ).toBeTruthy();
    await waitFor(() => expect(api.callsTo(READ).length).toBeGreaterThan(0));
    expect(api.callsTo(READ)[0]?.at).toEqual({ kind: 'revision', revision: old });
    expect(api.callsTo('/api/reads/get-sources')[0]?.at).toEqual({
      kind: 'revision',
      revision: old,
    });
  });
});

describe('malformed links', () => {
  it('never sends a selection or revision the generated schemas reject', async () => {
    const api = mockApi(workspaceHandlers());
    renderAt(`/w/${WS}/i/${NOTE}?sel=${encodeURIComponent('"lines 2 to 3"')}&rev=HEAD`);
    expect(await screen.findByText('Whole text')).toBeTruthy();
    const read = api.callsTo(READ)[0];
    expect(read?.selection).toEqual({ kind: 'all' });
    expect(read?.at).toEqual({ kind: 'revision', revision: HEAD });
    expect(screen.queryByText(/Showing/)).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('never sends an item id that is not a UUID (P5)', async () => {
    const api = mockApi(workspaceHandlers());
    renderAt(`/w/${WS}/i/not-an-item`);
    await screen.findByRole('tree', { name: 'Folders' });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(api.violations).toEqual([]);
  });

  it('says a malformed item link is not valid, and requests nothing for it', async () => {
    const api = mockApi(workspaceHandlers());
    renderAt(`/w/${WS}/i/not-an-item`);
    const documents = await screen.findByRole('region', { name: 'Documents' });
    const alert = await within(documents).findByRole('alert');
    expect(alert.textContent).toContain('This link is not valid.');
    expect(alert.textContent).toContain('nothing was requested');
    await screen.findByRole('tree', { name: 'Folders' });
    expect(api.callsTo('/api/items/get-item')).toEqual([]);
    expect(api.callsTo(READ)).toEqual([]);
    expect(api.callsTo('/api/reads/get-sources')).toEqual([]);
    expect(screen.queryByRole('tab')).toBeNull();
  });

  it('says a malformed workspace link is not valid, and opens nothing', async () => {
    const api = mockApi(workspaceHandlers());
    renderAt('/w/not-a-workspace');
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('This link is not valid.');
    expect(screen.getByRole('link', { name: 'Go to your workspaces' })).toBeTruthy();
    expect(api.calls).toEqual([]);
  });
});
