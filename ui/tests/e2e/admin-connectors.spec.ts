/**
 * Issue a connector and revoke it (rulings C5 and C3), in Chromium and WebKit: once with the
 * pointer, and once from the keyboard alone (Tab, Shift+Tab, Enter and Space). On the workspace's
 * Administration page the local owner names a connector, allows it to propose, and issues it; the
 * secret is shown exactly once, with how to put it in an MCP client, and is gone once hidden and
 * after a reload, which shows the connector without it. Revoking it asks through the alert dialog
 * bound to it; once revoked, the list says so when revoked connectors are shown. Every screen has
 * zero axe violations and visible focus, and a screenshot for the owner under `.artifacts/ui/`
 * (untracked).
 *
 * The service's operations are played by `page.route` (no server exists in this lane), as a local
 * installation keeps connectors (`support/admin-security.ts`).
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  adminWorkspace,
  connectorService,
  DESKTOP,
  localOwner,
  MADE,
  SECRET,
} from './support/admin-security';
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

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** The workspace's Administration page over a connector service; the connectors listed. */
async function open(page: Page, origin: string) {
  const service = connectorService();
  await routeApi(page, {
    '/api/workspaces/open-workspace': () => ok(adminWorkspace),
    '/api/session/get-session': () => ok(localOwner),
    '/api/events/list-events': () => ok({ events: [], next_cursor: null }),
    ...service.answers,
  });
  await page.goto(`${origin}/w/${WS}/admin`);
  const region = page.getByRole('region', { name: 'Connectors' });
  await expect(rows(region)).toHaveCount(1);
  await expect(region.getByText('You are the local owner here.')).toBeVisible();
  return { service, region };
}

const rows = (region: Locator) =>
  region.getByRole('list', { name: 'Connectors that read this workspace' }).getByRole('listitem');
const row = (region: Locator, name: string) => rows(region).filter({ hasText: name });

/** Nothing the page keeps holds the secret: not the page, its address or its storage. */
async function expectSecretKeptNowhere(page: Page) {
  expect(await page.content()).not.toContain(SECRET);
  expect(page.url()).not.toContain(SECRET);
  const stored = await page.evaluate(() =>
    JSON.stringify([{ ...localStorage }, { ...sessionStorage }]),
  );
  expect(stored).not.toContain(SECRET);
}

