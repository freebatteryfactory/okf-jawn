/**
 * A navigation that takes the focused element off the page ends with keyboard focus on the new
 * page's level-one heading, not on the page body. It never acts on the first load, when focus was
 * on the body before the navigation, or when focus survived or was already placed by the page.
 */

import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fail, mockApi, ok } from './support/api';
import { explorerHandlers, HEAD, NOTE, OLD, renderAt, WS, workspace } from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const WORKSPACES = '/api/workspaces/list-workspaces';
const OPEN = '/api/workspaces/open-workspace';
const GET = '/api/items/get-item';
const READ = '/api/reads/read-item';

const listed = () => ok({ items: [workspace], next_cursor: null });
const pageHeadingNamed = (name: string) => screen.getByRole('heading', { level: 1, name });

/** The notice that says these words, with the control in it that has this role. */
async function controlOfNotice(words: string, role: 'button' | 'link') {
  const said = await screen.findByText(words);
  const notice = said.closest<HTMLElement>('[role="alert"]')?.parentElement;
  if (notice === null || notice === undefined) throw new Error('the alert is not in a notice');
  return within(notice).getByRole(role);
}

describe('a navigation that removes the focused element', () => {
  it('ends on the workspace’s name after Back from the reader with focus on its retry button', async () => {
    mockApi(explorerHandlers({ [READ]: () => fail(500, 'internal', 'storage stumbled') }));
    const { router } = renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
      }),
    );
    const retry = await controlOfNotice(
      'Reading this item: Something went wrong in the service.',
      'button',
    );
    act(() => retry.focus());
    act(() => router.history.back());
    await waitFor(() => expect(retry.isConnected).toBe(false));
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    await waitFor(() => expect(document.activeElement).toBe(pageHeadingNamed('Workshop notes')));
  });

  it('ends on the workspace’s name after the way out of a failed read is activated from the keyboard', async () => {
    mockApi(explorerHandlers({ [GET]: () => fail(404, 'not_found', 'gone') }));
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    const wayOut = await controlOfNotice(
      'Reading this item’s properties: This could not be found.',
      'link',
    );
    expect(wayOut.textContent).toBe('Go to this workspace');
    act(() => wayOut.focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(wayOut.isConnected).toBe(false));
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    await waitFor(() => expect(document.activeElement).toBe(pageHeadingNamed('Workshop notes')));
  });

  it('ends on the list’s heading after the way out of a workspace that is gone is activated from the keyboard', async () => {
    mockApi(
      explorerHandlers({ [OPEN]: () => fail(404, 'not_found', 'gone'), [WORKSPACES]: listed }),
    );
    const { router } = renderAt(`/w/${WS}`);
    const wayOut = await controlOfNotice(
      'Opening this workspace: This could not be found.',
      'link',
    );
    expect(wayOut.textContent).toBe('Go to your workspaces');
    act(() => wayOut.focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(wayOut.isConnected).toBe(false));
    expect(router.state.location.pathname).toBe('/');
    await waitFor(() => expect(document.activeElement).toBe(pageHeadingNamed('Your workspaces')));
  });
});

describe('the heading a navigation ended on, when the page replaces it', () => {
  it('hands focus on to the workspace’s name when a workspace opened from the list by keyboard has opened', async () => {
    // The workspace answers when the test says so: until then the page before the Explorer shows.
    let opened: (reply: ReturnType<typeof ok>) => void = () => {};
    mockApi(
      explorerHandlers({
        [WORKSPACES]: listed,
        [OPEN]: () =>
          new Promise((resolve) => {
            opened = resolve;
          }),
      }),
    );
    renderAt('/');
    const link = await screen.findByRole('link', { name: 'Workshop notes' });
    act(() => link.focus());
    await userEvent.keyboard('{Enter}');
    await screen.findByText('Opening the workspace…');
    // The list is gone with the link: the navigation ends on the heading of the page shown now.
    const opening = pageHeadingNamed('Workspace');
    await waitFor(() => expect(document.activeElement).toBe(opening));

    opened(ok(workspace));
    await screen.findByRole('tree', { name: 'Folders' });
    expect(opening.isConnected).toBe(false);
    expect(document.activeElement).toBe(pageHeadingNamed('Workshop notes'));
  });
});

