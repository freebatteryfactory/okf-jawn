/**
 * Export a workspace and download it (rulings B3 and B4), in Chromium and WebKit: once with the
 * pointer, and once from the keyboard alone (Tab, Space and Enter). "Export this workspace" on the
 * Jobs page asks for the workspace as it is now (`at: latest`), with its history when asked; the
 * service answers 202 with a queued job, which the list shows; the next listing answers it
 * succeeded with its archive, and its row offers a plain link to the archive's download path, with
 * the archive's kind, size and SHA-256. The browser then downloads the archive, and what it saved
 * has the SHA-256 the row shows. Every screen has zero axe violations and visible focus, and a
 * screenshot for the owner under `.artifacts/ui/` (untracked).
 *
 * The service's operations are played by `page.route` (no server exists in this lane). The archive
 * is a valid empty zip whose comment fills it to 3,072 bytes. A browser fetches a `download` link's
 * target as a download, which `page.route` does not see, so the spec's static server serves the
 * archive at its download path, as an attachment, on the page's own origin (`support/exports.ts`).
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  type Answers,
  expectNoAxeViolations,
  expectVisibleFocus,
  HEAD,
  ok,
  routeApi,
  shot,
  WS,
  writableWorkspace,
} from './support/explorer';
import { useDownloadServer } from './support/exports';

type Body = Record<string, unknown>;

const JOB = '77777777-7777-4777-8777-777777777777';
const ARTIFACT = '99999999-9999-4999-8999-999999999999';
const ARTIFACT_PATH = `/api/workspaces/${WS}/artifacts/${ARTIFACT}`;
const FILENAME = 'workshop-notes-export.zip';

/** An empty zip archive (its end record alone) with a comment that makes it `size` bytes. */
function emptyZip(size: number): Buffer {
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(size - 22, 20);
  return Buffer.concat([end, Buffer.alloc(size - 22, 0x20)]);
}

const ARCHIVE = emptyZip(3072);
const ARCHIVE_SHA = createHash('sha256').update(ARCHIVE).digest('hex');

const jobOf = (state: string, extra: Body = {}): Body => ({
  id: JOB,
  kind: 'export_workspace',
  state,
  attempt: 1,
  progress: state === 'succeeded' ? 100 : 0,
  item_ids: [],
  warnings: [],
  workspace_id: WS,
  created_at: '2026-10-19T08:00:00.000Z',
  ...extra,
});

const finished = jobOf('succeeded', {
  started_at: '2026-10-19T08:00:01.000Z',
  finished_at: '2026-10-19T08:00:04.000Z',
  artifact: {
    artifact_id: ARTIFACT,
    kind: 'export',
    sha256: ARCHIVE_SHA,
    size: String(ARCHIVE.length),
    download_path: ARTIFACT_PATH,
  },
});

interface Service {
  /** Every `export_workspace` request, in order. */
  exports: Body[];
  /** Listings answered since the export was accepted. */
  listedSince: number;
}

/**
 * The service: no jobs until an export is accepted; the export answered queued, and listed
 * queued once, then succeeded with its archive.
 */
async function play(page: Page, extra: Answers = {}): Promise<Service> {
  const service: Service = { exports: [], listedSince: 0 };
  await routeApi(page, {
    ...extra,
    '/api/workspaces/open-workspace': () => ok(writableWorkspace),
    '/api/workspaces/export-workspace': (body) => {
      service.exports.push(body);
      return { status: 202, body: jobOf('queued') };
    },
    '/api/imports/list-jobs': () => {
      if (service.exports.length === 0) return ok({ items: [], next_cursor: null });
      service.listedSince += 1;
      return ok({
        items: [service.listedSince === 1 ? jobOf('queued') : finished],
        next_cursor: null,
      });
    },
  });
  return service;
}

const group = (page: Page) => page.getByRole('group', { name: 'Export this workspace' });
const row = (page: Page) => page.locator(`li[data-job="${JOB}"]`);
const downloadLink = (page: Page) =>
  row(page).getByRole('link', { name: 'Download the export of the workspace' });

/** Press Tab until `target` has keyboard focus. */
async function tabTo(page: Page, target: Locator, guard = 80) {
  for (let presses = 0; presses < guard; presses += 1) {
    if (await target.evaluate((element) => element === document.activeElement)) return;
    await page.keyboard.press('Tab');
  }
  await expect(target).toBeFocused();
}

/** What the finished export's row says of its archive, and where its link leads. */
async function expectArchiveOffered(page: Page) {
  await expect(downloadLink(page)).toBeVisible();
  await expect(downloadLink(page)).toHaveAttribute('href', ARTIFACT_PATH);
  await expect(downloadLink(page)).toHaveAttribute('download', '');
  await expect(row(page)).toContainText('Finished.');
  await expect(row(page)).toContainText(
    'It produced an export of the workspace: 3,072 bytes (3.0 KB).',
  );
  await expect(row(page)).toContainText(`SHA-256: ${ARCHIVE_SHA}`);
}

/** The browser saved the archive the row describes. */
async function expectSaved(page: Page, follow: () => Promise<void>) {
  const downloading = page.waitForEvent('download');
  await follow();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe(FILENAME);
  const saved = await readFile(await download.path());
  expect(saved.length).toBe(3072);
  expect(createHash('sha256').update(saved).digest('hex')).toBe(ARCHIVE_SHA);
}

