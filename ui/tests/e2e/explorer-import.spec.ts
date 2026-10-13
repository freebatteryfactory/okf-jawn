/**
 * Bringing files in, the jobs and Attention, from the keyboard, in Chromium and WebKit: choosing
 * a folder and seeing each file's path; an upload that fails once and is retried; an import that
 * becomes a job and then links to its items; an unsupported file shown as unsupported; a job
 * cancelled; the Attention list with warnings, and empty. Every journey ends with keyboard focus
 * on a sensible element and zero axe violations, and saves a screenshot of its screen under
 * `.artifacts/ui/` for the owner.
 *
 * The service is played by `page.route` (no server exists in this lane). It keeps the rules of
 * the real upload store: a slot is unique on its key, bytes are appended, and the finish is
 * refused unless the bytes that arrived have the announced length and SHA-256, which it computes
 * itself. It records the CSRF header of every request, the byte upload included.
 *
 * The bytes of an upload are received by a small HTTP server of the spec's own (`useUploadSink`):
 * Playwright does not show a route the body of a request whose body is a file, so the route sends
 * the browser's own request on to that server, which reads what the browser really sent.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, type Page, test } from '@playwright/test';
import {
  type Answers,
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  NOTE,
  note,
  ok,
  type Reply,
  readAnswer,
  routeApi,
  SOURCE,
  shot,
  source,
  summary,
  useExplorerServer,
  WS,
  writableWorkspace,
} from './support/explorer';

type Body = Record<string, unknown>;

const TOKEN = '5f1d7c0e9a4b4c2e8d3f6a1b0c9e8d7f';
const JOB = '88888888-8888-4888-8888-888888888888';
/** The revision the import made. */
const IMPORTED = 'f'.repeat(40);
/** An imported file no converter reads. */
const DRAWING = '66666666-6666-4666-8666-666666666666';
const drawing = {
  ...summary(DRAWING, 'incoming/plan.dwg.md', 'plan.dwg', 'drawing', 'source'),
  extraction: { status: 'unsupported', text_origin: 'none', corrected: false },
  media_type: 'application/acad',
  revision: IMPORTED,
};

const refuse = (status: number, code: string, message: string): Reply => ({
  status,
  body: { code, message },
});

interface Slot {
  id: string;
  key: string;
  expected: number;
  parts: Buffer[];
  complete: boolean;
  folder: string;
  filename: string;
}

interface Service {
  answers: Answers;
  slots: Slot[];
  /** Every `start_import`, `cancel_job` and `retry_job` request. */
  sent: Array<{ path: string; body: Body }>;
  /** Byte uploads that are yet to fail, as a broken connection. */
  failUploads: number;
  /** The CSRF header of each byte upload that arrived. */
  uploadCsrf: Array<string | undefined>;
  /** The job as the service now reports it; null before an import started. */
  job: Body | null;
  warnings: Body[];
}

const jobOf = (state: string, extra: Body = {}): Body => ({
  id: JOB,
  kind: 'import',
  state,
  attempt: 1,
  progress: state === 'succeeded' ? 100 : 40,
  item_ids: [],
  warnings: [],
  workspace_id: WS,
  ...extra,
});

const viewOf = (slot: Slot) => ({
  id: slot.id,
  workspace_id: WS,
  upload_path: `/api/workspaces/${WS}/uploads/${slot.id}/content`,
  received_bytes: String(slot.parts.reduce((sum, part) => sum + part.length, 0)),
  complete: slot.complete,
});

/** The service the test now running plays; the upload sink appends to its slots. */
let playing: Service | null = null;

/**
 * Receive the bytes of each upload for the specs of the calling describe block: append them to the
 * slot the request names, and answer the slot as it then stands.
 */
