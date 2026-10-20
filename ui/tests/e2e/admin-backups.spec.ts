/**
 * Back up a workspace and restore one (rulings C6 and C3), in Chromium and WebKit: once with the
 * pointer, and once from the keyboard alone (Tab, Shift+Tab and Enter; a link focused directly in
 * WebKit, which leaves links out of the Tab order).
 *
 * On the workspace's Administration, "Back up this workspace" asks `backup_workspace`: the job is
 * accepted, followed in the section until the service reports it finished, and its row then
 * offers the archive as a plain link. The browser downloads it, and what it saved has the SHA-256
 * the row shows. In a blank workspace, that saved archive is chosen, uploaded (a slot, the bytes,
 * the finish, which the service checks against the length and SHA-256 it computes), and restored
 * through the destructive dialog, which says exactly what is acted on. The restore's job is
 * followed until it is finished, and what it did is read in words. Every screen has zero axe
 * violations and visible focus, and a screenshot for the owner under `.artifacts/ui/` (untracked).
 *
 * The services are played by `page.route` (`support/admin-backups.ts`). A browser fetches a
 * `download` link's target as a download, which `page.route` does not see, so the archive is
 * served for real at its download path on the page's own origin (`support/exports.ts`); the bytes
 * of the upload go to a server of the spec's own, which reads what the browser really sent.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { readFile } from 'node:fs/promises';
import { expect, type Page, test } from '@playwright/test';
import {
  BACKUP_ARCHIVE,
  BACKUP_JOB,
  BACKUP_PATH,
  BLANK,
  blankWorkspace,
  play,
  REPORT,
  RESTORE_JOB,
  sha256Of,
  useUploadSink,
  WS,
} from './support/admin-backups';
import { expectNoAxeViolations, expectVisibleFocus, focused, shot } from './support/explorer';
import { useDownloadServer } from './support/exports';
import { tabTo } from './support/views';

const FILENAME = 'workshop-notes-backup.zip';
const ARCHIVE_SHA = sha256Of(BACKUP_ARCHIVE);

const backups = (page: Page) => page.getByRole('region', { name: 'Backups' });
const restorePart = (page: Page) => page.getByRole('region', { name: 'Restore from a backup' });
const backupButton = (page: Page) =>
  backups(page).getByRole('button', { name: 'Back up this workspace' });
const backupRow = (page: Page) => page.locator(`li[data-job="${BACKUP_JOB}"]`);
const downloadLink = (page: Page) =>
  backupRow(page).getByRole('link', { name: 'Download the backup of the workspace' });
const chooseButton = (page: Page) =>
  restorePart(page).getByRole('button', { name: 'Choose a backup archive…' });
const uploadButton = (page: Page) =>
  restorePart(page).getByRole('button', { name: 'Upload the archive' });
const restoreButton = (page: Page) =>
  restorePart(page).getByRole('button', { name: 'Restore this workspace from it…' });
const reportList = (page: Page) =>
  restorePart(page).getByRole('list', { name: 'What the restore did' });
const dialog = (page: Page) =>
  page.getByRole('alertdialog', { name: 'Restore this workspace from this archive?' });

/** Keyboard focus 300 ms on, after the frame in which a browser moves it off a removed control. */
async function focusSettled(page: Page): Promise<string> {
  await page.waitForTimeout(300);
  return focused(page);
}

/** The backup was accepted, followed until finished, and its row offers the archive. */
async function expectBackupOffered(page: Page) {
  await expect(backups(page).getByRole('status').first()).toHaveText(
    'The backup was accepted as Backup of the workspace, job bbbbbbbb: “waiting to start” when the service answered. Its row below shows how it stands now; once finished, it offers the download.',
  );
  await expect(downloadLink(page)).toBeVisible();
  await expect(downloadLink(page)).toHaveAttribute('href', BACKUP_PATH);
  await expect(downloadLink(page)).toHaveAttribute('download', '');
  await expect(backupRow(page)).toContainText('Finished.');
  await expect(backupRow(page)).toContainText(
    'It produced a backup of the workspace: 4,096 bytes (4.0 KB).',
  );
  await expect(backupRow(page)).toContainText(`SHA-256: ${ARCHIVE_SHA}`);
  await expect(backups(page)).toContainText('It left nothing out of the archive.');
}

