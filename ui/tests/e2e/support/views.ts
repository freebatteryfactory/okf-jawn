/**
 * Ground for the Views browser specs (M4b B1 and B2): one small contract-valid saved View, the
 * source it cites at a pinned revision, and the retained dataset behind its table and chart,
 * answered through `page.route` beside the Explorer's own answers (`explorer.ts`); a proposal that
 * would create that View; and the keyboard-only helpers the passes share.
 *
 * The View's file is stored as the contract stores one: its title in the `title` property, its
 * description as Markdown, then the one ```` ```json okf-jawn-view ```` block that ends the body
 * and names no workspace. The dataset's SHA-256 is computed here, once, and the bytes it names are
 * what `get_object` serves; the views side checks them in the browser.
 */

import { createHash } from 'node:crypto';
import { expect, type Locator, type Page } from '@playwright/test';
import { type Answers, answers, HEAD, NOW, ok, summary, WS } from './explorer';

export const VIEW = '66666666-6666-4666-8666-666666666666';
export const FIGURES = '77777777-7777-4777-8777-777777777777';
/** The revision the View's binding pins: older than the head the workspace shows. */
export const CITED_AT = 'f00d'.repeat(10);
export const FIGURES_PATH = 'data/revenue.csv.md';
export const VIEW_PATH = 'views/revenue.md';
export const RECEIPT = '55555555-5555-4555-8555-555555555555';
export const PROPOSAL_ID = '99999999-1111-4111-8111-111111111111';
export const CANDIDATE = '2'.repeat(40);
export const ACCEPTED = '4'.repeat(40);
const CONFIRMATION_ID = '66666666-6666-4666-8666-666666666667';

export const viewItem = summary(VIEW, VIEW_PATH, 'Revenue by quarter', 'chart-view', 'view');
export const figuresItem = {
  ...summary(FIGURES, FIGURES_PATH, 'Revenue figures', 'figures', 'source'),
  extraction: { status: 'completed', text_origin: 'converter', corrected: false },
  media_type: 'text/csv',
  revision: CITED_AT,
};

const citation = {
  workspace_id: WS,
  item_id: FIGURES,
  path: FIGURES_PATH,
  revision: CITED_AT,
  selection: { kind: 'all' },
};

/**
 * The View's document as the wire carries it: its binding names its workspace. It draws a table,
 * a chart and the list of its sources. Limit: it holds no `SourceExcerpt`. A source's text that
 * opens with a level-one heading (as a converted document does) puts an `h1` before the Chart's
 * `h3` inside the View, and axe reports heading-order there; the fix belongs to the views lane
 * (requested with M4b), and this spec does not hide it under a source without a heading.
 */
export function viewOf(title = 'Revenue by quarter') {
  return {
    schema_version: 1,
    title,
    description: 'Revenue grew in **2025**: each quarter above the one before.',
    mode: 'pinned',
    grammar: 'json_render',
    bindings: [
      { name: 'revenue', source: citation, units: { revenue: 'EUR million' }, transforms: [] },
    ],
    spec: {
      root: 'root',
      elements: {
        root: {
          type: 'Stack',
          props: { title: 'Revenue by quarter, 2025' },
          children: ['table', 'chart', 'sources'],
        },
        table: { type: 'DataTable', props: { binding: 'revenue' }, children: [] },
        chart: {
          type: 'Chart',
          props: { binding: 'revenue', chart: 'revenue_chart', title: 'Revenue per quarter' },
          children: [],
        },
        sources: { type: 'SourceList', props: { bindings: ['revenue'] }, children: [] },
      },
    },
    charts: {
      revenue_chart: {
        data: { name: 'revenue' },
        mark: 'bar',
        encoding: {
          x: { field: 'quarter', type: 'nominal' },
          y: { field: 'revenue', type: 'quantitative', title: 'Revenue (EUR million)' },
        },
      },
    },
  };
}

/** The file of the View: its title property, and its description and block as the body. */
export function viewFileOf(view = viewOf()) {
  const block = {
    schema_version: view.schema_version,
    mode: view.mode,
    grammar: view.grammar,
    bindings: view.bindings.map(({ name, source, units, transforms }) => {
      const { workspace_id: _workspace, ...stored } = source;
      return { name, source: stored, units, transforms };
    }),
    spec: view.spec,
    charts: view.charts,
  };
  return {
    properties: { type: 'chart-view', title: view.title },
    body: `${view.description}\n\n\`\`\`json okf-jawn-view\n${JSON.stringify(block, null, 2)}\n\`\`\`\n`,
  };
}

