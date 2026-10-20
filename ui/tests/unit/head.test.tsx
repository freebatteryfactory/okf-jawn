/**
 * The workspace head: shown "as of" when it was resolved, re-resolved only on Refresh, checked on
 * window focus, and a revision is called "the latest" only when a request made for that statement
 * resolved it (SPEC section 3: "At::Latest is a request selector resolved once before the
 * operation reads").
 */

import { focusManager, onlineManager } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { openWorkspaceQueryKey } from '../../src/api/generated/@tanstack/react-query.gen';
import type { ItemSummary } from '../../src/api/generated/types.gen';
import { fail, type Json, mockApi, ok, type Reply } from './support/api';
import {
  explorerHandlers,
  HEAD,
  listing,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  readResponse,
  renderAt,
  sourceSummary,
  summary,
  WS,
  workspace,
} from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const OPEN = '/api/workspaces/open-workspace';
const LIST = '/api/items/list-items';
const READ = '/api/reads/read-item';
const GET = '/api/items/get-item';
const SOURCES = '/api/reads/get-sources';
const NEW = 'e'.repeat(40);

/** The window loses and regains focus, as TanStack Query's focus manager sees it. */
function windowRefocus() {
  act(() => {
    focusManager.setFocused(false);
    focusManager.setFocused(true);
  });
  act(() => focusManager.setFocused(undefined));
}

const focusedName = () => document.activeElement?.getAttribute('aria-label') ?? '';

/** open_workspace answers HEAD the first time and NEW from then on. */
function headMovesOn() {
  let opens = 0;
  return () => {
    opens += 1;
    return ok({ ...workspace, head: opens === 1 ? HEAD : NEW });
  };
}

/** A read answered at the revision it asked for; `at: latest` resolves to NEW. */
function readAt(extra: (at: Json) => Json = () => ({})) {
  return (body: Json) => {
    const at = body.at as Json;
    const resolved = at.kind === 'latest' ? NEW : String(at.revision);
    const response = readResponse(noteSummary, `Text at ${resolved.slice(0, 1)}`, extra(at));
    return ok({ ...response, source: { ...response.source, revision: resolved } });
  };
}