/** The browser saves the archive the row describes; answers where it saved it. */
async function expectSaved(page: Page, follow: () => Promise<void>): Promise<string> {
  const downloading = page.waitForEvent('download');
  await follow();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe(FILENAME);
  const path = await download.path();
  const saved = await readFile(path);
  expect(saved.length).toBe(BACKUP_ARCHIVE.length);
  expect(sha256Of(saved)).toBe(ARCHIVE_SHA);
  return path;
}

/** The archive was uploaded and the service confirmed it. */
async function expectUploaded(page: Page) {
  await expect(restorePart(page)).toContainText(
    `Uploaded, and the service checked its length and SHA-256: ${FILENAME}, 4,096 bytes (4.0 KB). SHA-256: ${ARCHIVE_SHA}`,
  );
}

/** What the restore did, in words, once its job finished. */
async function expectReport(page: Page) {
  await expect(
    restorePart(page).getByText('Restore of the workspace, job cccccccc: finished.'),
  ).toBeVisible();
  const said = reportList(page);
  await expect(said).toContainText(`It restored ${REPORT.items} items.`);
  await expect(said).toContainText('3 drafts were given back to their editors.');
  await expect(said).toContainText(
    '1 draft stays in the archive, unassigned: its editor is not a known identity here.',
  );
  await expect(said).toContainText(
    '2 records were not restored: the archive’s own backup had left them out.',
  );
  await expect(restorePart(page)).toContainText('press Refresh at the top of the page');
}

