/**
 * Where Administration lives (ruling C1). A workspace's bar offers "Administration" only to a
 * person whose grant there includes `admin`; its page is the workspace's own, under its name. The
 * installation's Administration is reached from the workspace list, offered only to a person who
 * holds `admin` on a workspace listed there or is the installation's local owner: the contract
 * names no tenant grant a client can read, so these are what the page knows.
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { sessionOptions } from '../../src/features/imports/limits';
import { adminWorkspace, SESSION, sessionOn, WORKSPACES, workspacesPage } from './support/admin';
import { mockApi, ok } from './support/api';
import { explorerHandlers, renderAt, WS, workspace } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const bar = () => screen.findByRole('navigation', { name: 'Workspace' });

describe('a workspace’s Administration (C1)', { timeout: 30_000 }, () => {
  it('is a page of the bar for a person who holds admin there, with its own address', async () => {
    const api = mockApi(
      explorerHandlers({ '/api/workspaces/open-workspace': () => ok(adminWorkspace) }),
    );
    const { router } = renderAt(`/w/${WS}`);
    const link = await within(await bar()).findByRole('link', { name: 'Administration' });
    fireEvent.click(link);
    const page = await screen.findByRole('region', { name: 'Administration' });
    expect(within(page).getByRole('heading', { level: 2, name: 'Administration' })).toBeTruthy();
    expect(router.state.location.pathname).toBe(`/w/${WS}/admin`);
    expect(link.getAttribute('aria-current')).toBe('page');
    expect(api.violations).toEqual([]);
  });

  it('is not offered to a person whose grant there has no admin', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}`);
    const nav = await bar();
    expect(within(nav).getByRole('link', { name: 'Settings' })).toBeTruthy();
    expect(within(nav).queryByRole('link', { name: 'Administration' })).toBeNull();
  });

  it('says so in words at its address for a person who holds no admin there', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/admin`);
    const page = await screen.findByRole('region', { name: 'Administration' });
    expect(page.textContent).toContain(
      'Administering this workspace needs administrator rights here, which you do not have.',
    );
  });
});

describe('the installation’s Administration (C1)', { timeout: 30_000 }, () => {
  const homeLink = () => screen.findByRole('link', { name: 'Administration of this installation' });

  it('is offered from the workspace list to a person who holds admin on a listed workspace', async () => {
    const api = mockApi({
      [WORKSPACES]: () => ok(workspacesPage([workspace, adminWorkspace])),
      [SESSION]: () => ok(sessionOn('browser_session')),
    });
    const { router } = renderAt('/');
    fireEvent.click(await homeLink());
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Administration of this installation' }),
    ).toBeTruthy();
    expect(router.state.location.pathname).toBe('/admin');
    expect(api.violations).toEqual([]);
  });

  it('is offered to the local owner, whose installation it is, before any workspace exists', async () => {
    mockApi({
      [WORKSPACES]: () => ok(workspacesPage([])),
      [SESSION]: () => ok(sessionOn('local_owner')),
    });
    renderAt('/');
    expect(await homeLink()).toBeTruthy();
  });

  it('is not offered to anyone else', async () => {
    const api = mockApi({
      [WORKSPACES]: () => ok(workspacesPage([workspace])),
      [SESSION]: () => ok(sessionOn('browser_session')),
    });
    const { queryClient } = renderAt('/');
    expect(await screen.findByRole('link', { name: workspace.name })).toBeTruthy();
    // Both answers are in: nothing more will offer it.
    await waitFor(() =>
      expect(queryClient.getQueryState(sessionOptions().queryKey)?.status).toBe('success'),
    );
    expect(api.callsTo(SESSION)).toHaveLength(1);
    expect(screen.queryByRole('link', { name: 'Administration of this installation' })).toBeNull();
  });
});
