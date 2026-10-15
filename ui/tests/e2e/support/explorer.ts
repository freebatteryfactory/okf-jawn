/**
 * Shared ground for the Explorer browser specs: one small contract-shaped workspace answered
 * through `page.route` (no server exists in this lane), a static server for the built `dist/`,
 * and the checks every screen runs: zero axe violations, visible focus, a screenshot for the
 * owner under `.artifacts/ui/` (untracked).
 *
 * The specs need `bun --bun run build` first: they serve the built `dist/` themselves.
 */

import { mkdir, readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, type Route, test } from '@playwright/test';

const dist = fileURLToPath(new URL('../../../dist', import.meta.url));
const shots = fileURLToPath(new URL('../../../../.artifacts/ui', import.meta.url));

export const WS = '11111111-1111-4111-8111-111111111111';
export const HEAD = 'a'.repeat(40);
export const NOTE = '22222222-2222-4222-8222-222222222222';
export const PLAN = '33333333-3333-4333-8333-333333333333';
export const SOURCE = '44444444-4444-4444-8444-444444444444';
export const NOW = '2026-01-01T23:30:00.000Z';

export const workspace = {
  id: WS,
  name: 'Workshop notes',
  description: 'Notes and sources from the October workshop',
  head: HEAD,
  created_at: NOW,
  permissions: ['read'],
};
/** The same workspace for a person who may write in it. */
export const writableWorkspace = { ...workspace, permissions: ['read', 'write'] };
export const summary = (id: string, path: string, title: string, type: string, kind = 'note') => ({
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
export const note = summary(NOTE, 'notes/meeting.md', 'Meeting notes', 'meeting-note');
export const plan = summary(PLAN, 'notes/plan.md', 'Plan', 'plan');
export const source = summary(SOURCE, 'annual-report.md', 'Annual report 2025', 'report', 'source');
const byId: Record<string, ReturnType<typeof summary>> = {
  [NOTE]: note,
  [PLAN]: plan,
  [SOURCE]: source,
};
export const markdown = [
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

export type Reply = { status: number; body: unknown };
export type Answers = Record<string, (body: Record<string, unknown>) => Reply | Promise<Reply>>;
export const ok = (body: unknown): Reply => ({ status: 200, body });

export function readAnswer(
  item: ReturnType<typeof summary>,
  body: Record<string, unknown>,
  extra: Record<string, unknown> = {},
) {
  return {
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
    ...extra,
  };
}

export function answers(overrides: Answers = {}): Answers {
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
    '/api/reads/read-item': (body) => ok(readAnswer(byId[String(body.item_id)] ?? note, body)),
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
    // The person has no drafts, and the workspace defines no types, unless a spec says so.
    '/api/items/list-drafts': () => ok({ revision: HEAD, items: [], next_cursor: null }),
    '/api/types/list-types': () => ok({ items: [] }),
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

/** Answer `/api/**` from the table; an operation without an answer is `not_implemented`. */
export async function routeApi(page: Page, overrides: Answers = {}) {
  const table = answers(overrides);
  await page.route('**/api/**', async (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    const raw = route.request().postData();
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    const body =
      parsed !== null && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {};
    const answer = table[path];
    const reply = answer
      ? await answer(body)
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

/**
 * Serve `dist/` for the specs of the calling describe block, at the default 1440 x 900 viewport,
 * with room for a cold browser start on a shared machine. Returns the origin, read inside tests.
 */
export function useExplorerServer(): { origin: () => string } {
  let origin = '';
  let close: () => void = () => {};
  test.describe.configure({ timeout: 90_000 });
  test.use({ viewport: { width: 1440, height: 900 } });
  test.beforeAll(async () => {
    const served = await serveDist();
    origin = served.origin;
    close = () => served.server.close();
  });
  test.afterAll(() => close());
  return { origin: () => origin };
}

export async function expectNoAxeViolations(page: Page, screen: string) {
  const result = await new AxeBuilder({ page }).analyze();
  const found = result.violations.map((violation) => ({
    id: violation.id,
    impact: violation.impact,
    nodes: violation.nodes.map((node) => node.target.join(' ')),
  }));
  expect(found, `${screen}: ${JSON.stringify(found, null, 2)}`).toEqual([]);
}

/** Save a full-page screenshot named for the screen and the browser that took it. */
export async function shot(page: Page, name: string) {
  await mkdir(shots, { recursive: true });
  const browser = page.context().browser()?.browserType().name() ?? 'browser';
  await page.screenshot({ path: join(shots, `${name}-${browser}.png`), fullPage: true });
}

/** The focused element draws a visible outline. */
export async function expectVisibleFocus(page: Page) {
  const outline = await page.evaluate(() => {
    const element = document.activeElement;
    if (!element || element === document.body) return 'nothing focused';
    const style = getComputedStyle(element);
    return `${style.outlineStyle} ${style.outlineWidth}`;
  });
  expect(outline).not.toBe('nothing focused');
  expect(outline.startsWith('none')).toBe(false);
}

/** The focused element: its role and accessible-ish name, or "body". */
export async function focused(page: Page): Promise<string> {
  return page.evaluate(() => {
    const element = document.activeElement;
    if (!element || element === document.body) return 'body';
    const role = element.getAttribute('role') ?? element.tagName.toLowerCase();
    const name =
      element.getAttribute('aria-label') ??
      (element.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
    return `${role}:${name}`;
  });
}
