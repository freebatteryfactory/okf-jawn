/** The Explorer shell: workspace list, lazily listed tree, and deep links that reopen an item and selection. */

import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fail, mockApi, notImplemented, ok } from './support/api';
import {
  HEAD,
  itemDocument,
  listing,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  readResponse,
  renderAt,
  sourceSummary,
  sourcesOfNote,
  summary,
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

  it('moves focus to the first workspace "Show more workspaces" brought, and says how many are shown', async () => {
    const second = {
      ...workspace,
      id: '77777777-7777-4777-8777-777777777777',
      name: 'Second workspace',
    };
    mockApi({
      '/api/workspaces/list-workspaces': (body) =>
        (body.page as { cursor?: string | null }).cursor === 'more'
          ? ok({ items: [second], next_cursor: null })
          : ok({ items: [workspace], next_cursor: 'more' }),
    });
    renderAt('/');
    const more = await screen.findByRole('button', { name: 'Show more workspaces' });
    act(() => more.focus());
    await userEvent.keyboard('{Enter}');
    const link = await screen.findByRole('link', { name: 'Second workspace' });
    await waitFor(() => expect(document.activeElement).toBe(link));
    expect(screen.getByText('2 workspaces shown.').getAttribute('role')).toBe('status');
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

  it('wraps a long name in a taller measured row, never a sideways-scrolling one', async () => {
    const title = 'Minutes of the October workshop on source provenance and naming rules';
    const long = summary(OTHER, 'minutes.md', title, 'meeting-note');
    mockApi({
      ...workspaceHandlers(),
      [LIST]: () => ok(listing(['notes'], [long, sourceSummary])),
    });
    // happy-dom has no layout and never reports resizes: the entry holding the long name
    // measures one line high, then two once the column narrows (its observers are told).
    let lines = 1;
    const resized: Array<() => void> = [];
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: (entries: unknown[], observer: unknown) => void) {
          resized.push(() => callback([], this));
        }
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
    const original = HTMLElement.prototype.getBoundingClientRect;
    const spy = vi
      .spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockImplementation(function (this: HTMLElement) {
        if (!this.classList.contains('tree-entry')) return original.call(this);
        const wraps = this.textContent?.includes(title) && lines === 2;
        return new DOMRect(0, 0, 200, wraps ? 52 : 30);
      });
    try {
      renderAt(`/w/${WS}`);
      const tree = await screen.findByRole('tree', { name: 'Folders' });
      const row = await within(tree).findByRole('treeitem', { name: new RegExp(title) });
      const after = within(tree).getByRole('treeitem', { name: /Annual report/ });
      await waitFor(() => expect(after.style.top).toBe('60px'));
      lines = 2;
      act(() => {
        for (const callback of resized) callback();
      });
      await waitFor(() => expect(row.style.height).toBe('52px'));
      await waitFor(() => expect(after.style.top).toBe('82px'));
      for (const each of within(tree).getAllByRole('treeitem')) {
        expect(each.style.minWidth).not.toBe('max-content');
      }
    } finally {
      spy.mockRestore();
    }
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

  it('moves focus to the first entry "Show more entries" brought, and says so', async () => {
    mockApi({
      ...workspaceHandlers(),
      [LIST]: (body) => {
        const page = body.page as { cursor: string | null };
        return page.cursor === 'next-page'
          ? ok(listing([], [otherSummary]))
          : ok(listing([], [noteSummary], 'next-page'));
      },
    });
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await within(tree).findByRole('treeitem', { name: 'Show more entries' });
    tree.focus();
    await userEvent.keyboard('{End}');
    await waitFor(() =>
      expect(document.activeElement?.getAttribute('aria-label')).toBe('Show more entries'),
    );
    await userEvent.keyboard('{Enter}');
    const plan = await within(tree).findByRole('treeitem', { name: /Plan/ });
    await waitFor(() => expect(document.activeElement).toBe(plan));
    expect(screen.getByText('1 entry shown.').getAttribute('role')).toBe('status');
  });

  it('keeps focus on the row when more entries fail to list, and says so', async () => {
    mockApi({
      ...workspaceHandlers(),
      [LIST]: (body) => {
        const page = body.page as { cursor: string | null };
        return page.cursor === 'next-page'
          ? fail(500, 'internal', 'storage stumbled')
          : ok(listing([], [noteSummary], 'next-page'));
      },
    });
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await within(tree).findByRole('treeitem', { name: 'Show more entries' });
    tree.focus();
    await userEvent.keyboard('{End}{Enter}');
    const failed = await within(tree).findByRole('treeitem', {
      name: /Could not list more entries/,
    });
    await waitFor(() => expect(document.activeElement).toBe(failed));
    expect(screen.getByText('The entries could not be listed.')).toBeTruthy();
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
    expect(await screen.findByText(/it is not the latest version/)).toBeTruthy();
    await waitFor(() => expect(api.callsTo(READ).length).toBeGreaterThan(0));
    expect(api.callsTo(READ)[0]?.at).toEqual({ kind: 'revision', revision: old });
    expect(api.callsTo('/api/reads/get-sources')[0]?.at).toEqual({
      kind: 'revision',
      revision: old,
    });
  });
});

describe('P3 failures: an unbuilt listing is never an empty workspace', () => {
  it('shows a NotImplemented root listing as not available, never as "no folders or items"', async () => {
    mockApi({ ...workspaceHandlers(), [LIST]: () => notImplemented('list_items') });
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('region', { name: 'Folders' });
    const alert = await within(tree).findByRole('alert');
    expect(alert.textContent).toContain('Listing this workspace: Not available in this build.');
    expect(within(tree).queryByText(/no folders or items/)).toBeNull();
  });

  it('shows an unbuilt open_workspace as not available', async () => {
    mockApi({
      ...workspaceHandlers(),
      '/api/workspaces/open-workspace': () => notImplemented('open_workspace'),
    });
    renderAt(`/w/${WS}`);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Opening this workspace: Not available in this build.');
    expect(screen.queryByRole('tree')).toBeNull();
  });
});

describe('the page before there is an Explorer', () => {
  const pageHeading = () => screen.getByRole('heading', { level: 1, name: 'Workspace' });

  it('has a level-one heading while the workspace opens, and when it did not open', async () => {
    let answer: (reply: ReturnType<typeof fail>) => void = () => {};
    mockApi({
      ...workspaceHandlers(),
      '/api/workspaces/open-workspace': () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    renderAt(`/w/${WS}`);
    expect((await screen.findByText('Opening the workspace…')).getAttribute('role')).toBe('status');
    // Script can focus the heading: focus that was in a failure notice goes there when nothing
    // else is named, and a browser ignores the call without the attribute.
    expect(pageHeading().getAttribute('tabindex')).toBe('-1');
    answer(fail(500, 'internal', 'storage stumbled'));
    await screen.findByRole('alert');
    expect(pageHeading().getAttribute('tabindex')).toBe('-1');
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
  });

  it('has a level-one heading when the link names no workspace', async () => {
    mockApi(workspaceHandlers());
    renderAt('/w/not-a-workspace');
    await screen.findByRole('alert');
    expect(pageHeading().getAttribute('tabindex')).toBe('-1');
  });

  it('leaves the level-one heading to the workspace’s name once the Explorer is there', async () => {
    mockApi(workspaceHandlers());
    renderAt(`/w/${WS}`);
    const name = await screen.findByRole('heading', { level: 1, name: 'Workshop notes' });
    expect(name.getAttribute('tabindex')).toBe('-1');
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
  });
});

describe('P4 state: revision and selection travel together in the URL', () => {
  it('reopens a deep link with both a cited revision and a selection, and keeps the revision when the whole item is shown', async () => {
    const api = mockApi(workspaceHandlers());
    const selection = { kind: 'lines', range: { start: 2, end: 3 } };
    const { router } = renderAt(
      `/w/${WS}/i/${NOTE}?sel=${encodeURIComponent(JSON.stringify(selection))}&rev=${OLD}`,
    );
    // Adapted: the cited-revision notice now reads "You are reading revision …".
    await screen.findByText(/You are reading revision/);
    await waitFor(() => expect(api.callsTo(READ).length).toBeGreaterThan(0));
    expect(api.callsTo(READ)[0]?.at).toEqual({ kind: 'revision', revision: OLD });
    expect(api.callsTo(READ)[0]?.selection).toEqual(selection);
    await userEvent.click(screen.getByRole('button', { name: 'Show the whole item' }));
    await waitFor(() => expect(router.state.location.search).toEqual({ rev: OLD }));
    expect(api.callsTo(READ).at(-1)?.at).toEqual({ kind: 'revision', revision: OLD });
  });

  it('writes an outline choice into the URL selection', async () => {
    const section = { kind: 'section', heading: 'Budget' } as const;
    mockApi({
      ...workspaceHandlers(),
      [READ]: () =>
        ok(
          readResponse(noteSummary, '# Meeting notes\n\n## Budget\n\nText', {
            outline: [{ kind: 'heading', label: 'Budget', level: 2, selection: section }],
          }),
        ),
    });
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await userEvent.click(await within(article).findByText('Outline (1)'));
    await userEvent.click(within(article).getByRole('button', { name: 'Budget' }));
    await waitFor(() => expect(router.state.location.search).toEqual({ sel: section }));
  });
});

describe('revealing a linked item', () => {
  it('reveals a cited item where the listed head holds it, found by id, and lists only the head', async () => {
    // At the cited revision the note lived in archive/; at the head it is in notes/.
    const moved = { ...noteSummary, path: 'archive/meeting.md', revision: OLD };
    const api = mockApi({
      ...workspaceHandlers(),
      [LIST]: (body) =>
        body.folder === 'notes'
          ? ok(listing([], [noteSummary, otherSummary]))
          : body.folder === 'archive'
            ? ok(listing([], []))
            : ok(listing(['archive', 'notes'], [sourceSummary])),
      '/api/items/get-item': (body) => {
        const at = body.at as { revision?: string };
        if (body.item_id !== NOTE) return ok(itemDocument(otherSummary));
        return ok(itemDocument(at.revision === OLD ? moved : noteSummary));
      },
    });
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const row = await within(tree).findByRole('treeitem', { name: /Meeting notes/ });
    await waitFor(() => expect(row.getAttribute('aria-selected')).toBe('true'));
    expect(
      within(tree).getByRole('treeitem', { name: 'notes' }).getAttribute('aria-expanded'),
    ).toBe('true');
    expect(
      within(tree).getByRole('treeitem', { name: 'archive' }).getAttribute('aria-expanded'),
    ).toBe('false');
    expect(api.callsTo(LIST).map((body) => body.folder)).toEqual(['', 'notes']);
    for (const body of api.callsTo(LIST)) {
      expect(body.at).toEqual({ kind: 'revision', revision: HEAD });
    }
  });
});

describe('the tree from the keyboard, however focus enters it (B5)', () => {
  /** A deep link selects Meeting notes, in notes/, between the folder and Plan. */
  async function linkedTree() {
    mockApi(workspaceHandlers());
    const view = renderAt(`/w/${WS}/i/${NOTE}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const row = await within(tree).findByRole('treeitem', { name: /Meeting notes/ });
    await waitFor(() => expect(row.getAttribute('aria-selected')).toBe('true'));
    await within(tree).findByRole('treeitem', { name: /Plan/ });
    return { ...view, tree, row };
  }
  const focusedRow = () => document.activeElement?.getAttribute('aria-label') ?? '';

  it('lands on the tab-stop row and moves from it with arrows, Home, End, type-ahead and Enter (Q2c)', async () => {
    const { tree, row, router } = await linkedTree();
    // Shift+Tab from after the tree lands on the tab-stop row itself: the selected entry.
    expect(row.tabIndex).toBe(0);
    act(() => row.focus());
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Plan,/));
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Annual report,/));
    await userEvent.keyboard('{ArrowUp}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Plan,/));
    await userEvent.keyboard('{ArrowLeft}');
    await waitFor(() => expect(focusedRow()).toBe('notes'));
    await userEvent.keyboard('{End}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Annual report,/));
    await userEvent.keyboard('{Home}');
    await waitFor(() => expect(focusedRow()).toBe('notes'));
    await userEvent.keyboard('p');
    await waitFor(() => expect(focusedRow()).toMatch(/^Plan,/));
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`));
    expect(within(tree).getByRole('treeitem', { name: /Plan/ })).toBeTruthy();
  });

  it('moves Tab onto the tree container on to the selected entry, which the arrows move from', async () => {
    const { tree } = await linkedTree();
    act(() => tree.focus());
    await waitFor(() => expect(focusedRow()).toMatch(/^Meeting notes,/));
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Plan,/));
  });

  it('keeps the scrolling list in the Tab order, as a group of the tree, and moves focus from it to the tab stop', async () => {
    const { tree } = await linkedTree();
    const scroller = tree.firstElementChild;
    if (!(scroller instanceof HTMLElement)) throw new Error('the tree has no scrolling list');
    expect(scroller.getAttribute('role')).toBe('group');
    expect(scroller.getAttribute('tabindex')).toBe('0');
    expect(within(scroller).getAllByRole('treeitem').length).toBeGreaterThan(0);
    act(() => scroller.focus());
    await waitFor(() => expect(focusedRow()).toMatch(/^Meeting notes,/));
    await userEvent.keyboard('{ArrowUp}');
    await waitFor(() => expect(focusedRow()).toBe('notes'));
  });

  it('enters on the selected entry, not on the one last focused (WAI-ARIA tree pattern, N25)', async () => {
    const { tree, row } = await linkedTree();
    act(() => tree.focus());
    await waitFor(() => expect(focusedRow()).toMatch(/^Meeting notes,/));
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Plan,/));
    act(() => screen.getByRole('searchbox', { name: 'Search this workspace' }).focus());
    // Shift+Tab's target, the row in the Tab order, is the selected one.
    expect(row.tabIndex).toBe(0);
    expect(within(tree).getByRole('treeitem', { name: /Plan/ }).tabIndex).toBe(-1);
    // Tab onto the tree goes there too.
    act(() => tree.focus());
    await waitFor(() => expect(focusedRow()).toMatch(/^Meeting notes,/));
  });

  it('enters on the first entry when the tree does not show the selected one', async () => {
    const { tree } = await linkedTree();
    act(() => tree.focus());
    await waitFor(() => expect(focusedRow()).toMatch(/^Meeting notes,/));
    // Left to the folder, Left again to close it: the selected entry is hidden inside.
    await userEvent.keyboard('{ArrowLeft}');
    await waitFor(() => expect(focusedRow()).toBe('notes'));
    await userEvent.keyboard('{ArrowLeft}');
    await waitFor(() =>
      expect(within(tree).queryByRole('treeitem', { name: /Meeting notes/ })).toBeNull(),
    );
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedRow()).toMatch(/^Annual report,/));
    act(() => screen.getByRole('searchbox', { name: 'Search this workspace' }).focus());
    act(() => tree.focus());
    await waitFor(() => expect(focusedRow()).toBe('notes'));
  });

  it('leaves a pointer press on the scrolling list to arborist, so it never scrolls the list away', async () => {
    const many = Array.from({ length: 80 }, (_, index) =>
      summary(
        `${String(index).padStart(8, '0')}-1111-4111-8111-111111111111`,
        `item-${index}.md`,
        `Item number ${index}`,
        'note',
      ),
    );
    const item70 = many[70];
    if (item70 === undefined) throw new Error('no 71st entry');
    mockApi({
      ...workspaceHandlers(),
      [LIST]: () => ok(listing([], many)),
      '/api/items/get-item': () => ok(itemDocument(item70)),
    });
    renderAt(`/w/${WS}/i/${item70.id}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const selected = await within(tree).findByRole('treeitem', { name: /Item number 70,/ });
    await waitFor(() => expect(selected.getAttribute('aria-selected')).toBe('true'));
    const scroller = tree.firstElementChild;
    if (!(scroller instanceof HTMLElement)) throw new Error('the tree has no scrolling list');
    // The wheel takes the list back to the top, far from the selected entry.
    const wheelToTop = async () => {
      act(() => {
        scroller.scrollTop = 0;
        fireEvent.scroll(scroller);
      });
      await within(tree).findByRole('treeitem', { name: /Item number 0,/ });
    };
    await wheelToTop();
    // The keyboard is taken to the tab stop, scrolled into view.
    act(() => scroller.focus());
    await waitFor(() => expect(focusedRow()).toMatch(/^Item number 70,/));
    expect(scroller.scrollTop).toBeGreaterThan(0);
    // A pointer press on the list is not: the list stays where the wheel left it.
    act(() => screen.getByRole('searchbox', { name: 'Search this workspace' }).focus());
    await wheelToTop();
    fireEvent.pointerDown(scroller);
    act(() => scroller.focus());
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(scroller.scrollTop).toBe(0);
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
    // No selection notice: the malformed selection was dropped, not shown.
    expect(screen.queryByRole('button', { name: 'Show the whole item' })).toBeNull();
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