/** The converter's typed table of the cited source, as the View retains it. */
const dataset = {
  schema_version: 1,
  source: citation,
  text_origin: 'converter',
  columns: [
    { name: 'quarter', kind: 'string' },
    { name: 'revenue', kind: 'number' },
  ],
  rows: [
    ['Q1', 1.2],
    ['Q2', 1.4],
    ['Q3', 1.9],
    ['Q4', 2.3],
  ],
};
const datasetBytes = Buffer.from(JSON.stringify(dataset));
export const DATASET_SHA = createHash('sha256').update(datasetBytes).digest('hex');

/** A `present_view` or `resolve_view` answer: each binding as cited, its dataset retained. */
export function presentationOf(view = viewOf()) {
  return {
    as_of: NOW,
    charts: [
      {
        chart: { kind: 'named', name: 'revenue_chart' },
        status: { status: 'ready', bindings: ['revenue'] },
      },
    ],
    receipt_id: RECEIPT,
    resolved_bindings: view.bindings.map((binding) => ({
      ...binding,
      source: { ...binding.source, locations: [] },
      materialized: DATASET_SHA,
    })),
    view,
    warnings: [],
  };
}

function revisionOf(body: Record<string, unknown>): string {
  const at = body.at as { revision?: unknown } | undefined;
  return typeof at?.revision === 'string' ? at.revision : HEAD;
}

/** The text of the cited source, as `read_item` answers it at the revision asked. */
function figuresRead(body: Record<string, unknown>) {
  return {
    markdown: '# Revenue figures\n\nQuarterly revenue as reported, in EUR million.',
    media: [],
    outline: [],
    receipt_id: RECEIPT,
    source: { ...citation, revision: revisionOf(body), selection: body.selection, locations: [] },
    truncated: false,
    view: 'text',
    warnings: [],
  };
}

/**
 * The Explorer's answers with the View, its source and the View operations; `overrides` answers
 * whatever a spec studies.
 */
export function viewAnswers(overrides: Answers = {}): Answers {
  const base = answers();
  const file = viewFileOf();
  const item = base['/api/items/get-item'];
  const read = base['/api/reads/read-item'];
  const listed = base['/api/items/list-items'];
  const sources = base['/api/reads/get-sources'];
  return {
    '/api/items/list-items': (body) => {
      if (body.folder === 'views')
        return ok({ folders: [], items: [viewItem], revision: HEAD, next_cursor: null });
      if (body.folder === 'data')
        return ok({ folders: [], items: [figuresItem], revision: HEAD, next_cursor: null });
      if (body.folder === undefined || body.folder === null || body.folder === '')
        return ok({
          folders: ['data', 'notes', 'views'],
          items: [],
          revision: HEAD,
          next_cursor: null,
        });
      return listed ? listed(body) : ok({ folders: [], items: [], revision: HEAD });
    },
    '/api/items/get-item': (body) => {
      const revision = revisionOf(body);
      if (body.item_id === VIEW)
        return ok({
          summary: { ...viewItem, revision },
          body: file.body,
          content_digest: 'c'.repeat(64),
          properties: { ...file.properties, okf_jawn: { id: VIEW } },
        });
      if (body.item_id === FIGURES)
        return ok({
          summary: { ...figuresItem, revision },
          body: '# Revenue figures',
          content_digest: 'c'.repeat(64),
          properties: { type: 'figures', okf_jawn: { id: FIGURES } },
        });
      return item ? item(body) : ok({});
    },
    '/api/reads/read-item': (body) => {
      if (body.item_id === FIGURES) return ok(figuresRead(body));
      if (body.item_id === VIEW)
        return ok({
          ...figuresRead(body),
          markdown: file.body,
          source: {
            workspace_id: WS,
            item_id: VIEW,
            path: VIEW_PATH,
            revision: revisionOf(body),
            selection: body.selection,
            locations: [],
          },
        });
      return read ? read(body) : ok({});
    },
    '/api/reads/get-sources': (body) =>
      body.item_id === VIEW
        ? ok({
            revision: revisionOf(body),
            sources: [{ ...citation, locations: [] }],
            declared: [],
          })
        : sources
          ? sources(body)
          : ok({ revision: HEAD, sources: [], declared: [] }),
    '/api/views/get-view': () =>
      ok({
        ...viewOf(),
        bindings: viewOf().bindings.map((binding) => ({
          ...binding,
          source: { ...binding.source, locations: [] },
        })),
      }),
    '/api/views/resolve-view': () => ok(presentationOf()),
    // The candidate as sent, its one binding resolved as cited.
    '/api/views/present-view': (body) => ok({ ...presentationOf(), view: body.view }),
    '/api/reads/get-object': (body) =>
      ok({
        data_base64: datasetBytes.toString('base64'),
        has_more: false,
        media_type: 'application/json',
        offset: String(body.offset ?? '0'),
        sha256: String(body.object),
        total_size: String(datasetBytes.byteLength),
      }),
    ...overrides,
  };
}