function useUploadSink(): { origin: () => string } {
  let origin = '';
  let close: () => void = () => {};
  test.beforeAll(async () => {
    const sink = createServer((request, response) => {
      // WebKit holds a request sent on to another address to the rules of a cross-origin one.
      const cors = {
        'access-control-allow-origin': request.headers.origin ?? '*',
        'access-control-allow-credentials': 'true',
        'access-control-allow-methods': 'PUT',
        'access-control-allow-headers': 'content-type, x-okf-csrf',
      };
      if (request.method === 'OPTIONS') {
        response.writeHead(204, cors);
        response.end();
        return;
      }
      const chunks: Buffer[] = [];
      request.on('data', (chunk: Buffer) => chunks.push(chunk));
      request.on('end', () => {
        const slot = playing?.slots.find((candidate) => request.url?.includes(candidate.id));
        if (!slot || request.method !== 'PUT') {
          response.writeHead(404, { ...cors, 'content-type': 'application/json' });
          response.end('{"code":"not_found","message":"no such upload"}');
          return;
        }
        slot.parts.push(Buffer.concat(chunks));
        response.writeHead(200, { ...cors, 'content-type': 'application/json' });
        response.end(JSON.stringify(viewOf(slot)));
      });
    });
    await new Promise<void>((resolve) => sink.listen(0, '127.0.0.1', () => resolve()));
    const address = sink.address();
    if (!address || typeof address === 'string') throw new Error('sink address unavailable');
    origin = `http://127.0.0.1:${address.port}`;
    close = () => sink.close();
  });
  test.afterAll(() => close());
  return { origin: () => origin };
}

/** A service that remembers its upload slots and one job; `sink` receives the byte uploads. */
async function play(
  page: Page,
  sink: string,
  options: { job?: Body; warnings?: Body[] } = {},
): Promise<Service> {
  const service: Service = {
    slots: [],
    sent: [],
    failUploads: 0,
    uploadCsrf: [],
    job: options.job ?? null,
    warnings: options.warnings ?? [],
    answers: {},
  };
  const view = viewOf;
  playing = service;
  service.answers = {
    '/api/workspaces/open-workspace': () => ok(writableWorkspace),
    '/api/imports/create-upload': (body) => {
      const known = service.slots.find((slot) => slot.key === body.idempotency_key);
      if (known) return ok(view(known));
      const slot: Slot = {
        id: `77777777-7777-4777-8777-${String(service.slots.length + 1).padStart(12, '0')}`,
        key: String(body.idempotency_key),
        expected: Number(body.size),
        parts: [],
        complete: false,
        folder: String(body.relative_path),
        filename: String(body.filename),
      };
      service.slots.push(slot);
      return ok(view(slot));
    },
    '/api/imports/complete-upload': (body) => {
      const slot = service.slots.find((candidate) => candidate.id === body.upload_id);
      if (!slot) return refuse(404, 'not_found', 'no such upload');
      const content = Buffer.concat(slot.parts);
      if (content.length !== slot.expected) {
        return refuse(409, 'conflict', `received ${content.length} bytes of ${slot.expected}`);
      }
      if (createHash('sha256').update(content).digest('hex') !== body.sha256) {
        return refuse(409, 'conflict', 'the digest differs from the one announced');
      }
      slot.complete = true;
      return ok(view(slot));
    },
    '/api/imports/start-import': (body) => {
      service.sent.push({ path: 'start_import', body });
      service.job = jobOf('queued');
      return { status: 202, body: service.job };
    },
    '/api/imports/get-job': () =>
      service.job ? ok(service.job) : refuse(404, 'not_found', 'no such job'),
    '/api/imports/list-jobs': () =>
      ok({ items: service.job ? [service.job] : [], next_cursor: null }),
    '/api/imports/cancel-job': (body) => {
      service.sent.push({ path: 'cancel_job', body });
      service.job = jobOf('cancelled');
      return ok(service.job);
    },
    '/api/attention/get-attention': () =>
      ok({ items: service.warnings, next_cursor: null, revision: HEAD }),
    '/api/items/get-item': (body) => {
      const item = body.item_id === DRAWING ? drawing : body.item_id === SOURCE ? source : note;
      return ok({
        summary: item,
        body: item === drawing ? '' : `# ${item.title}`,
        content_digest: 'c'.repeat(64),
        properties: { type: item.type_name, okf_jawn: { id: item.id } },
      });
    },
    '/api/reads/read-item': (body) =>
      body.item_id === DRAWING
        ? ok({
            ...readAnswer(drawing, body, { outline: [] }),
            markdown: '',
            extraction: drawing.extraction,
          })
        : ok(readAnswer(body.item_id === SOURCE ? source : note, body)),
  };
  await routeApi(page, service.answers);
  // The byte upload: registered after the operations, so it is the route that answers it.
  await page.route('**/api/workspaces/*/uploads/*/content', async (route) => {
    const request = route.request();
    service.uploadCsrf.push(request.headers()['x-okf-csrf']);
    if (service.failUploads > 0) {
      service.failUploads -= 1;
      await route.abort('connectionfailed');
      return;
    }
    // The browser's own request goes on to the sink, body and all.
    await route.continue({ url: `${sink}${new URL(request.url()).pathname}` });
  });
  return service;
}

