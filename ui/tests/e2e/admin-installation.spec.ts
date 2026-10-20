/**
 * Create a workspace and open it (ruling C9), and back up the installation (ruling C6), in
 * Chromium and WebKit: once with the pointer, and once from the keyboard alone (Tab and Enter; a
 * link focused directly in WebKit, which leaves links out of the Tab order).
 *
 * The workspace list offers "New workspace" to the local owner; it leads to the form on the
 * installation's Administration with the name field focused. Creating the workspace asks
 * `create_workspace` under an idempotency key, and the new workspace opens. "Back up the
 * installation" asks `backup_installation`; the installation's jobs list the job, follow it until
 * the service reports it finished, and its row then offers the archive at the installation's own
 * path; the browser downloads it, and what it saved has the SHA-256 the row shows. Every screen has
 * zero axe violations and visible focus, and a screenshot for the owner under `.artifacts/ui/`
 * (untracked).
 *
 * The services are played by `page.route` (`support/admin-backups.ts`); the archive is served for
 * real at its download path on the page's own origin (`support/exports.ts`).
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { readFile } from 'node:fs/promises';
import { expect, type Page, test } from '@playwright/test';
import {
  CREATED,
  INSTALLATION_ARCHIVE,
  INSTALLATION_JOB,
  INSTALLATION_PATH,
  play,
  sha256Of,
} from './support/admin-backups';
import { expectNoAxeViolations, expectVisibleFocus, shot } from './support/explorer';
import { useDownloadServer } from './support/exports';
import { tabTo } from './support/views';

const FILENAME = 'installation-backup.zip';
const ARCHIVE_SHA = sha256Of(INSTALLATION_ARCHIVE);

const newLink = (page: Page) => page.getByRole('link', { name: 'New workspace' });
const form = (page: Page) => page.getByRole('region', { name: 'New workspace' });
const nameField = (page: Page) => form(page).getByRole('textbox', { name: 'Name' });
const descriptionField = (page: Page) => form(page).getByRole('textbox', { name: 'Description' });
const createButton = (page: Page) =>
  form(page).getByRole('button', { name: 'Create the workspace' });
const jobRow = (page: Page) => page.locator(`li[data-job="${INSTALLATION_JOB}"]`);
const downloadLink = (page: Page) =>
  jobRow(page).getByRole('link', { name: 'Download the backup of the installation' });

/** The new workspace opened: its address, and its name as the page's heading. */
async function expectOpened(page: Page, name: string) {
  await expect(page).toHaveURL(new RegExp(`/w/${CREATED}$`));
  await expect(page.getByRole('heading', { level: 1, name })).toBeVisible();
}

test.describe('create a workspace and open it', () => {
  const server = useDownloadServer({
    [INSTALLATION_PATH]: { body: INSTALLATION_ARCHIVE, filename: FILENAME },
  });

  test('with the pointer: from the workspace list to the new workspace', async ({ page }) => {
    const service = await play(page);
    await page.goto(`${server.origin()}/`);
    await expect(newLink(page)).toBeVisible();
    await expect(newLink(page)).toHaveAttribute('href', '/admin#new-workspace');
    await expectNoAxeViolations(page, 'the workspace list with “New workspace”');
    await shot(page, 'm4c-installation-01-list');

    await newLink(page).click();
    await expect(nameField(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expect(form(page)).toContainText('no example content');
    await expectNoAxeViolations(page, 'the new workspace form');
    await shot(page, 'm4c-installation-02-form');

    await nameField(page).fill('Field notes');
    await descriptionField(page).fill('Notes from the field trip');
    await createButton(page).click();
    await expectOpened(page, 'Field notes');
    expect(service.sent.create_workspace).toEqual([
      {
        name: 'Field notes',
        description: 'Notes from the field trip',
        idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
      },
    ]);
    await expectNoAxeViolations(page, 'the new workspace, opened');
    await shot(page, 'm4c-installation-03-opened');
  });

  test('from the keyboard alone: Tab and Enter', async ({ page, browserName }) => {
    const service = await play(page);
    await page.goto(`${server.origin()}/`);
    await expect(newLink(page)).toBeVisible();
    if (browserName === 'webkit') {
      // WebKit, like Safari by default, leaves links out of Tab order (a platform setting the page
      // cannot change: see E8 in explorer-keyboard.spec.ts), so the link is focused directly.
      await newLink(page).focus();
    } else {
      await tabTo(page, newLink(page), 20);
    }
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(nameField(page)).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.type('Field notes');
    await page.keyboard.press('Tab');
    await expect(descriptionField(page)).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.type('Notes from the field trip');
    await page.keyboard.press('Tab');
    await expect(createButton(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the new workspace form, from the keyboard');
    await shot(page, 'm4c-installation-04-keyboard-form');
    await page.keyboard.press('Enter');
    await expectOpened(page, 'Field notes');
    // Focus went to the new workspace's heading, never to the page body.
    await expect(page.getByRole('heading', { level: 1, name: 'Field notes' })).toBeFocused();
    expect(service.sent.create_workspace).toHaveLength(1);
  });
});

test.describe('back up the installation', () => {
  const server = useDownloadServer({
    [INSTALLATION_PATH]: { body: INSTALLATION_ARCHIVE, filename: FILENAME },
  });

  test('its job is listed, followed until finished, and its archive downloads', async ({
    page,
    browserName,
  }) => {
    const service = await play(page);
    await page.goto(`${server.origin()}/admin`);
    const section = page.getByRole('region', { name: 'Backup of the installation' });
    const button = section.getByRole('button', { name: 'Back up the installation' });
    await expect(page.getByText('This installation has no jobs yet.')).toBeVisible();
    await tabTo(page, button);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(section.getByRole('status')).toHaveText(
      'The backup of the installation was accepted as Backup of the installation, job dddddddd: “waiting to start” when the service answered. Its row under “Installation jobs” shows how it stands now, and offers the download once it has finished.',
    );
    expect(service.sent.backup_installation).toEqual([
      { idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/) },
    ]);
    await expect(downloadLink(page)).toBeVisible();
    await expect(downloadLink(page)).toHaveAttribute('href', INSTALLATION_PATH);
    await expect(jobRow(page)).toContainText('Finished.');
    await expect(jobRow(page)).toContainText(`SHA-256: ${ARCHIVE_SHA}`);
    await expect(button).toBeFocused();
    await expectNoAxeViolations(page, 'the installation’s backup, finished');
    await shot(page, 'm4c-installation-05-backup-finished');

    if (browserName === 'webkit') {
      await downloadLink(page).focus();
    } else {
      await tabTo(page, downloadLink(page), 20);
    }
    await expectVisibleFocus(page);
    const downloading = page.waitForEvent('download');
    await page.keyboard.press('Enter');
    const download = await downloading;
    expect(download.suggestedFilename()).toBe(FILENAME);
    const saved = await readFile(await download.path());
    expect(sha256Of(saved)).toBe(ARCHIVE_SHA);
    expect(server.downloads(INSTALLATION_PATH)).toBe(1);
  });
});
