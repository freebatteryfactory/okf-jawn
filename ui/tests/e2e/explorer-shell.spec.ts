/**
 * The Explorer shell in a real browser, keyboard first, with the API answered through
 * `page.route` (no server exists yet in this lane). Every main screen must have zero axe
 * violations; screenshots go to `.artifacts/ui/` (untracked) for the owner to judge the look.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { mkdir, readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, type Route, test } from '@playwright/test';

const dist = fileURLToPath(new URL('../../dist', import.meta.url));
const shots = fileURLToPath(new URL('../../../.artifacts/ui', import.meta.url));

const WS = '11111111-1111-4111-8111-111111111111';
const HEAD = 'a'.repeat(40);
const NOTE = '22222222-2222-4222-8222-222222222222';
const PLAN = '33333333-3333-4333-8333-333333333333';
const SOURCE = '44444444-4444-4444-8444-444444444444';
const NOW = '2026-01-01T23:30:00.000Z';

const workspace = {
  id: WS,
  name: 'Workshop notes',
  description: 'Notes and sources from the October workshop',
  head: HEAD,
  created_at: NOW,
  permissions: ['read'],
};
const summary = (id: string, path: string, title: string, type: string, kind = 'note') => ({
  id,
  path,
  title,
  type_name: type,
  kind,
  description: `${title}.`,
  archived: false,
  revision: HEAD,
  status: 'stable',
  ...(kind === 'source'
    ? {
        extraction: { status: 'partial', text_origin: 'converter', corrected: false },
        media_type: 'application/pdf',
      }
    : {}),
});
const note = summary(NOTE, 'notes/meeting.md', 'Meeting notes', 'meeting-note');
const plan = summary(PLAN, 'notes/plan.md', 'Plan', 'plan');
const source = summary(SOURCE, 'annual-report.md', 'Annual report 2025', 'report', 'source');
const byId: Record<string, ReturnType<typeof summary>> = {
  [NOTE]: note,
  [PLAN]: plan,
  [SOURCE]: source,
};
const markdown = [
  '# Meeting notes',
  '',
  'The group agreed to **publish the budget** after review. Revenue grew by 12% in 2025.',
  '',
  '| Quarter | Revenue |',
  '| --- | --- |',
  '| Q1 | 1.2 M |',
  '| Q2 | 1.4 M |',
  '',
  '- [x] Collect the sources',
  '- [ ] Draft the summary',
].join('\n');

type Reply = { status: number; body: unknown };
type Answers = Record<string, (body: Record<string, unknown>) => Reply>;
const ok = (body: unknown): Reply => ({ status: 200, body });

function answers(overrides: Answers = {}): Answers {
  return {
    '/api/workspaces/list-workspaces': () => ok({ items: [workspace], next_cursor: null }),
    '/api/workspaces/open-workspace': () => ok(workspace),
    '/api/items/list-items': (body) =>
      body.folder === 'notes'
        ? ok({ folders: [], items: [note, plan], revision: HEAD })
        : ok({ folders: ['notes'], items: [source], revision: HEAD }),
    '/api/items/get-item': (body) => {
      const item = byId[String(body.item_id)] ?? note;
      return ok({
        summary: item,
        body: markdown,
        content_digest: 'c'.repeat(64),
        properties: {
          type: item.type_name,
          owner: 'Ada Lovelace',
          meeting_date: '2025-10-03',
          okf_jawn: { id: item.id },
        },
      });
    },
    '/api/reads/read-item': (body) => {
      const item = byId[String(body.item_id)] ?? note;
      return ok({
        markdown: item.id === NOTE ? markdown : `# ${item.title}\n\nText of ${item.title}.`,
        media: [],
        outline: [
          {
            kind: 'heading',
            label: item.title,
            level: 1,
            selection: { kind: 'lines', range: { start: 1, end: 11 } },
          },
        ],
        receipt_id: '55555555-5555-4555-8555-555555555555',
        source: {
          workspace_id: WS,
          item_id: item.id,
          path: item.path,
          revision: HEAD,
          selection: body.selection,
          locations: [],
        },
        truncated: false,
        view: 'text',
        warnings: [],
      });
    },
    '/api/reads/get-sources': () =>
      ok({
        revision: HEAD,
        sources: [
          {
            workspace_id: WS,
            item_id: SOURCE,
            path: 'annual-report.md',
            revision: HEAD,
            digest: 'c'.repeat(64),
            selection: { kind: 'pages', range: { start: 2, end: 3 } },
            locations: [
              { provenance: 'direct', locator: { kind: 'page', page_no: 2 } },
              { provenance: 'inferred', locator: { kind: 'page', page_no: 3 } },
            ],
          },
        ],
        declared: [
          {
            entry: { id: 'report', path: 'annual-report.md' },
            outcome: { kind: 'cited', index: 0 },
          },
        ],
      }),
    '/api/search/search-items': () =>
      ok({
        revision: HEAD,
        hits: [
          {
            title: 'Meeting notes',
            snippet: '…agreed to publish the budget after review…',
            score: 1,
            source: {
              workspace_id: WS,
              item_id: NOTE,
              path: 'notes/meeting.md',
              revision: HEAD,
              selection: { kind: 'lines', range: { start: 3, end: 3 } },
              locations: [],
            },
          },
        ],
      }),
    ...overrides,
  };
}

async function routeApi(page: Page, overrides: Answers = {}) {
  const table = answers(overrides);
  await page.route('**/api/**', async (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    const raw = route.request().postData();
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    const body =
      parsed !== null && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {};
    const answer = table[path];
    const reply = answer
      ? answer(body)
      : { status: 501, body: { code: 'not_implemented', message: `${path} is not implemented` } };
    await route.fulfill({
      status: reply.status,
      contentType: 'application/json',
      body: JSON.stringify(reply.body),
    });
  });
}

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.json': 'application/json',
  '.map': 'application/json',
};

async function serveDist() {
  const server = createServer(async (request, response) => {
    const path = decodeURIComponent(new URL(request.url ?? '/', 'http://local').pathname);
    const file = extname(path) ? normalize(join(dist, path)) : join(dist, 'index.html');
    if (!file.startsWith(dist)) {
      response.writeHead(403).end();
      return;
    }
    try {
      const body = await readFile(file);
      response.writeHead(200, {
        'content-type': TYPES[extname(file)] ?? 'application/octet-stream',
      });
      response.end(body);
    } catch {
      response.writeHead(404).end('not found');
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('server address unavailable');
  return { server, origin: `http://127.0.0.1:${address.port}` };
}

async function expectNoAxeViolations(page: Page, screen: string) {
  const result = await new AxeBuilder({ page }).analyze();
  const found = result.violations.map((violation) => ({
    id: violation.id,
    impact: violation.impact,
    nodes: violation.nodes.map((node) => node.target.join(' ')),
  }));
  expect(found, `${screen}: ${JSON.stringify(found, null, 2)}`).toEqual([]);
}

async function shot(page: Page, name: string) {
  await mkdir(shots, { recursive: true });
  await page.screenshot({ path: join(shots, `${name}.png`), fullPage: true });
}

/** The focused element draws a visible outline. */
async function expectVisibleFocus(page: Page) {
  const outline = await page.evaluate(() => {
    const element = document.activeElement;
    if (!element || element === document.body) return 'nothing focused';
    const style = getComputedStyle(element);
    return `${style.outlineStyle} ${style.outlineWidth}`;
  });
  expect(outline).not.toBe('nothing focused');
  expect(outline.startsWith('none')).toBe(false);
}

