/**
 * A workspace's lifecycle on its Administration page (rulings C3, C4 and C9), in Chromium and
 * WebKit, each journey once with the pointer and once from the keyboard alone (Tab, Shift+Tab,
 * Enter, Space; in WebKit a link is focused directly, the approved E8 exception):
 *
 * - archive and unarchive a workspace: one button, archived on the head the page shows; the shell
 *   then reads the workspace again and shows it archived (the name and description say why they
 *   cannot change), and the same button unarchives it;
 * - purge a workspace, then read its record: the destructive dialog says what a purge removes and
 *   what it cannot erase, keyboard focus starts on Cancel and comes back to the button that opened
 *   it, and confirming goes to the purge's record (`/purges/<id>`), read with `get_purge` alone:
 *   running when first read, then completed with every count of its report.
 *
 * Every screen has zero axe violations and visible focus, and a screenshot for the owner under
 * `.artifacts/ui/` (untracked). The service's operations are played by `page.route`
 * (`support/admin-lifecycle.ts`). Needs `bun --bun run build` first.
 */
import { expect, type Page, test } from '@playwright/test';
import { PURGE_ID, playLifecycle } from './support/admin-lifecycle';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  HEAD,
  NOTE,
  shot,
  useExplorerServer,
  WS,
} from './support/explorer';
import { tabTo } from './support/views';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const ARCHIVED =
  'Archived. Nothing in it can be changed until it is unarchived; it can still be read, exported and backed up.';
const UNARCHIVED = 'Unarchived. It can be changed again.';

const lifecycle = (page: Page) => page.getByRole('region', { name: 'This workspace' });
const archivePart = (page: Page) => lifecycle(page).getByRole('region', { name: 'Archive' });
const archiveButton = (page: Page) => archivePart(page).getByRole('button');
const saveButton = (page: Page) =>
  lifecycle(page).getByRole('button', { name: 'Save the name and description' });
const rebuildButton = (page: Page) =>
  lifecycle(page).getByRole('button', { name: 'Rebuild the search index' });
const purgeButton = (page: Page) =>
  lifecycle(page).getByRole('button', { name: 'Purge this workspace…' });
const purgeDialog = (page: Page) =>
  page.getByRole('alertdialog', { name: 'Purge this workspace?' });
const recordHeading = (page: Page) =>
  page.getByRole('heading', { level: 1, name: 'Purge 66666666' });

/** What the archived workspace's name and description say beside Save. */
async function expectArchivedSaid(page: Page) {
  await expect(lifecycle(page)).toContainText('This workspace was archived on');
  await expect(saveButton(page)).toHaveAttribute('aria-disabled', 'true');
  await expect(lifecycle(page)).toContainText(
    'nothing in it can be changed until it is unarchived. It can still be read.',
  );
}

/** The record as the service has it once the purge has completed: every count, in words. */
async function expectCompletedRecord(page: Page) {
  // The record is asked again on its clock while it runs; the service has then completed it.
  await expect(page.getByText(/^Completed: every copy the application manages/)).toBeVisible({
    timeout: 20_000,
  });
  const removed = page.getByRole('list', { name: 'What it removed' });
  await expect(removed.getByRole('listitem')).toHaveCount(11);
  await expect(removed).toContainText('1,048,576 stored objects deleted.');
  await expect(removed).toContainText(
    '12 stored objects kept, because a surviving item still references them.',
  );
  await expect(removed).toContainText(
    '0 stored citations of other items moved to a rewritten revision.',
  );
  await expect(removed).toContainText('1 managed backup rewritten without what was purged.');
}