/** A proposal that would create the View, in the state `status` says. */
export function proposalOf(status: string, change = createChange()) {
  return {
    id: PROPOSAL_ID,
    workspace_id: WS,
    base_revision: HEAD,
    proposal_revision: CANDIDATE,
    content_digest: '3'.repeat(64),
    title: 'Add the revenue View',
    description: 'A chart of the quarterly revenue, from the figures as converted.',
    changes: [change],
    kind: 'content',
    status,
    created_by: 'claude-agent',
    created_via: 'mcp_delegation',
    created_at: '2026-01-01T09:30:00.000Z',
    warnings: [],
  };
}

/** The change that creates the View of `file`. */
export function createChange(file = viewFileOf()) {
  return {
    kind: 'create',
    item_kind: 'view',
    path: VIEW_PATH,
    type_name: 'chart-view',
    properties: file.properties,
    body: file.body,
  };
}

/** What the proposal changes: the View's file, added. */
function addedPatch(body: string) {
  const lines = body.replace(/\n$/, '').split('\n');
  return [
    `diff --git a/${VIEW_PATH} b/${VIEW_PATH}`,
    'new file mode 100644',
    '--- /dev/null',
    `+++ b/${VIEW_PATH}`,
    `@@ -0,0 +1,${lines.length} @@`,
    ...lines.map((line) => `+${line}`),
    '',
  ].join('\n');
}

/**
 * The answers of a proposal that creates the View (of `change`, the View of `viewOf()` unless
 * given) for a person who may approve it; Approve accepts it.
 */
export function proposalAnswers(change = createChange()): Answers {
  const state = { status: 'open' };
  return viewAnswers({
    '/api/workspaces/open-workspace': () =>
      ok({
        id: WS,
        name: 'Workshop notes',
        description: 'Notes and sources from the October workshop',
        head: state.status === 'accepted' ? ACCEPTED : HEAD,
        created_at: NOW,
        permissions: ['read', 'write', 'propose', 'approve', 'review'],
      }),
    '/api/proposals/list-proposals': () =>
      ok({ items: [proposalOf(state.status, change)], next_cursor: null }),
    '/api/proposals/get-proposal': () => ok(proposalOf(state.status, change)),
    '/api/proposals/list-comments': () => ok({ items: [], next_cursor: null }),
    '/api/history/diff-items': (body) =>
      ok({
        from: body.from,
        to: body.to,
        changes: [
          {
            new_path: VIEW_PATH,
            kind: 'added',
            binary: false,
            patch: addedPatch(String(change.body)),
          },
        ],
        warnings: [],
      }),
    '/api/reviews/create-confirmation': (body) =>
      ok({ id: CONFIRMATION_ID, revision: body.revision, expires_at: '2026-01-01T10:25:00.000Z' }),
    '/api/proposals/accept-proposal': () => {
      state.status = 'accepted';
      return ok({ revision: ACCEPTED, receipt_id: RECEIPT, warnings: [] });
    },
  });
}

/** Whether `target` has keyboard focus. */
async function hasFocus(target: Locator): Promise<boolean> {
  return target.evaluate((element) => element === document.activeElement);
}

/**
 * Press Tab (or `key`) until `target` has keyboard focus, at most `limit` times; the test fails if
 * it never does.
 */
export async function tabTo(page: Page, target: Locator, limit = 150, key = 'Tab') {
  for (let presses = 0; presses < limit; presses += 1) {
    if (await hasFocus(target)) return;
    await page.keyboard.press(key);
  }
  await expect(target).toBeFocused();
}