test.describe('Explorer shell', () => {
  let origin = '';
  let close: () => void = () => {};
  test.beforeAll(async () => {
    const served = await serveDist();
    origin = served.origin;
    close = () => served.server.close();
  });
  test.afterAll(() => close());
  test.use({ viewport: { width: 1440, height: 900 } });

  test('lists workspaces and opens one', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${origin}/`);
    await expect(page.getByRole('link', { name: 'Workshop notes' })).toBeVisible();
    await expectNoAxeViolations(page, 'workspace list');
    await shot(page, '01-workspaces');
    await page.getByRole('link', { name: 'Workshop notes' }).focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { level: 1, name: 'Workshop notes' })).toBeVisible();
  });

  test('reads an item opened from the tree with the keyboard alone', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${origin}/w/${WS}`);
    const tree = page.getByRole('tree', { name: 'Folders' });
    await expect(tree.getByRole('treeitem', { name: 'notes', exact: true })).toBeVisible();
    await expectNoAxeViolations(page, 'workspace');
    await shot(page, '02-workspace-tree');

    await page.getByRole('searchbox', { name: 'Search this workspace' }).focus();
    await page.keyboard.press('Tab'); // the Search button
    await page.keyboard.press('Tab'); // the tree, which focuses its first entry
    await expect(tree.getByRole('treeitem', { name: 'notes', exact: true })).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('ArrowRight');
    await expect(tree.getByRole('treeitem', { name: /Meeting notes/ })).toBeVisible();
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/w/${WS}/i/${NOTE}$`));
    const article = page.getByRole('article', { name: 'Meeting notes' });
    await expect(article.getByText('publish the budget')).toBeVisible();
    await expect(page.getByRole('tab', { name: 'Meeting notes', selected: true })).toBeVisible();
    const sources = page.getByRole('region', { name: 'Source panel' });
    await expect(sources.getByText('Inferred location, not certain:')).toBeVisible();
    await expectNoAxeViolations(page, 'document');
    await shot(page, '03-document-and-sources');
  });

  test('searches, opens a result at its selection, and splits into panes', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${origin}/w/${WS}`);
    const search = page.getByRole('searchbox', { name: 'Search this workspace' });
    await search.fill('budget');
    await search.press('Enter');
    const results = page.getByRole('region', { name: 'Results for “budget”' });
    await expect(results.getByRole('link', { name: /Meeting notes/ })).toBeVisible();
    await expectNoAxeViolations(page, 'search results');
    await shot(page, '04-search-results');

    await results.getByRole('link', { name: /Meeting notes/ }).focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page.getByText('Showing line 3.')).toBeVisible();

    await page.getByRole('button', { name: 'Back to folders' }).click();
    const tree = page.getByRole('tree', { name: 'Folders' });
    // Opening the result revealed it, so its folder is already expanded in the tree.
    await expect(tree.getByRole('treeitem', { name: 'notes', exact: true })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    await tree.getByRole('treeitem', { name: /Plan/ }).click();
    await expect(page.getByRole('tab', { name: 'Plan', selected: true })).toBeVisible();
    await page.getByRole('button', { name: 'Split right' }).click();
    await expect(page.getByRole('region', { name: 'Documents, pane 2 of 2' })).toBeVisible();
    const separator = page.getByRole('separator', { name: 'Resize panes 1 and 2' });
    // After the pointer click above, a key press returns the page to keyboard modality, as a
    // keyboard user moving to the separator would.
    await page.keyboard.press('Shift');
    await separator.focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('ArrowLeft');
    await page.getByRole('tab', { name: 'Plan' }).first().focus();
    await page.keyboard.press('ArrowLeft');
    await expect(page.getByRole('tab', { name: 'Meeting notes' })).toBeFocused();
    await expectNoAxeViolations(page, 'split panes');
    await shot(page, '05-split-panes');
  });

  test('shows an unbuilt operation as not available', async ({ page }) => {
    await routeApi(page, {
      '/api/reads/read-item': () => ({
        status: 501,
        body: { code: 'not_implemented', message: 'read_item is not implemented yet' },
      }),
      '/api/reads/get-sources': () => ({
        status: 501,
        body: { code: 'not_implemented', message: 'get_sources is not implemented yet' },
      }),
    });
    await page.goto(`${origin}/w/${WS}/i/${NOTE}`);
    await expect(
      page
        .getByRole('alert')
        .filter({ hasText: 'Reading this item: Not available in this build.' }),
    ).toBeVisible();
    await expect(
      page
        .getByRole('alert')
        .filter({ hasText: 'Listing this item’s sources: Not available in this build.' }),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'not available');
    await shot(page, '06-not-available');
  });
});