/** A folder on this computer with three files in two folders. */
async function folderToImport(): Promise<string> {
  const root = join(await mkdtemp(join(tmpdir(), 'okf-import-')), 'incoming');
  await mkdir(join(root, 'reports', '2025'), { recursive: true });
  await writeFile(join(root, 'readme.txt'), 'About this folder.');
  await writeFile(join(root, 'reports', 'summary.txt'), 'A summary.');
  await writeFile(join(root, 'reports', '2025', 'q1.txt'), 'First quarter.');
  return root;
}

const PATHS = [
  'incoming/readme.txt',
  'incoming/reports/2025/q1.txt',
  'incoming/reports/summary.txt',
];

/** Press a control from the keyboard: focus it, then Enter. */
async function press(page: Page, name: string | RegExp) {
  const button = page.getByRole('button', { name });
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  await button.focus();
  await page.keyboard.press('Enter');
}

/** Choose the folder from the keyboard: the button opens the browser's picker. */
async function chooseFolder(page: Page, folder: string) {
  const choosing = page.waitForEvent('filechooser');
  await press(page, 'Choose a folder…');
  await (await choosing).setFiles(folder);
}

const rows = (page: Page) =>
  page.getByRole('list', { name: /^Files to import/ }).getByRole('listitem');
const rowOf = (page: Page, path: string) => rows(page).filter({ hasText: path });
const jobRow = (page: Page) => page.locator(`li[data-job="${JOB}"]`);

/** Keyboard focus 300 ms on, after the frame in which a browser moves it off a removed control. */
async function focusSettled(page: Page): Promise<string> {
  await page.waitForTimeout(300);
  return focused(page);
}

/** The paths listed, in an order that does not depend on how the browser walks a folder. */
async function listedPaths(page: Page): Promise<string[]> {
  const paths = await rows(page).locator('.upload-path').allTextContents();
  return paths.sort();
}