test.describe('back up a workspace and restore one', () => {
  const server = useDownloadServer({ [BACKUP_PATH]: { body: BACKUP_ARCHIVE, filename: FILENAME } });
  const sink = useUploadSink();

  test('with the pointer: the backup downloads, and restores into a blank workspace', async ({
    page,
  }) => {
    const service = await play(page, { sink: sink.origin(), route: 'browser_session' });
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    await expect(backupButton(page)).toBeVisible();
    await expect(backups(page)).toContainText(
      'Backing up this workspace is done only by an administrator in their own browser session',
    );
    await expectNoAxeViolations(page, 'the backups offered');
    await shot(page, 'm4c-backups-01-offered');

    await backupButton(page).click();
    await expectBackupOffered(page);
    expect(service.sent.backup_workspace).toEqual([
      { workspace_id: WS, idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/) },
    ]);
    // The pane scrolls by itself: the row is brought into view for the screenshot.
    await downloadLink(page).scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'a finished backup with its download');
    await shot(page, 'm4c-backups-02-backup-finished');
    const saved = await expectSaved(page, () => downloadLink(page).click());
    expect(server.downloads(BACKUP_PATH)).toBe(1);

    // In a blank workspace, the archive just saved is restored.
    await page.goto(`${server.origin()}/w/${BLANK}/admin`);
    await expect(page.getByRole('heading', { level: 1, name: blankWorkspace.name })).toBeVisible();
    const choosing = page.waitForEvent('filechooser');
    await chooseButton(page).click();
    await (await choosing).setFiles({
      name: FILENAME,
      mimeType: 'application/zip',
      buffer: await readFile(saved),
    });
    await expect(
      restorePart(page).getByText(`Chosen: ${FILENAME}, 4,096 bytes (4.0 KB).`),
    ).toBeVisible();
    await uploadButton(page).click();
    await expectUploaded(page);
    expect(service.sent.create_upload).toEqual([
      {
        workspace_id: BLANK,
        filename: FILENAME,
        relative_path: '',
        size: '4096',
        sha256: ARCHIVE_SHA,
        idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
      },
    ]);
    expect(sha256Of(Buffer.concat(service.slots.at(0)?.parts ?? []))).toBe(ARCHIVE_SHA);
    await restoreButton(page).scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'the archive uploaded');
    await shot(page, 'm4c-backups-03-archive-uploaded');

    await restoreButton(page).click();
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page)).toContainText(FILENAME);
    await expect(dialog(page)).toContainText('4,096 bytes (4.0 KB)');
    await expect(dialog(page)).toContainText(ARCHIVE_SHA);
    await expect(dialog(page)).toContainText(`this workspace, ${blankWorkspace.name}`);
    await expect(dialog(page).getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expectNoAxeViolations(page, 'the restore dialog');
    await shot(page, 'm4c-backups-04-restore-dialog');

    await dialog(page).getByRole('button', { name: 'Restore the workspace' }).click();
    await expect(dialog(page)).toBeHidden();
    await expect(
      restorePart(page).getByText(
        'The restore was accepted as Restore of the workspace, job cccccccc.',
      ),
    ).toBeVisible();
    await expectReport(page);
    expect(service.sent.restore_workspace).toEqual([
      {
        workspace_id: BLANK,
        upload_id: service.slots.at(0)?.id,
        sha256: ARCHIVE_SHA,
        idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
      },
    ]);
    expect(service.asked.get(RESTORE_JOB)).toBeGreaterThanOrEqual(2);
    await reportList(page).scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'the restore’s report');
    await shot(page, 'm4c-backups-05-restore-report');
  });

  test('from the keyboard alone: Tab, Shift+Tab and Enter', async ({ page, browserName }) => {
    const service = await play(page, { sink: sink.origin(), route: 'browser_session' });
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    await expect(backupButton(page)).toBeVisible();

    await tabTo(page, backupButton(page));
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expectBackupOffered(page);
    // The button kept keyboard focus through the answer and the job's progress.
    await expect(backupButton(page)).toBeFocused();
    if (browserName === 'webkit') {
      // WebKit, like Safari by default, leaves links out of Tab order (a platform setting the page
      // cannot change: see E8 in explorer-keyboard.spec.ts), so the link is focused directly.
      await downloadLink(page).focus();
    } else {
      await tabTo(page, downloadLink(page), 20);
    }
    await expect(downloadLink(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the download link, from the keyboard');
    await shot(page, 'm4c-backups-06-keyboard-download');
    const saved = await expectSaved(page, () => page.keyboard.press('Enter'));

    await page.goto(`${server.origin()}/w/${BLANK}/admin`);
    await expect(chooseButton(page)).toBeVisible();
    await tabTo(page, chooseButton(page));
    await expectVisibleFocus(page);
    const choosing = page.waitForEvent('filechooser');
    await page.keyboard.press('Enter');
    await (await choosing).setFiles({
      name: FILENAME,
      mimeType: 'application/zip',
      buffer: await readFile(saved),
    });
    await expect(
      restorePart(page).getByText(`Chosen: ${FILENAME}, 4,096 bytes (4.0 KB).`),
    ).toBeVisible();
    await tabTo(page, uploadButton(page), 5);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expectUploaded(page);
    // The upload button left once the archive was uploaded: focus went to what comes next.
    expect(await focusSettled(page)).toBe('button:Restore this workspace from it…');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the archive uploaded, from the keyboard');

    await page.keyboard.press('Enter');
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page).getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Shift+Tab');
    await expect(dialog(page).getByRole('button', { name: 'Restore the workspace' })).toBeFocused();
    await expectVisibleFocus(page);
    await shot(page, 'm4c-backups-07-keyboard-dialog');
    await page.keyboard.press('Enter');
    await expect(dialog(page)).toBeHidden();
    // Focus went back to what opened the dialog, which says that the restore took the upload.
    await expect(restoreButton(page)).toBeFocused();
    await expect(restoreButton(page)).toHaveAttribute('aria-disabled', 'true');
    await expectVisibleFocus(page);
    await expectReport(page);
    expect(service.sent.restore_workspace).toHaveLength(1);
    await reportList(page).scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'the restore’s report, from the keyboard');
    await shot(page, 'm4c-backups-08-keyboard-report');
  });
});
