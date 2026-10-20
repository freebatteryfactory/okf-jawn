/**
 * Export a View (ruling B3, "Export visual"), in Chromium and WebKit: once with the pointer, and
 * once from the keyboard alone (Tab, Enter; Option+Tab to a link in WebKit, as Safari reaches
 * links). The View's tab offers "Export visual", which asks for the View at the revision the tab
 * reads, never the latest; the service answers 202 with a queued job; the Jobs page lists it, and
 * the next listing answers it succeeded with its archive, whose row offers a plain link to the
 * archive's download path with its kind, size and SHA-256 (ruling B4). The browser downloads the
 * archive, and what it saved has the SHA-256 the row shows. Every screen has zero axe violations
 * and visible focus, and a screenshot for the owner under `.artifacts/ui/` (untracked).
 *
 * The View is the one `support/views.ts` answers for the saved-View journeys; the archive is served
 * by `support/exports.ts`'s static server at its download path, on the page's own origin, because
 * `page.route` does not see a `download` link's request.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { expect, type Page, test } from '@playwright/test';
import {
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
import { tabTo, VIEW, viewAnswers } from './support/views';

type Body = Record<string, unknown>;

const JOB = '88888888-8888-4888-8888-888888888888';
const ARTIFACT = 'aaaaaaaa-9999-4999-8999-999999999999';
const ARTIFACT_PATH = `/api/workspaces/${WS}/artifacts/${ARTIFACT}`;
const FILENAME = 'revenue-by-quarter.zip';

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
  kind: 'export_view',
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
    kind: 'view_export',
    sha256: ARCHIVE_SHA,
    size: String(ARCHIVE.length),
    download_path: ARTIFACT_PATH,
  },
});

/** Every `export_view` request, in order, and the listings answered since the first. */
interface Service {
  exports: Body[];
  listedSince: number;
}

/**
 * The service: the saved View of the Views journeys in a workspace the person may change; no jobs
 * until the View is exported; the export answered queued, listed queued once, then succeeded.
 */
async function play(page: Page): Promise<Service> {
  const service: Service = { exports: [], listedSince: 0 };
  await routeApi(
    page,
    viewAnswers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      '/api/views/export-view': (body) => {
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
    }),
  );
  return service;
}

const view = (page: Page) => page.getByRole('region', { name: 'The View' });
const exportButton = (page: Page) => view(page).getByRole('button', { name: 'Export visual' });
const jobsLink = (page: Page) =>
  view(page).getByRole('link', { name: 'All jobs of this workspace' });
const row = (page: Page) => page.locator(`li[data-job="${JOB}"]`);
const downloadLink = (page: Page) =>
  row(page).getByRole('link', { name: 'Download the export of the View' });
const accepted =
  'The export of this View was accepted as Export of a View, job 88888888: “waiting to start” when the service answered. The Jobs page shows how it stands now.';

/** The one request: this View, at the revision its tab reads, under a retry identity. */
function expectOneExport(service: Service) {
  expect(service.exports).toEqual([
    {
      workspace_id: WS,
      item_id: VIEW,
      at: { kind: 'revision', revision: HEAD },
      idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
    },
  ]);
}

/** What the finished export's row says of its archive, and where its link leads. */
async function expectArchiveOffered(page: Page) {
  await expect(downloadLink(page)).toBeVisible();
  await expect(downloadLink(page)).toHaveAttribute('href', ARTIFACT_PATH);
  await expect(downloadLink(page)).toHaveAttribute('download', '');
  await expect(row(page)).toContainText('It produced an export of a View: 3,072 bytes (3.0 KB).');
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

test.describe('export a View', () => {
  const server = useDownloadServer({ [ARTIFACT_PATH]: { body: ARCHIVE, filename: FILENAME } });

  test('with the pointer: the View is exported at its tab’s revision, and downloaded', async ({
    page,
  }) => {
    const service = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${VIEW}`);
    await expect(view(page).getByRole('table', { name: 'revenue' })).toHaveCount(1);
    await expect(exportButton(page)).toBeVisible();
    await expectNoAxeViolations(page, 'a View with Export visual');
    await shot(page, 'm4b-export-view-01-offered');

    await exportButton(page).click();
    await expect(view(page).getByRole('status')).toHaveText(accepted);
    expectOneExport(service);
    await expectNoAxeViolations(page, 'a View export accepted');
    await shot(page, 'm4b-export-view-02-accepted');

    await jobsLink(page).click();
    expect(new URL(page.url()).pathname).toBe(`/w/${WS}/jobs`);
    await expect(row(page)).toContainText('Export of a View, job 88888888');
    // The list is asked again on its clock while the job waits; the service then has finished it.
    await expectArchiveOffered(page);
    expectOneExport(service);
    await expectNoAxeViolations(page, 'a finished View export with its download');
    await shot(page, 'm4b-export-view-03-finished');

    await expectSaved(page, () => downloadLink(page).click());
    expect(server.downloads(ARTIFACT_PATH)).toBe(1);
  });

  test('from the keyboard alone', async ({ page, browserName }) => {
    // WebKit, like Safari by default, leaves links out of the Tab order; Option+Tab reaches them.
    const toLink = browserName === 'webkit' ? 'Alt+Tab' : 'Tab';
    const service = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${VIEW}`);
    await expect(view(page).getByRole('table', { name: 'revenue' })).toHaveCount(1);

    await tabTo(page, exportButton(page));
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(view(page).getByRole('status')).toHaveText(accepted);
    expectOneExport(service);

    await tabTo(page, jobsLink(page), 150, toLink);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(row(page)).toContainText('Export of a View, job 88888888');
    expect(await page.evaluate(() => document.activeElement === document.body)).toBe(false);

    await expectArchiveOffered(page);
    await tabTo(page, downloadLink(page), 150, toLink);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a finished View export, from the keyboard');
    await shot(page, 'm4b-export-view-04-keyboard');
    await expectSaved(page, () => page.keyboard.press('Enter'));
    expect(server.downloads(ARTIFACT_PATH)).toBe(1);
    expectOneExport(service);
  });
});