test.describe('export a workspace and download it', () => {
  const server = useDownloadServer({ [ARTIFACT_PATH]: { body: ARCHIVE, filename: FILENAME } });

  test('with the pointer: the export is a job, and its finished row offers the archive', async ({
    page,
  }) => {
    const service = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/jobs`);
    await expect(group(page)).toBeVisible();
    await expect(group(page)).toContainText('as it is now');
    await expect(page.getByText(/This workspace has no jobs yet\./)).toBeVisible();
    await expectNoAxeViolations(page, 'the export offered');
    await shot(page, 'm4b-export-01-offered');

    await group(page).getByRole('checkbox', { name: 'Include the workspace’s history' }).check();
    await group(page).getByRole('button', { name: 'Export the workspace' }).click();
    await expect(group(page).getByRole('status')).toHaveText(
      'The export was accepted as Export of the workspace, job 77777777: “waiting to start” when the service answered. Its row in the list below shows how it stands now.',
    );
    expect(service.exports).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'latest' },
        include_history: true,
        idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
      },
    ]);
    await expect(row(page)).toContainText('Export of the workspace, job 77777777');
    await expectNoAxeViolations(page, 'the export accepted');
    await shot(page, 'm4b-export-02-accepted');

    // The list is asked again on its clock while the job waits; the service then has finished it.
    await expectArchiveOffered(page);
    await expect(page.getByText('Export of the workspace, job 77777777: finished.')).toBeVisible();
    // One press, one export.
    expect(service.exports).toHaveLength(1);
    await expectNoAxeViolations(page, 'a finished export with its download');
    await shot(page, 'm4b-export-03-finished');

    await expectSaved(page, () => downloadLink(page).click());
    expect(server.downloads(ARTIFACT_PATH)).toBe(1);
  });

  test('from the keyboard alone: Tab, Space and Enter', async ({ page, browserName }) => {
    const service = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/jobs`);
    await expect(group(page)).toBeVisible();

    const history = group(page).getByRole('checkbox', { name: 'Include the workspace’s history' });
    await tabTo(page, history);
    await expectVisibleFocus(page);
    await page.keyboard.press('Space');
    await expect(history).toBeChecked();
    await page.keyboard.press('Tab');
    const button = group(page).getByRole('button', { name: 'Export the workspace' });
    await expect(button).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(group(page).getByRole('status')).toContainText(
      'The export was accepted as Export of the workspace, job 77777777: “waiting to start”',
    );
    expect(service.exports.map((body) => body.include_history)).toEqual([true]);
    // The button stays, with keyboard focus, through the answer and the listing.
    await expect(button).toBeFocused();
    await expectNoAxeViolations(page, 'the export accepted, from the keyboard');

    await expectArchiveOffered(page);
    await expect(button).toBeFocused();
    if (browserName === 'webkit') {
      // WebKit, like Safari by default, leaves links out of Tab order (a platform setting the
      // page cannot change: see E8 in explorer-keyboard.spec.ts), so the link is focused
      // directly; Enter still follows it.
      await downloadLink(page).focus();
    } else {
      await tabTo(page, downloadLink(page), 10);
    }
    await expect(downloadLink(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the download link, from the keyboard');
    await shot(page, 'm4b-export-04-keyboard');

    await expectSaved(page, () => page.keyboard.press('Enter'));
    expect(service.exports).toHaveLength(1);
    expect(server.downloads(ARTIFACT_PATH)).toBe(1);
  });
});

/** A version of the workspace's history before the head the page shows. */
const EARLIER = 'c'.repeat(40);
const versions = {
  '/api/history/log-items': () =>
    ok({
      commits: [
        {
          revision: HEAD,
          parents: [EARLIER],
          message: 'Publish the notes',
          author: 'ada',
          committed_at: '2026-01-01T12:00:00.000Z',
        },
        {
          revision: EARLIER,
          parents: [],
          message: 'Start the notes',
          author: 'ada',
          committed_at: '2026-01-01T09:00:00.000Z',
        },
      ],
      next_cursor: null,
    }),
};

test.describe('export a version of the workspace’s history', () => {
  const server = useDownloadServer({ [ARTIFACT_PATH]: { body: ARCHIVE, filename: FILENAME } });

  test('chosen from the keyboard alone: the export asks that revision', async ({
    page,
    browserName,
  }) => {
    const service = await play(page, versions);
    await page.goto(`${server.origin()}/w/${WS}/jobs`);
    const now = group(page).getByRole('radio', { name: 'As it is now' });
    await expect(now).toBeChecked();
    await tabTo(page, now);
    await expectVisibleFocus(page);
    // Arrow keys move the choice within the group, as for every radio group.
    await page.keyboard.press('ArrowDown');
    await expect(
      group(page).getByRole('radio', { name: 'A version from its history' }),
    ).toBeChecked();
    const list = group(page).getByRole('combobox', { name: 'Version' });
    await expect(list).toHaveValue(HEAD);
    await tabTo(page, list);
    await expectVisibleFocus(page);
    if (browserName === 'webkit') {
      // WebKit opens a native menu for a list from the keyboard, which the page cannot drive; the
      // choice is made as that menu would make it.
      await list.selectOption(EARLIER);
    } else {
      await page.keyboard.press('ArrowDown');
    }
    await expect(list).toHaveValue(EARLIER);
    await expect(group(page)).toContainText(`as of revision ${EARLIER.slice(0, 10)}`);
    await expectNoAxeViolations(page, 'a version of the history chosen');
    await shot(page, 'm4b-export-05-version-chosen');

    const button = group(page).getByRole('button', { name: 'Export the workspace' });
    await tabTo(page, button);
    await page.keyboard.press('Enter');
    await expect(group(page).getByRole('status')).toContainText('The export was accepted');
    expect(service.exports.map((body) => body.at)).toEqual([
      { kind: 'revision', revision: EARLIER },
    ]);
    await expect(button).toBeFocused();
  });
});