describe('a navigation that does not take focus anywhere', () => {
  it.each([
    ['the workspace list', '/', () => screen.findByRole('link', { name: 'Workshop notes' })],
    ['a workspace', `/w/${WS}`, () => screen.findByRole('tree', { name: 'Folders' })],
  ] as const)('leaves focus on the body on the first load of %s', async (_page, url, shown) => {
    mockApi(explorerHandlers({ [WORKSPACES]: listed }));
    renderAt(url);
    await shown();
    // The page has its heading, and nothing has put focus on it.
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(document.activeElement).toBe(document.body);
  });

  it('leaves focus on the body when it was there before the navigation', async () => {
    mockApi(explorerHandlers({ [WORKSPACES]: listed }));
    const { router } = renderAt('/');
    await screen.findByRole('link', { name: 'Workshop notes' });
    // A pointer press that focuses nothing (WebKit does not focus a link it is pressed on).
    expect(document.activeElement).toBe(document.body);
    await act(() => router.navigate({ to: '/w/$workspaceId', params: { workspaceId: WS } }));
    await screen.findByRole('tree', { name: 'Folders' });
    expect(document.activeElement).toBe(document.body);
  });

  it('leaves focus on the body when it was there before this navigation, though an earlier one began in the page', async () => {
    mockApi(explorerHandlers({ [WORKSPACES]: listed }));
    const { router } = renderAt('/');
    const link = await screen.findByRole('link', { name: 'Workshop notes' });
    // The first navigation begins with focus in the page and ends on the workspace's name.
    act(() => link.focus());
    await userEvent.keyboard('{Enter}');
    await screen.findByRole('tree', { name: 'Folders' });
    const name = pageHeadingNamed('Workshop notes');
    await waitFor(() => expect(document.activeElement).toBe(name));
    // Focus is on the body when the second navigation begins: what the first one saw is not kept.
    act(() => name.blur());
    expect(document.activeElement).toBe(document.body);
    await act(() => router.navigate({ to: '/' }));
    await screen.findByRole('link', { name: 'Workshop notes' });
    expect(document.activeElement).toBe(document.body);
  });

  it('leaves focus on a tree entry that opens an item from the keyboard and stays on the page', async () => {
    mockApi(explorerHandlers());
    const { router } = renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    const entry = await within(tree).findByRole('treeitem', { name: /Annual report/ });
    act(() => entry.focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(router.state.location.pathname).toContain('/i/'));
    await screen.findByRole('tab', { name: /Annual report/ });
    expect(entry.isConnected).toBe(true);
    expect(document.activeElement).toBe(entry);
  });

  it('leaves focus where the page has put it in the same change: the reader’s revision notice, when another revision is read', async () => {
    // The first request opens the workspace; the reader's own check of the revision fails.
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          return opens === 2 ? fail(500, 'internal', 'storage stumbled') : ok(workspace);
        },
      }),
    );
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const retry = await controlOfNotice(
      'Checking for the latest version: Something went wrong in the service.',
      'button',
    );
    const revisionNotice = retry.closest<HTMLElement>('.pinned-revision');
    act(() => retry.focus());
    // The address changes under the page (Back or Forward): the reader stays, its check is new.
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
        search: { rev: HEAD },
      }),
    );
    await waitFor(() => expect(retry.isConnected).toBe(false));
    await screen.findByText(/the latest version when checked at/);
    expect(revisionNotice?.isConnected).toBe(true);
    expect(document.activeElement).toBe(revisionNotice);
  });
});