test.describe('issue a connector and revoke it', () => {
  const server = useExplorerServer();

  test('with the pointer: the secret once, then revoked', async ({ page }) => {
    const { service, region } = await open(page, server.origin());
    await expect(region).toContainText(
      'Only the installation’s local owner, in their own browser on this computer, issues, lists and revokes connectors.',
    );
    await expectNoAxeViolations(page, 'the connectors listed');
    await rows(region).first().scrollIntoViewIfNeeded();
    await shot(page, 'm4c-connectors-01-listed');

    await region.getByRole('textbox', { name: 'Name of the connector' }).fill('Research agent');
    await region.getByRole('checkbox', { name: 'Also allow proposing' }).check();
    await region.getByRole('button', { name: 'Issue a connector' }).click();
    const secretBox = region.getByRole('group', { name: 'The secret of “Research agent”' });
    await expect(secretBox).toBeFocused();
    await expect(secretBox.getByRole('textbox', { name: 'Secret' })).toHaveValue(SECRET);
    await expect(secretBox).toContainText(
      'It is shown only this once: it will not be shown again, here or anywhere.',
    );
    await expect(secretBox).toContainText('okf-jawn mcp --stdio');
    await expect(secretBox).toContainText(`${server.origin()}/mcp`);
    expect(service.issues).toEqual([
      {
        label: 'Research agent',
        allow_propose: true,
        workspace_ids: [WS],
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    await expect(row(region, 'Research agent')).toContainText(
      'May read and propose changes; it can never verify or approve anything.',
    );
    await expectNoAxeViolations(page, 'the secret, shown once');
    await secretBox.scrollIntoViewIfNeeded();
    await shot(page, 'm4c-connectors-02-secret-once');

    await secretBox.getByRole('button', { name: 'Hide the secret' }).click();
    await expect(secretBox).toHaveCount(0);
    await expectSecretKeptNowhere(page);

    await row(region, 'Research agent')
      .getByRole('button', { name: 'Revoke “Research agent”…' })
      .click();
    const dialog = page.getByRole('alertdialog', {
      name: 'Revoke the connector “Research agent”?',
    });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(MADE);
    await expect(dialog).toContainText('This cannot be undone');
    await expectNoAxeViolations(page, 'the revocation dialog');
    await shot(page, 'm4c-connectors-03-revoke-dialog');
    await dialog.getByRole('button', { name: 'Revoke it' }).click();
    await expect(dialog).toHaveCount(0);
    await expect(
      region.getByText('“Research agent” was revoked: its MCP client is refused from now on.'),
    ).toBeVisible();
    await expect(rows(region)).toHaveCount(1);
    expect(service.revocations).toEqual([
      { connector_id: MADE, idempotency_key: expect.stringMatching(UUID) },
    ]);

    await region.getByRole('checkbox', { name: 'Show revoked connectors too' }).check();
    await expect(rows(region)).toHaveCount(2);
    await expect(row(region, 'Research agent')).toContainText('Revoked on');
    await expect(row(region, 'Research agent').getByRole('button')).toHaveCount(0);
    await expectNoAxeViolations(page, 'the revoked connector shown');
    await row(region, 'Research agent').scrollIntoViewIfNeeded();
    await shot(page, 'm4c-connectors-04-revoked-shown');

    // A reload: the connector is listed, its secret is not anywhere.
    await page.reload();
    await expect(rows(region)).toHaveCount(1);
    await expect(row(region, 'Desktop app')).toBeVisible();
    await expect(page.getByRole('textbox', { name: 'Secret' })).toHaveCount(0);
    await expectSecretKeptNowhere(page);
    expect(service.connectors.map((connector) => connector.connector_id)).toEqual([DESKTOP, MADE]);
  });

  test('from the keyboard alone: Tab, Shift+Tab, Enter and Space', async ({ page }) => {
    const { service, region } = await open(page, server.origin());

    const name = region.getByRole('textbox', { name: 'Name of the connector' });
    await tabTo(page, name);
    await expectVisibleFocus(page);
    await page.keyboard.type('Research agent');
    await page.keyboard.press('Tab');
    const propose = region.getByRole('checkbox', { name: 'Also allow proposing' });
    await expect(propose).toBeFocused();
    await page.keyboard.press('Space');
    await expect(propose).toBeChecked();
    await page.keyboard.press('Tab');
    const issue = region.getByRole('button', { name: 'Issue a connector' });
    await expect(issue).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');

    const secretBox = region.getByRole('group', { name: 'The secret of “Research agent”' });
    await expect(secretBox).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Tab');
    await expect(secretBox.getByRole('textbox', { name: 'Secret' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the secret, from the keyboard');
    await secretBox.scrollIntoViewIfNeeded();
    await shot(page, 'm4c-connectors-05-keyboard-secret');
    await tabTo(page, secretBox.getByRole('button', { name: 'Hide the secret' }), 5);
    await page.keyboard.press('Enter');
    await expect(secretBox).toHaveCount(0);
    await expect(issue).toBeFocused();
    expect(service.issues).toHaveLength(1);

    const revoke = row(region, 'Research agent').getByRole('button', {
      name: 'Revoke “Research agent”…',
    });
    await tabTo(page, revoke, 20);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('alertdialog', {
      name: 'Revoke the connector “Research agent”?',
    });
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Shift+Tab');
    await expect(dialog.getByRole('button', { name: 'Revoke it' })).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(dialog).toHaveCount(0);
    // The button that opened it went with the revocation: focus is on the section's heading.
    await expect(region.getByRole('heading', { level: 3, name: 'Connectors' })).toBeFocused();
    await expectVisibleFocus(page);
    await expect(
      region.getByText('“Research agent” was revoked: its MCP client is refused from now on.'),
    ).toBeVisible();

    const revoked = region.getByRole('checkbox', { name: 'Show revoked connectors too' });
    await tabTo(page, revoked, 40);
    await page.keyboard.press('Space');
    await expect(revoked).toBeChecked();
    await expect(row(region, 'Research agent')).toContainText('Revoked on');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the revoked connector, from the keyboard');
    await row(region, 'Research agent').scrollIntoViewIfNeeded();
    await shot(page, 'm4c-connectors-06-keyboard-revoked');
    expect(service.revocations).toHaveLength(1);
  });
});
