/**
 * Settings (ruling B6): a page of the workspace's bar that holds the WebMCP setting. It is per
 * browser, off by default, kept in `localStorage`, and a browser whose storage is blocked acts as
 * off and says the choice could not be kept. It says in plain words what turning it on allows.
 */
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WEBMCP_SETTING_KEY } from '../../src/features/settings/webmcp-setting';
import { mockApi } from './support/api';
import { blockStorage, FIVE, installModelContext, removeModelContext } from './support/webmcp';
import { explorerHandlers, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
  removeModelContext();
  window.localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const SETTING = 'Let an assistant in this browser use the page tools (WebMCP)';

async function openSettings() {
  renderAt(`/w/${WS}`);
  fireEvent.click(await screen.findByRole('link', { name: 'Settings' }));
  return screen.findByRole('region', { name: 'Settings' });
}

describe('Settings', { timeout: 30_000 }, () => {
  it('is a page of the workspace’s bar, with its own address and a level-two heading', async () => {
    const api = mockApi(explorerHandlers());
    const { router } = renderAt(`/w/${WS}`);
    const link = await screen.findByRole('link', { name: 'Settings' });
    expect(link.closest('nav')?.getAttribute('aria-label')).toBe('Workspace');
    fireEvent.click(link);
    const page = await screen.findByRole('region', { name: 'Settings' });
    expect(within(page).getByRole('heading', { level: 2, name: 'Settings' })).toBeTruthy();
    expect(router.state.location.pathname).toBe(`/w/${WS}/settings`);
    expect(link.getAttribute('aria-current')).toBe('page');
    expect(api.violations).toEqual([]);
  });

  it('says the setting is this browser’s, off by default, and what turning it on allows', async () => {
    mockApi(explorerHandlers());
    installModelContext();
    const page = await openSettings();
    const box = within(page).getByRole('checkbox', { name: SETTING });
    expect((box as HTMLInputElement).checked).toBe(false);
    const words = page.textContent ?? '';
    expect(words).toContain('kept in this browser, not in the workspace');
    for (const name of FIVE) expect(words).toContain(name);
    expect(words).toContain('act with your own session');
    expect(words).toContain('None of them can verify, approve or review anything.');
    expect(words).toContain('It never sends it');
    expect(words).toContain('does nothing unless this browser offers WebMCP');
    expect(words).not.toContain('This browser does not offer WebMCP');
  });

  it('keeps the choice in this browser’s storage, and reads it back on the next visit', async () => {
    mockApi(explorerHandlers());
    installModelContext();
    const page = await openSettings();
    const box = within(page).getByRole('checkbox', { name: SETTING });
    fireEvent.click(box);
    await waitFor(() => expect((box as HTMLInputElement).checked).toBe(true));
    expect(window.localStorage.getItem(WEBMCP_SETTING_KEY)).toBe('on');
    expect(within(page).getByRole('status').textContent).toContain('Turned on');
    cleanup();
    const again = await openSettings();
    const kept = within(again).getByRole('checkbox', { name: SETTING });
    expect((kept as HTMLInputElement).checked).toBe(true);
    fireEvent.click(kept);
    await waitFor(() => expect((kept as HTMLInputElement).checked).toBe(false));
    expect(window.localStorage.getItem(WEBMCP_SETTING_KEY)).toBeNull();
    expect(within(again).getByRole('status').textContent).toContain('Turned off');
  });

  it('follows a change made in another tab of this browser without a reload', async () => {
    mockApi(explorerHandlers());
    const page = await openSettings();
    const box = within(page).getByRole('checkbox', { name: SETTING });
    expect((box as HTMLInputElement).checked).toBe(false);
    act(() => {
      window.localStorage.setItem(WEBMCP_SETTING_KEY, 'on');
      window.dispatchEvent(
        new StorageEvent('storage', { key: WEBMCP_SETTING_KEY, newValue: 'on' }),
      );
    });
    await waitFor(() => expect((box as HTMLInputElement).checked).toBe(true));
  });

  it('says when this browser does not offer WebMCP, and still keeps the choice', async () => {
    mockApi(explorerHandlers());
    removeModelContext();
    const page = await openSettings();
    expect(page.textContent).toContain('This browser does not offer WebMCP');
    const box = within(page).getByRole('checkbox', { name: SETTING });
    fireEvent.click(box);
    await waitFor(() => expect((box as HTMLInputElement).checked).toBe(true));
    expect(window.localStorage.getItem(WEBMCP_SETTING_KEY)).toBe('on');
  });

  it('acts as off where storage is blocked, and says plainly that turning it on could not be kept', async () => {
    mockApi(explorerHandlers());
    installModelContext();
    const unblock = blockStorage();
    try {
      const page = await openSettings();
      const box = within(page).getByRole('checkbox', { name: SETTING });
      expect((box as HTMLInputElement).checked).toBe(false);
      fireEvent.click(box);
      await waitFor(() =>
        expect(within(page).getByRole('status').textContent).toContain(
          'could not be kept in this browser',
        ),
      );
      expect((box as HTMLInputElement).checked).toBe(false);
    } finally {
      unblock();
    }
  });
});