describe('the "latest version" is the latest the server resolved for that request', () => {
  it('reads the latest version with a fresh at-latest request, never the cached head, and pins what it resolved (P6, adapted)', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn(), [READ]: readAt() }));
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Read the latest version' }));
    // The tab and the URL are pinned to the revision the server resolved.
    await waitFor(() => expect(router.state.location.search).toEqual({ rev: NEW }));
    const reads = api.callsTo(READ);
    const latest = reads.findIndex((body) => (body.at as Json).kind === 'latest');
    expect(latest).toBeGreaterThanOrEqual(0);
    for (const body of reads.slice(latest)) {
      expect(body.at).not.toEqual({ kind: 'revision', revision: HEAD });
    }
    // The revision shown is the one the response carried.
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await within(article).findByText('Text at e');
    expect(within(article).getAllByTitle(NEW).length).toBeGreaterThan(0);
    expect(within(article).queryAllByTitle(HEAD)).toEqual([]);
    // The answer to that one read is the pane's text: no second read of the same part.
    expect(reads.slice(latest + 1).filter((body) => body.cursor === null)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it("continues the latest read through that read's own cursor, at the revision it resolved", async () => {
    const api = mockApi(
      explorerHandlers({
        [OPEN]: headMovesOn(),
        [READ]: readAt((at) =>
          at.kind === 'latest' ? { truncated: true, next_cursor: 'latest-part-2' } : {},
        ),
      }),
    );
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Read the latest version' }));
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await userEvent.click(await within(article).findByRole('button', { name: 'Continue reading' }));
    await waitFor(() => expect(api.callsTo(READ).at(-1)?.cursor).toBe('latest-part-2'));
    expect(api.callsTo(READ).at(-1)?.at).toEqual({ kind: 'revision', revision: NEW });
  });

  it('compares a pinned revision with a head fetched for that comparison, not the cached one', async () => {
    // The shell resolved HEAD; by the time the reader asks, the head has moved on to NEW.
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn() }));
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    expect(await screen.findByText(/it is not the latest version/)).toBeTruthy();
    expect(api.callsTo(OPEN).length).toBeGreaterThanOrEqual(2);
  });

  it('says a pinned revision is the latest only as of when it was checked', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    const notice = await screen.findByText(/the latest version when checked at/);
    expect(notice.querySelector('time')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Read the latest version' })).toBeNull();
  });

  it('keeps keyboard focus at the revision notice when "Read the latest version" has read it', async () => {
    mockApi(explorerHandlers({ [OPEN]: headMovesOn(), [READ]: readAt() }));
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const button = await screen.findByRole('button', { name: 'Read the latest version' });
    act(() => button.focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(router.state.location.search).toEqual({ rev: NEW }));
    await waitFor(() => expect(button.isConnected).toBe(false));
    const focused = document.activeElement;
    expect(focused).not.toBe(document.body);
    await waitFor(() =>
      expect(focused?.textContent).toContain(`reading revision ${NEW.slice(0, 10)}`),
    );
  });

  // The reader is keyed by its tab: once the person has gone to another, this one is gone.
  describe('when the reader is gone before the latest version answers', () => {
    /** The note pinned at OLD beside a tab of the plan; the at-latest read answers when told. */
    async function readingLatest() {
      const latest: { answer?: () => void } = {};
      const api = mockApi(
        explorerHandlers({
          [OPEN]: headMovesOn(),
          [READ]: (body) => {
            if (body.item_id !== NOTE || (body.at as Json).kind !== 'latest') {
              return body.item_id === NOTE
                ? readAt()(body)
                : ok(readResponse(otherSummary, 'Text of Plan'));
            }
            return new Promise<Reply>((resolve) => {
              latest.answer = () => resolve(readAt()(body));
            });
          },
        }),
      );
      // The signal of every read the page sends, in order.
      const signals: Array<AbortSignal | undefined> = [];
      const inner = globalThis.fetch;
      vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
        const sent = input instanceof Request ? input : null;
        const path = new URL(sent === null ? String(input) : sent.url, 'http://x').pathname;
        if (path === READ) signals.push(init?.signal ?? sent?.signal);
        return inner(input, init);
      });
      const view = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
      await screen.findByRole('tab', { name: 'Meeting notes', selected: true });
      const tree = await screen.findByRole('tree', { name: 'Folders' });
      await userEvent.click(await within(tree).findByRole('treeitem', { name: /Plan/ }));
      await screen.findByRole('tab', { name: 'Plan', selected: true });
      await userEvent.click(screen.getByRole('tab', { name: 'Meeting notes' }));
      const button = await screen.findByRole('button', { name: 'Read the latest version' });
      const before = signals.length;
      act(() => button.focus());
      await userEvent.keyboard('{Enter}');
      await waitFor(() => expect(latest.answer).toBeDefined());
      const answer = () => latest.answer?.();
      return { ...view, api, answer, asked: () => signals.at(before) };
    }

    it('leaves the pane on the tab the person chose, with focus where they put it, and aborts the read', async () => {
      const { router, answer, asked } = await readingLatest();
      expect(asked()?.aborted).toBe(false);
      const plan = screen.getByRole('tab', { name: 'Plan' });
      await userEvent.click(plan);
      await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`));
      await screen.findByRole('article', { name: 'Plan' });
      // The reader that asked is gone: its request does not go on.
      expect(asked()?.aborted).toBe(true);
      act(() => plan.focus());
      act(() => answer());
      await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
      expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`);
      expect(router.state.location.search).toEqual({});
      expect(screen.getByRole('tab', { name: 'Plan', selected: true })).toBeTruthy();
      expect(document.activeElement).toBe(plan);
    });

    it('moves nothing when the answer had arrived as the reader went', async () => {
      const { router, answer, unmount } = await readingLatest();
      // The answer is in, and before the page has acted on it the reader is taken away.
      act(() => {
        answer();
        unmount();
      });
      await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
      expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`);
      expect(router.state.location.search).toEqual({ rev: OLD });
    });
  });

  it('shows a failed latest read, and keeps the pinned revision', async () => {
    mockApi(
      explorerHandlers({
        [OPEN]: headMovesOn(),
        [READ]: (body) =>
          (body.at as Json).kind === 'latest'
            ? fail(500, 'internal', 'storage stumbled')
            : readAt()(body),
      }),
    );
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Read the latest version' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Reading the latest version:');
    expect(router.state.location.search).toEqual({ rev: OLD });
  });
});

describe('the shell head is shown as of when it was resolved', () => {
  it('shows the head it lists "as of" an instant, and Refresh re-resolves it', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn() }));
    renderAt(`/w/${WS}`);
    const line = await screen.findByText(/Showing the workspace as of/);
    expect(line.querySelector('time')).toBeTruthy();
    expect(within(line).getByTitle(HEAD)).toBeTruthy();
    await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await screen.findByText(`Refreshed: now showing revision ${NEW.slice(0, 10)}.`);
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).querySelector('code')?.title).toBe(
        NEW,
      ),
    );
    // The tree lists the re-resolved head.
    await waitFor(() =>
      expect(api.callsTo(LIST).some((body) => (body.at as Json).revision === NEW)).toBe(true),
    );
  });

  it('checks for a newer head when the window regains focus, says so politely, and moves nothing (Q3)', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn() }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await within(article).findByText('Text of Meeting notes');
    await within(tree).findByRole('treeitem', { name: /Plan/ });
    expect(api.callsTo(OPEN).length).toBe(1);
    const asked = () => [LIST, READ, GET, SOURCES].map((path) => api.callsTo(path).length);
    const before = asked();
    windowRefocus();
    const notice = await screen.findByText(/A newer version of this workspace exists/);
    expect(notice.closest('[role="status"]')).toBeTruthy();
    expect(notice.querySelector('time')).toBeTruthy();
    expect(api.callsTo(OPEN).length).toBe(2);
    // Nothing reloads: the tree, the reading and the sources stay on the head they were opened at.
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(asked()).toEqual(before);
    for (const path of [LIST, READ, GET, SOURCES]) {
      for (const body of api.callsTo(path)) {
        expect(body.at).toEqual({ kind: 'revision', revision: HEAD });
      }
    }
    expect(within(screen.getByText(/Showing the workspace as of/)).getByTitle(HEAD)).toBeTruthy();
    expect(screen.getByRole('tree', { name: 'Folders' })).toBe(tree);
    expect(within(article).getByText('Text of Meeting notes')).toBeTruthy();
  });

  it('moves to the newer head only on Refresh, keeping the tree on screen, its focused entry and the focus (Q3a)', async () => {
    const api = mockApi(
      explorerHandlers({
        [OPEN]: headMovesOn(),
        [LIST]: (body) => {
          const revision = String((body.at as Json).revision);
          const answer =
            body.folder === 'notes'
              ? listing([], [noteSummary, otherSummary])
              : listing(['notes'], [sourceSummary]);
          return ok({ ...answer, revision });
        },
      }),
    );
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await within(tree).findByRole('treeitem', { name: /Annual report/ });
    act(() => tree.focus());
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedName()).toMatch(/^Annual report,/));
    windowRefocus();
    await screen.findByText(/A newer version of this workspace exists/);
    const refresh = screen.getByRole('button', { name: 'Refresh' });
    await userEvent.click(refresh);
    await screen.findByText(`Refreshed: now showing revision ${NEW.slice(0, 10)}.`);
    await waitFor(() =>
      expect(api.callsTo(LIST).some((body) => (body.at as Json).revision === NEW)).toBe(true),
    );
    expect(screen.queryByText(/A newer version of this workspace exists/)).toBeNull();
    // The tree was never taken off screen, and Refresh kept keyboard focus.
    expect(screen.getByRole('tree', { name: 'Folders' })).toBe(tree);
    expect(document.activeElement).toBe(refresh);
    // Back in the tree, focus lands where the WAI-ARIA tree pattern says: nothing is selected,
    // so the first entry.
    act(() => tree.focus());
    await waitFor(() => expect(focusedName()).toBe('notes'));
  });

  /** Root entries at HEAD, and the ones a newer head lists; focus Annual report (index 1). */
  async function focusedWhileRefreshing(atNew: ItemSummary[], folders: string[] = ['notes']) {
    mockApi(
      explorerHandlers({
        [OPEN]: headMovesOn(),
        [LIST]: (body) => {
          const revision = String((body.at as Json).revision);
          const answer =
            revision === NEW
              ? listing(folders, atNew)
              : listing(['notes'], [sourceSummary, otherSummary]);
          return ok({ ...answer, revision });
        },
      }),
    );
    const { queryClient } = renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await within(tree).findByRole('treeitem', { name: /Plan/ });
    act(() => tree.focus());
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedName()).toMatch(/^Annual report,/));
    // A Refresh resolves the newer head while keyboard focus is still on that row.
    await act(() =>
      queryClient.refetchQueries({
        queryKey: openWorkspaceQueryKey({ body: { workspace_id: WS } }),
        exact: true,
      }),
    );
    return tree;
  }

  it('moves keyboard focus from an entry a refresh removed to the entry now in its place (N18, R4b)', async () => {
    const tree = await focusedWhileRefreshing([otherSummary]);
    await waitFor(() =>
      expect(within(tree).queryByRole('treeitem', { name: /Annual report/ })).toBeNull(),
    );
    await waitFor(() => expect(focusedName()).toMatch(/^Plan,/));
    expect(document.activeElement).not.toBe(document.body);
  });

  it('moves keyboard focus to the nearest remaining entry when the removed one was last', async () => {
    const tree = await focusedWhileRefreshing([], ['notes']);
    await waitFor(() =>
      expect(within(tree).queryByRole('treeitem', { name: /Annual report/ })).toBeNull(),
    );
    await waitFor(() => expect(focusedName()).toBe('notes'));
  });

  it('moves keyboard focus to the empty-workspace line when a refresh leaves no entry', async () => {
    await focusedWhileRefreshing([], []);
    const empty = await screen.findByText('This workspace has no folders or items yet.');
    await waitFor(() => expect(document.activeElement).toBe(empty));
  });

  it('follows the focused entry through a refresh that moves it, to find its place when a later one removes it', async () => {
    const NEWER = 'f'.repeat(40);
    const extra = summary('66666666-6666-4666-8666-666666666666', 'extra.md', 'Extra', 'note');
    const heads = [HEAD, NEW, NEWER];
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          return ok({ ...workspace, head: heads[Math.min(opens, heads.length) - 1] ?? HEAD });
        },
        [LIST]: (body) => {
          const revision = String((body.at as Json).revision);
          const items =
            revision === NEWER
              ? [extra, otherSummary]
              : revision === NEW
                ? [extra, sourceSummary, otherSummary]
                : [sourceSummary, otherSummary];
          return ok({ ...listing(['notes'], items), revision });
        },
      }),
    );
    const { queryClient } = renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await within(tree).findByRole('treeitem', { name: /Plan/ });
    act(() => tree.focus());
    await userEvent.keyboard('{ArrowDown}');
    await waitFor(() => expect(focusedName()).toMatch(/^Annual report,/));
    const refresh = () =>
      act(() =>
        queryClient.refetchQueries({
          queryKey: openWorkspaceQueryKey({ body: { workspace_id: WS } }),
          exact: true,
        }),
      );
    // The newer head adds an entry above: the focused one moves down a place and keeps focus.
    await refresh();
    await within(tree).findByRole('treeitem', { name: /Extra/ });
    expect(focusedName()).toMatch(/^Annual report,/);
    // The next one removes it: focus goes to the entry now at its new place, Plan.
    await refresh();
    await waitFor(() =>
      expect(within(tree).queryByRole('treeitem', { name: /Annual report/ })).toBeNull(),
    );
    await waitFor(() => expect(focusedName()).toMatch(/^Plan,/));
  });

  it('keeps keyboard focus on an entry the refresh keeps', async () => {
    const tree = await focusedWhileRefreshing([sourceSummary, otherSummary]);
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).querySelector('code')?.title).toBe(
        NEW,
      ),
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(focusedName()).toMatch(/^Annual report,/);
    expect(within(tree).getByRole('treeitem', { name: /Annual report/ })).toBe(
      document.activeElement,
    );
  });

  it('says quietly that the newer-head check failed, in plain words, never as an alert (N20)', async () => {
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          if (opens === 2) return fail(500, 'internal', 'storage stumbled');
          return ok({ ...workspace, head: opens === 1 ? HEAD : NEW });
        },
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    windowRefocus();
    const failed = await screen.findByText(
      'Could not check for a newer version: Something went wrong in the service.',
    );
    expect(failed.closest('[role="status"]')).toBe(failed);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(within(screen.getByText(/Showing the workspace as of/)).getByTitle(HEAD)).toBeTruthy();
    // A Refresh resolves the head itself, so the failed check no longer applies.
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await screen.findByText(`Refreshed: now showing revision ${NEW.slice(0, 10)}.`);
    expect(screen.queryByText(/Could not check for a newer version/)).toBeNull();
  });

  it('says nothing of a check older than the head Refresh resolved since', async () => {
    const NEWER = 'f'.repeat(40);
    const heads = [HEAD, NEW, NEWER];
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          return ok({ ...workspace, head: heads[Math.min(opens, heads.length) - 1] ?? HEAD });
        },
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    windowRefocus();
    await screen.findByText(/A newer version of this workspace exists/);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await screen.findByText(`Refreshed: now showing revision ${NEWER.slice(0, 10)}.`);
    expect(screen.queryByText(/A newer version of this workspace exists/)).toBeNull();
  });

  it('keeps the Explorer when a refresh fails, and says so', async () => {
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          return opens === 1 ? ok(workspace) : fail(500, 'internal', 'storage stumbled');
        },
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Refreshing this workspace:');
    expect(screen.getByRole('tree', { name: 'Folders' })).toBeTruthy();
    expect(
      await screen.findByText(
        'The workspace could not be refreshed; still showing the version above.',
      ),
    ).toBeTruthy();
    // The failure allows a retry, and the notice offers none of its own: Refresh is the retry.
    const notice = alert.parentElement;
    if (notice === null) throw new Error('the alert is not in a notice');
    expect(notice.className).toBe('failure-notice');
    expect(within(notice).queryByRole('button')).toBeNull();
    expect(within(notice).queryByRole('status')).toBeNull();
  });

  it('moves focus from the way out of a failed refresh to Refresh when the failure goes away by itself', async () => {
    // The workspace opens, a refresh finds it gone, and it is there again when asked once more.
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          return opens === 2 ? fail(404, 'not_found', 'gone') : ok(workspace);
        },
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    const refresh = screen.getByRole('button', { name: 'Refresh' });
    await userEvent.click(refresh);
    const words = await screen.findByText('Refreshing this workspace: This could not be found.');
    const notice = words.closest<HTMLElement>('[role="alert"]')?.parentElement;
    if (notice === null || notice === undefined) throw new Error('the alert is not in a notice');
    const wayOut = within(notice).getByRole('link', { name: 'Go to your workspaces' });
    act(() => wayOut.focus());
    // The network returns: a query whose last answer was a failure is asked again.
    act(() => {
      onlineManager.setOnline(false);
      onlineManager.setOnline(true);
    });
    await waitFor(() => expect(wayOut.isConnected).toBe(false));
    expect(opens).toBe(3);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(document.activeElement).toBe(refresh);
  });
});