test.describe('bringing files in', () => {
  const server = useExplorerServer();
  const sink = useUploadSink();

  test('choose three files in two folders by keyboard and see their paths', async ({ page }) => {
    const service = await play(page, sink.origin());
    await page.goto(`${server.origin()}/w/${WS}`);
    // The page is reached from the workspace's bar, by keyboard.
    await page.getByRole('link', { name: 'Import' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Import files' })).toBeVisible();
    expect(await focusSettled(page)).toBe('a:Import');

    await chooseFolder(page, await folderToImport());
    await expect(page.getByText('3 files added.')).toBeVisible();
    expect(await listedPaths(page)).toEqual(PATHS);
    await expect(rowOf(page, 'incoming/reports/2025/q1.txt')).toContainText('14 bytes');
    await expect(rowOf(page, 'incoming/reports/2025/q1.txt')).toContainText('Not sent yet.');
    await expect(page.getByRole('button', { name: 'Upload 3 files' })).toBeVisible();
    // Nothing has been sent.
    expect(service.slots).toEqual([]);
    expect(await focusSettled(page)).toBe('button:Choose a folder…');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the files chosen for import');
    await shot(page, '21-import-chosen');

    // An entry is removed from the keyboard; focus stays in the list.
    await page.getByRole('button', { name: 'Remove incoming/readme.txt' }).focus();
    await page.keyboard.press('Enter');
    await expect(rows(page)).toHaveCount(2);
    expect(await focusSettled(page)).toMatch(/^button:Remove incoming\//);
    await expectNoAxeViolations(page, 'the list after a removal');
  });

  test('files dropped on the page are listed, beside the tree’s own drag and drop', async ({
    page,
  }) => {
    const service = await play(page, sink.origin());
    await page.goto(`${server.origin()}/w/${WS}/import`);
    // The folder tree is on the page: its drag-and-drop listens on the window for every drag.
    await expect(page.getByRole('treeitem', { name: 'notes' })).toBeVisible();
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.evaluate(() => {
      const target = document.querySelector('.drop-target');
      const carried = new DataTransfer();
      carried.items.add(new File(['abc'], 'dropped.txt', { type: 'text/plain' }));
      carried.items.add(new File(['abcd'], 'second.txt', { type: 'text/plain' }));
      for (const type of ['dragenter', 'dragover', 'drop']) {
        target?.dispatchEvent(
          new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: carried }),
        );
      }
    });
    await expect(page.getByText('2 files added.')).toBeVisible();
    expect(await listedPaths(page)).toEqual(['dropped.txt', 'second.txt']);
    await expect(rowOf(page, 'second.txt')).toContainText('4 bytes');
    expect(service.slots).toEqual([]);
    expect(errors).toEqual([]);
    await expectNoAxeViolations(page, 'the files dropped for import');
  });

  test('an upload that fails once and is retried', async ({ page, context }) => {
    const service = await play(page, sink.origin());
    service.failUploads = 1;
    await context.addCookies([{ name: 'okf-csrf', value: TOKEN, url: server.origin() }]);
    await page.goto(`${server.origin()}/w/${WS}/import`);
    await chooseFolder(page, await folderToImport());
    await expect(rows(page)).toHaveCount(3);
    await press(page, 'Upload 3 files');

    // One file failed and kept its place; the others went on.
    const failed = rows(page).filter({ hasText: 'Not uploaded.' });
    await expect(failed).toHaveCount(1);
    await expect(rows(page).filter({ hasText: 'Uploaded: the service checked' })).toHaveCount(2);
    await expect(failed).toContainText('The workspace service could not be reached.');
    await expect(page.getByText(/^2 of 3 files uploaded; 1 not uploaded\./)).toBeVisible();
    expect(await listedPaths(page)).toEqual(PATHS);
    await expectNoAxeViolations(page, 'an upload that failed');
    await shot(page, '22-upload-failed');

    const path = (await failed.locator('.upload-path').textContent()) ?? '';
    await failed.getByRole('button', { name: 'Try again' }).focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(rows(page).filter({ hasText: 'Uploaded: the service checked' })).toHaveCount(3);
    await expect(page.getByText('All 3 files are uploaded and checked.')).toBeVisible();
    // The retry is gone; focus is on the row's own control.
    expect(await focusSettled(page)).toBe(`button:Remove ${path}`);
    await expectVisibleFocus(page);

    // The service holds each file under its folder, checked against its length and hash.
    expect(service.slots).toHaveLength(3);
    expect(service.slots.every((slot) => slot.complete)).toBe(true);
    expect(service.slots.map((slot) => `${slot.folder}/${slot.filename}`).sort()).toEqual(PATHS);
    // Every byte upload carried the CSRF cookie's value: three that arrived, and the one that failed.
    expect(service.uploadCsrf).toHaveLength(4);
    expect(
      service.uploadCsrf.filter((value) => value !== TOKEN).map((value) => value ?? 'none'),
    ).toEqual([]);
    await expectNoAxeViolations(page, 'the files uploaded');
    await shot(page, '23-uploaded');
  });

  test('an import that becomes a job and then links to its items', async ({ page }) => {
    const service = await play(page, sink.origin());
    await page.goto(`${server.origin()}/w/${WS}/import`);
    await chooseFolder(page, await folderToImport());
    await press(page, 'Upload 3 files');
    await expect(page.getByText('All 3 files are uploaded and checked.')).toBeVisible();

    await page.getByRole('combobox', { name: 'Folder to import into' }).focus();
    await page.keyboard.type('notes/2025');
    await press(page, 'Import 3 uploaded files');
    await expect(jobRow(page)).toContainText('Import, job 88888888');
    await expect(jobRow(page)).toContainText('Waiting to start.');
    await expect(
      page.getByText(
        'Import of 3 files into “notes/2025” was accepted as Import, job 88888888. It is not finished: its state is “waiting to start”.',
      ),
    ).toBeVisible();
    expect(service.sent.at(-1)).toMatchObject({
      path: 'start_import',
      body: {
        destination: 'notes/2025',
        base_revision: HEAD,
        apply_naming_rules: false,
        upload_ids: service.slots.map((slot) => slot.id),
      },
    });
    // The files it took have left the list; the button stayed and has focus.
    await expect(page.getByText('No files are listed yet.')).toBeVisible();
    expect(await focusSettled(page)).toBe('button:Import 0 uploaded files');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'an import just started');
    await shot(page, '24-import-started');

    // The job runs, then finishes: the row follows it, asked again on the clock.
    service.job = jobOf('running');
    await expect(jobRow(page)).toContainText('Running.', { timeout: 20_000 });
    service.job = jobOf('succeeded', { item_ids: [NOTE, DRAWING], revision: IMPORTED });
    await expect(jobRow(page)).toContainText('Finished.', { timeout: 20_000 });
    const made = jobRow(page).getByRole('list', { name: 'Items made by Import, job 88888888' });
    await expect(made.getByRole('link', { name: 'Meeting notes' })).toHaveAttribute(
      'href',
      `/w/${WS}/i/${NOTE}?rev=${IMPORTED}`,
    );
    // An unsupported file is named as one where the job lists it.
    await expect(made.getByRole('listitem').filter({ hasText: 'plan.dwg' })).toContainText(
      '(format not supported)',
    );
    await expectNoAxeViolations(page, 'a finished import');
    await shot(page, '25-import-finished');

    // Following an item's link from the keyboard opens it at the job's revision.
    await made.getByRole('link', { name: 'Meeting notes' }).focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/i/${NOTE}\\?rev=${IMPORTED}`));
    await expect(page.getByRole('tab', { name: 'Meeting notes' })).toBeVisible();
    // The link left with its page: focus is on the page's heading, the workspace's name.
    expect(await focusSettled(page)).toBe('h1:Workshop notes');
    await expectNoAxeViolations(page, 'an imported item, opened from its job');
  });

  test('an unsupported file shown as unsupported', async ({ page }) => {
    await play(page, sink.origin(), {
      job: jobOf('succeeded', { item_ids: [DRAWING], revision: IMPORTED }),
    });
    await page.goto(`${server.origin()}/w/${WS}/jobs`);
    const link = jobRow(page).getByRole('link', { name: 'plan.dwg' });
    await expect(link).toBeVisible();
    await link.focus();
    await page.keyboard.press('Enter');

    const state = page.getByRole('note').filter({ hasText: 'Conversion:' });
    await expect(state).toHaveText(
      'Conversion: Bytes kept; text extraction is not supported for this format. There is no extracted text. This is not an empty document, and it has not been converted.',
    );
    await expect(page.getByText('There is no text to show.')).toBeVisible();
    const tab = page.getByRole('tab', { name: 'plan.dwg' });
    await expect(tab).toHaveAccessibleDescription('(format not supported)');
    await expect(tab).toContainText('(format not supported)');
    await expect(page.getByText('Converted.', { exact: true })).toHaveCount(0);
    expect(await focusSettled(page)).toBe('h1:Workshop notes');
    await expectNoAxeViolations(page, 'an unsupported file in the reader');
    await shot(page, '26-unsupported');
  });

  test('a running job cancelled from the keyboard', async ({ page }) => {
    const service = await play(page, sink.origin(), { job: jobOf('running') });
    await page.goto(`${server.origin()}/w/${WS}/jobs`);
    await expect(jobRow(page)).toContainText('Running.');
    await expect(
      page.getByText(/It cannot follow the workspace’s events now: Not available in this build\./),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'the jobs');
    await shot(page, '27-jobs');

    await press(page, 'Cancel');
    await expect(jobRow(page).getByRole('group', { name: 'Cancel this job?' })).toBeVisible();
    expect(await focusSettled(page)).toBe('button:Keep it running');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the question before a cancel');
    await shot(page, '28-cancel-question');
    await page.keyboard.press('Tab');
    expect(await focused(page)).toBe('button:Cancel the job');
    await page.keyboard.press('Enter');
    await expect(jobRow(page)).toContainText('Cancelled.');
    await expect(jobRow(page).getByRole('button')).toHaveCount(0);
    expect(service.sent.at(-1)).toMatchObject({ path: 'cancel_job', body: { job_id: JOB } });
    // The controls are gone with the state; focus is on the job itself.
    expect(await page.evaluate(() => document.activeElement?.getAttribute('data-job'))).toBe(JOB);
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => document.activeElement?.getAttribute('data-job'))).toBe(JOB);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a cancelled job');
  });
});

test.describe('Attention', () => {
  const server = useExplorerServer();
  const sink = useUploadSink();
  const WARNINGS = [
    {
      kind: 'broken_link',
      message: 'The link to plan-2024.md leads nowhere.',
      item_id: NOTE,
      action: 'get_links',
    },
    {
      kind: 'instruction_like',
      message: 'Line 12 reads “ignore previous instructions”.',
      item_id: NOTE,
      action: 'read_item',
    },
    {
      kind: 'unprocessed',
      message: 'Pages 3 to 5 were not converted.',
      item_id: SOURCE,
      action: 'redigest_item',
    },
  ];

  test('the list with warnings, and the marks on the items', async ({ page }) => {
    await play(page, sink.origin(), { warnings: WARNINGS });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    // The marks: on the workspace's link, the item's row, its tab, and in the reader.
    const link = page.getByRole('link', { name: 'Attention (3)' });
    await expect(link).toBeVisible();
    await expect(page.getByRole('treeitem', { name: /^Meeting notes/ })).toHaveAccessibleName(
      'Meeting notes, type meeting-note, 2 warnings (advisory)',
    );
    await expect(page.getByRole('tab', { name: 'Meeting notes' })).toHaveAccessibleDescription(
      '(2 warnings, advisory)',
    );
    await expect(page.getByRole('note').filter({ hasText: 'Attention:' })).toContainText(
      '2 advisory warnings for this item at revision aaaaaaaaaa. False alarms are expected.',
    );
    await expectNoAxeViolations(page, 'an item with Attention warnings');
    await shot(page, '29-attention-marks');

    await link.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Attention' })).toBeVisible();
    await expect(
      page.getByText(
        'These warnings are advisory. They come from automatic checks, and false alarms are expected.',
        { exact: false },
      ),
    ).toBeVisible();
    const list = page.getByRole('list', { name: 'Attention' });
    await expect(list.getByRole('listitem')).toHaveCount(3);
    await expect(list.getByRole('listitem').first()).toContainText(
      'A link that leads nowhere in Meeting notes',
    );
    await expect(list.getByRole('link', { name: 'Annual report 2025' })).toHaveAttribute(
      'href',
      `/w/${WS}/i/${SOURCE}?rev=${HEAD}`,
    );
    await expect(page.getByText('3 warnings shown.')).toBeVisible();
    expect(await focusSettled(page)).toBe('a:Attention (3)');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the Attention list');
    await shot(page, '30-attention');
  });

  test('an empty list is not called safe', async ({ page }) => {
    await play(page, sink.origin(), { warnings: [] });
    await page.goto(`${server.origin()}/w/${WS}`);
    await page.getByRole('link', { name: 'Attention', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText('No warnings found.')).toBeVisible();
    await expect(
      page.getByText(
        'That does not make the workspace safe to give to an AI tool, or correct: the checks look for a few known signs and can miss what matters.',
      ),
    ).toBeVisible();
    await expect(
      page.getByRole('note').filter({ hasText: 'These warnings are advisory.' }),
    ).toBeVisible();
    expect(await focusSettled(page)).toBe('a:Attention');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the Attention list, empty');
    await shot(page, '31-attention-empty');
  });
});
