/**
 * Read the security events (ruling C8), in Chromium and WebKit: once with the pointer, and once
 * from the keyboard alone (Tab, Enter). On the installation's Administration page the first page
 * of its log is listed in words (a sign-in, a connector issued, twelve refused sign-ins counted as
 * one event); "Show more events" asks the next page by the service's cursor, only when pressed,
 * and lists the rest (a refused operation, a refused download counted by the minute, a connector
 * revoked), with the job's progress counted, not listed; at the end, "Look for newer events" asks
 * after the last event listed. A workspace's own security events, its refused requests, are on its
 * Administration page. Every screen has zero axe violations and visible focus, and a screenshot
 * for the owner under `.artifacts/ui/` (untracked).
 *
 * The service's operations are played by `page.route` (no server exists in this lane).
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import { adminWorkspace, localOwner, tenantLog, workspacePage } from './support/admin-security';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  ok,
  routeApi,
  shot,
  useExplorerServer,
  WS,
} from './support/explorer';
import { tabTo } from './support/views';

type Body = Record<string, unknown>;

const rows = (region: Locator) => region.getByRole('list').first().getByRole('listitem');

/** The installation's Administration page over its log; the first page listed. */
async function openInstallation(page: Page, origin: string) {
  const asked: Body[] = [];
  await routeApi(page, {
    '/api/session/get-session': () => ok(localOwner),
    ...tenantLog(asked),
  });
  await page.goto(`${origin}/admin`);
  const region = page.getByRole('region', { name: 'Security events of the installation' });
  await expect(rows(region)).toHaveCount(3);
  return { asked, region };
}

/** What the first page says, in words. */
async function expectFirstPage(region: Locator) {
  await expect(rows(region).nth(0)).toContainText('A person signed in');
  await expect(rows(region).nth(0)).toContainText(
    'local-owner, as the installation’s local owner, in the browser on this computer',
  );
  await expect(rows(region).nth(1)).toContainText('A connector was issued');
  await expect(rows(region).nth(1)).toContainText('connector 77777777');
  await expect(rows(region).nth(2)).toContainText('A sign-in was refused');
  await expect(rows(region).nth(2)).toContainText(
    '12 refused sign-ins in this minute, recorded as one event.',
  );
}

/** What the second page added, in words. */
async function expectSecondPage(region: Locator) {
  await expect(rows(region)).toHaveCount(6);
  await expect(rows(region).nth(3)).toContainText('A request was refused');
  await expect(rows(region).nth(3)).toContainText('reading the installation’s security events');
  await expect(rows(region).nth(3)).toContainText('with client desktop-app');
  await expect(rows(region).nth(4)).toContainText('downloading a backup of the installation');
  await expect(rows(region).nth(4)).toContainText(
    '3 refused requests in this minute, recorded as one event.',
  );
  await expect(rows(region).nth(5)).toContainText('A connector was revoked');
  await expect(region).toContainText('1 other event: 1 job update.');
  await expect(region).toContainText('That is every event recorded when the last page was read.');
}

test.describe('read the security events', () => {
  const server = useExplorerServer();

  test('with the pointer: a page, then “Show more events”', async ({ page }) => {
    const { asked, region } = await openInstallation(page, server.origin());
    await expectFirstPage(region);
    await expectNoAxeViolations(page, 'the first page of the installation’s events');
    await shot(page, 'm4c-events-01-first-page');
    // Nothing more is asked until the person asks.
    expect(asked).toEqual([{ page: { limit: 50 } }]);

    await region.getByRole('button', { name: 'Show more events' }).click();
    await expectSecondPage(region);
    await expect(rows(region).nth(3)).toBeFocused();
    expect(asked).toEqual([{ page: { limit: 50 } }, { page: { limit: 50, cursor: '3' } }]);
    await expectNoAxeViolations(page, 'the next page');
    await shot(page, 'm4c-events-02-next-page');

    await region.getByRole('button', { name: 'Look for newer events' }).click();
    await expect(region.getByText('No newer events were recorded.')).toBeVisible();
    expect(asked.at(2)).toEqual({ page: { limit: 50 }, after: '7' });
    await expect(rows(region)).toHaveCount(6);
  });

  test('from the keyboard alone: Tab and Enter', async ({ page }) => {
    const { asked, region } = await openInstallation(page, server.origin());
    const more = region.getByRole('button', { name: 'Show more events' });
    await tabTo(page, more);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expectSecondPage(region);
    // Focus moves to the first event the page brought.
    await expect(rows(region).nth(3)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the next page, from the keyboard');
    await shot(page, 'm4c-events-03-keyboard-next-page');

    const newer = region.getByRole('button', { name: 'Look for newer events' });
    await tabTo(page, newer, 20);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(region.getByText('No newer events were recorded.')).toBeVisible();
    await expect(newer).toBeFocused();
    await expectVisibleFocus(page);
    expect(asked).toHaveLength(3);
  });

  test('a workspace’s refused requests, on its Administration page', async ({ page }) => {
    const asked: Body[] = [];
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(adminWorkspace),
      '/api/session/get-session': () => ok(localOwner),
      '/api/session/list-connectors': () => ok({ connectors: [] }),
      '/api/events/list-events': (body) => {
        asked.push(body);
        return ok(workspacePage);
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    const region = page.getByRole('region', { name: 'Security events' });
    await expect(rows(region)).toHaveCount(2);
    await expect(rows(region).nth(0)).toContainText('purging a workspace');
    await expect(rows(region).nth(0)).toContainText(
      'agent-7, through an MCP client (a connector or a delegated agent), with client desktop-app',
    );
    await expect(rows(region).nth(1)).toContainText(
      'downloading an export or a backup of a workspace',
    );
    await expect(rows(region).nth(1)).toContainText(
      '40 refused requests in this minute, recorded as one event.',
    );
    await expect(region).toContainText('1 other event: 1 content change.');
    expect(asked).toEqual([{ workspace_id: WS, page: { limit: 50 } }]);
    const newer = region.getByRole('button', { name: 'Look for newer events' });
    await tabTo(page, newer);
    await expectVisibleFocus(page);
    await region.scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'a workspace’s security events');
    await shot(page, 'm4c-events-04-workspace');
  });
});