test.describe('archive and unarchive a workspace', () => {
  const server = useExplorerServer();

  test('with the pointer: the shell shows each state, and the same button turns it back', async ({
    page,
  }) => {
    const service = await playLifecycle(page);
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    await expect(lifecycle(page)).toBeVisible();
    await expect(archiveButton(page)).toHaveText('Archive this workspace');
    await expect(lifecycle(page).getByRole('textbox', { name: 'Name' })).toHaveValue(
      'Workshop notes',
    );
    await expect(lifecycle(page)).toContainText(`as of revision ${HEAD.slice(0, 10)}`);
    await expectNoAxeViolations(page, 'this workspace');
    await shot(page, 'm4c-lifecycle-01-this-workspace');

    await archiveButton(page).click();
    await expect(archivePart(page).getByRole('status')).toHaveText(ARCHIVED);
    await expect(archiveButton(page)).toHaveText('Unarchive this workspace');
    await expectArchivedSaid(page);
    // What the service runs in an archived workspace stays offered.
    await expect(rebuildButton(page)).toHaveAttribute('aria-disabled', 'false');
    await expect(purgeButton(page)).toHaveAttribute('aria-disabled', 'false');
    expect(service.archives).toEqual([
      { workspace_id: WS, base_revision: HEAD, idempotency_key: expect.stringMatching(UUID) },
    ]);
    await expectNoAxeViolations(page, 'the workspace archived');
    await shot(page, 'm4c-lifecycle-02-archived');

    await archiveButton(page).click();
    await expect(archivePart(page).getByRole('status')).toHaveText(UNARCHIVED);
    await expect(archiveButton(page)).toHaveText('Archive this workspace');
    await expect(saveButton(page)).toHaveAttribute('aria-disabled', 'false');
    expect(service.unarchives).toEqual([
      { workspace_id: WS, idempotency_key: expect.stringMatching(UUID) },
    ]);
    await expectNoAxeViolations(page, 'the workspace unarchived');
    await shot(page, 'm4c-lifecycle-03-unarchived');
  });

  test('from the keyboard alone: the button keeps focus through each change', async ({ page }) => {
    const service = await playLifecycle(page);
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    await expect(archiveButton(page)).toHaveText('Archive this workspace');
    await tabTo(page, archiveButton(page));
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(archivePart(page).getByRole('status')).toHaveText(ARCHIVED);
    await expect(archiveButton(page)).toHaveText('Unarchive this workspace');
    await expect(archiveButton(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expectArchivedSaid(page);
    await expectNoAxeViolations(page, 'archived from the keyboard');
    await shot(page, 'm4c-lifecycle-04-archived-keyboard');

    await page.keyboard.press('Space');
    await expect(archivePart(page).getByRole('status')).toHaveText(UNARCHIVED);
    await expect(archiveButton(page)).toHaveText('Archive this workspace');
    await expect(archiveButton(page)).toBeFocused();
    await expectVisibleFocus(page);
    // Back through the section: the rebuild, then Save, which can run again.
    await page.keyboard.press('Shift+Tab');
    await expect(rebuildButton(page)).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Shift+Tab');
    await expect(saveButton(page)).toBeFocused();
    await expect(saveButton(page)).toHaveAttribute('aria-disabled', 'false');
    await expectNoAxeViolations(page, 'unarchived from the keyboard');
    await shot(page, 'm4c-lifecycle-05-unarchived-keyboard');
    expect(service.archives).toHaveLength(1);
    expect(service.unarchives).toHaveLength(1);
  });
});

test.describe('purge a workspace, then read its record', () => {
  const server = useExplorerServer();

  test('with the pointer: the dialog says what a purge removes, and the record its counts', async ({
    page,
  }) => {
    const service = await playLifecycle(page);
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    await expect(purgeButton(page)).toBeVisible();
    await expect(lifecycle(page)).toContainText(
      'Purging a workspace is done only by an administrator in their own browser session',
    );
    await purgeButton(page).click();
    const dialog = purgeDialog(page);
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(
      'A purge removes the original bytes, derivatives, index entries, historical content and installation-managed backups of this workspace',
    );
    await expect(dialog).toContainText(
      'A purge never erases exports, clones or backups outside the application’s control',
    );
    await expect(dialog).toContainText(`as of revision ${HEAD.slice(0, 10)}`);
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expectNoAxeViolations(page, 'the purge’s dialog');
    await shot(page, 'm4c-lifecycle-06-purge-dialog');

    await dialog.getByRole('button', { name: 'Purge the workspace' }).click();
    await expect(page).toHaveURL(`${server.origin()}/purges/${PURGE_ID}`);
    await expect(recordHeading(page)).toBeVisible();
    await expect(page.getByText('Running: removing what it covers.')).toBeVisible();
    await expect(page.getByRole('region', { name: 'Purge 66666666' })).toContainText(
      `What it removes: the workspace ${WS}.`,
    );
    expect(service.purges).toEqual([
      { workspace_id: WS, base_revision: HEAD, idempotency_key: expect.stringMatching(UUID) },
    ]);
    await expectNoAxeViolations(page, 'the record while the purge runs');
    await shot(page, 'm4c-lifecycle-07-record-running');

    await expectCompletedRecord(page);
    await expectNoAxeViolations(page, 'the record of the completed purge');
    await shot(page, 'm4c-lifecycle-08-record-completed');
    // One press, one purge.
    expect(service.purges).toHaveLength(1);
  });

  test('from the keyboard alone: Escape and Cancel give focus back, Enter purges, focus rests on the record', async ({
    page,
    browserName,
  }) => {
    const service = await playLifecycle(page);
    await page.goto(`${server.origin()}/w/${WS}/admin`);
    await expect(purgeButton(page)).toBeVisible();
    await tabTo(page, purgeButton(page));
    await expectVisibleFocus(page);

    // Escape leaves without acting, and focus is back where it was.
    await page.keyboard.press('Enter');
    const dialog = purgeDialog(page);
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(purgeButton(page)).toBeFocused();

    await page.keyboard.press('Space');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    const confirm = dialog.getByRole('button', { name: 'Purge the workspace' });
    await expect(confirm).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the purge’s dialog from the keyboard');
    await shot(page, 'm4c-lifecycle-09-purge-dialog-keyboard');
    expect(service.purges).toEqual([]);

    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(`${server.origin()}/purges/${PURGE_ID}`);
    // The button that opened the dialog left with the workspace's page: focus is on the record's heading.
    await expect(recordHeading(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expectCompletedRecord(page);
    await expectNoAxeViolations(page, 'the record, reached from the keyboard');
    await shot(page, 'm4c-lifecycle-10-record-keyboard');
    expect(service.purges).toHaveLength(1);

    // The way back to the installation, from the keyboard.
    const back = page.getByRole('link', { name: 'Your workspaces' });
    if (browserName === 'webkit') {
      // WebKit leaves links out of the Tab order (E8): the link is focused directly.
      await back.focus();
    } else {
      await tabTo(page, back, 10);
    }
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(`${server.origin()}/`);
  });
});

test.describe('purge an item', () => {
  const server = useExplorerServer();

  test('from the item’s own actions, from the keyboard: not available in this build, nothing removed', async ({
    page,
  }) => {
    const service = await playLifecycle(page);
    const purges: Record<string, unknown>[] = [];
    page.on('request', (request) => {
      if (new URL(request.url()).pathname === '/api/items/purge-item') {
        purges.push(JSON.parse(request.postData() ?? '{}'));
      }
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const opener = page.getByRole('button', { name: 'Purge this item…' });
    await expect(opener).toBeVisible();
    await tabTo(page, opener);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('alertdialog', { name: 'Purge this item?' });
    await expect(dialog).toContainText('The item “Meeting notes” (notes/meeting.md');
    await expect(dialog).toContainText(
      'A purge never erases exports, clones or backups outside the application’s control',
    );
    // Focus has moved into the dialog, onto Cancel, before the keyboard goes on from there.
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    await expect(dialog.getByRole('button', { name: 'Purge the item' })).toBeFocused();
    await page.keyboard.press('Enter');
    // `purge_item` is not built on main: the service answers `not_implemented`.
    await expect(dialog.getByRole('alert')).toContainText(
      'Purging this item: Not available in this build.',
    );
    await expect(dialog.getByRole('alert')).toContainText('Nothing was done.');
    expect(purges).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    await expect(dialog.getByRole('button', { name: 'Purge the item' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'an item purge, not available in this build');
    await shot(page, 'm4c-lifecycle-11-item-purge-not-available');
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(opener).toBeFocused();
    await expect(page).toHaveURL(`${server.origin()}/w/${WS}/i/${NOTE}`);
    expect(service.purges).toEqual([]);
  });
});
