/**
 * Contract-valid answers for Views in the Explorer, over the workspace of `workspace.tsx`: one
 * saved View item, the source it cites at a pinned revision, the retained dataset behind its
 * table, a proposal's change that creates the View, and the View operations (`get_view`,
 * `resolve_view`, `present_view`) and the object read (`get_object`) added to the mock's contract
 * map.
 *
 * The View's file is written the way the contract stores one (`ItemKind::View`): its title in the
 * `title` property, its description as Markdown, then the one ```` ```json okf-jawn-view ```` block
 * that ends the body and names no workspace. Its dataset's SHA-256 is computed here, once, by the
 * test (`crypto.subtle`), and served as the bytes the digest names; the views side checks it.
 */
import type {
  Change,
  FileChange,
  ItemDocument,
  ItemSummary,
  PresentResponse,
  ReadItemResponse,
  ViewDocument,
} from '../../../src/api/generated/types.gen';
import {
  zGetObjectRequest,
  zGetObjectResponse,
  zGetViewRequest,
  zPresentRequest,
  zPresentResponse,
  zResolveViewRequest,
  zViewDocument,
} from '../../../src/api/generated/zod.gen';
import { CONTRACT, type Handler, type Json, ok } from './api';
import { HEAD, itemDocument, NOW, RECEIPT, readResponse, summary, WS } from './workspace';

export const GET_VIEW = '/api/views/get-view';
export const RESOLVE_VIEW = '/api/views/resolve-view';
export const PRESENT_VIEW = '/api/views/present-view';
export const GET_OBJECT = '/api/reads/get-object';
export const READ = '/api/reads/read-item';
export const ITEM = '/api/items/get-item';

Object.assign(CONTRACT, {
  [GET_VIEW]: { request: zGetViewRequest, response: zViewDocument },
  [RESOLVE_VIEW]: { request: zResolveViewRequest, response: zPresentResponse },
  [PRESENT_VIEW]: { request: zPresentRequest, response: zPresentResponse },
  [GET_OBJECT]: { request: zGetObjectRequest, response: zGetObjectResponse },
});

/** The saved View item, and the source item its one binding cites. */
export const VIEW = '66666666-6666-4666-8666-666666666666';
export const FIGURES = '77777777-7777-4777-8777-777777777777';
/** The revision the binding pins: an older one than the head the workspace shows. */
export const CITED_AT = '9'.repeat(40);
export const FIGURES_PATH = 'data/revenue.csv.md';

export const viewSummary: ItemSummary = summary(
  VIEW,
  'views/revenue.md',
  'Revenue by quarter',
  'chart-view',
  'view',
);
export const figuresSummary: ItemSummary = {
  ...summary(FIGURES, FIGURES_PATH, 'Revenue figures', 'figures', 'source'),
  revision: CITED_AT,
};

/** The View's document as the wire carries it: each binding names its workspace. */
export function viewOf(extra: Partial<ViewDocument> = {}): ViewDocument {
  return {
    schema_version: 1,
    title: 'Revenue by quarter',
    description: 'Revenue grew in **2025**, as the figures show.',
    mode: 'pinned',
    grammar: 'json_render',
    bindings: [
      {
        name: 'revenue',
        source: {
          workspace_id: WS,
          item_id: FIGURES,
          path: FIGURES_PATH,
          revision: CITED_AT,
          selection: { kind: 'all' },
        },
        units: { revenue: 'EUR million' },
        transforms: [],
      },
    ],
    spec: {
      root: 'root',
      elements: {
        root: { type: 'Stack', props: { title: 'Revenue' }, children: ['table', 'sources'] },
        table: { type: 'DataTable', props: { binding: 'revenue' }, children: [] },
        sources: { type: 'SourceList', props: { bindings: ['revenue'] }, children: [] },
      },
    },
    charts: {},
    ...extra,
  };
}

/** The file of a View item: its title property, and its description and block as the body. */
export function viewFileOf(view: ViewDocument = viewOf(), itemId: string = VIEW) {
  const block = {
    schema_version: view.schema_version,
    mode: view.mode,
    grammar: view.grammar,
    bindings: view.bindings.map(({ name, source, units, transforms }) => {
      const { workspace_id: _workspace, locations: _locations, ...stored } = source;
      return { name, source: stored, units, transforms };
    }),
    spec: view.spec,
    charts: view.charts ?? {},
  };
  return {
    properties: { type: 'chart-view', title: view.title, okf_jawn: { id: itemId } },
    body: `${view.description}\n\n\`\`\`json okf-jawn-view\n${JSON.stringify(block, null, 2)}\n\`\`\`\n`,
  };
}

/** Where a proposal would create the View. */
export const PROPOSED_PATH = 'views/revenue.md';

/** A proposal's change that creates a View of `file` (by default the file of `viewOf()`). */
export function createViewChange(
  file: { properties: Record<string, unknown>; body: string } = viewFileOf(),
): Change {
  // A new item has no server-owned header yet: the service writes it.
  const { okf_jawn: _header, ...properties } = file.properties;
  return {
    kind: 'create',
    item_kind: 'view',
    path: PROPOSED_PATH,
    type_name: 'chart-view',
    properties,
    body: file.body,
  };
}

/** A comparison that adds the View's file, as `diff_items` answers it for a create. */
export function addedViewChange(body: string): FileChange {
  const lines = body.split('\n').filter((line, index, all) => index < all.length - 1 || line);
  return {
    new_path: PROPOSED_PATH,
    kind: 'added',
    binary: false,
    patch: [
      `diff --git a/${PROPOSED_PATH} b/${PROPOSED_PATH}`,
      'new file mode 100644',
      '--- /dev/null',
      `+++ b/${PROPOSED_PATH}`,
      `@@ -0,0 +1,${lines.length} @@`,
      ...lines.map((line) => `+${line}`),
      '',
    ].join('\n'),
  };
}

/** `get_item` of the View item, at the revision asked. */
export function viewItemDocument(revision: string = HEAD): ItemDocument {
  const file = viewFileOf();
  return {
    ...itemDocument({ ...viewSummary, revision }),
    properties: file.properties,
    body: file.body,
  };
}

/** The dataset behind the binding: the converter's typed table of the cited source. */
export const dataset = {
  schema_version: 1,
  source: {
    workspace_id: WS,
    item_id: FIGURES,
    path: FIGURES_PATH,
    revision: CITED_AT,
    selection: { kind: 'all' },
  },
  text_origin: 'converter',
  columns: [
    { name: 'quarter', kind: 'string' },
    { name: 'revenue', kind: 'number' },
  ],
  rows: [
    ['Q1', 1.2],
    ['Q2', 1.4],
    ['Q3', 1.9],
  ],
};

const datasetBytes = new TextEncoder().encode(JSON.stringify(dataset));
/** The dataset's SHA-256, as the binding's `materialized` names it. */
export const DATASET_SHA = Array.from(
  new Uint8Array(await crypto.subtle.digest('SHA-256', datasetBytes)),
  (byte) => byte.toString(16).padStart(2, '0'),
).join('');

/** A `present_view` or `resolve_view` answer for `view`: each binding as cited, its dataset kept. */
export function presentationOf(view: ViewDocument = viewOf()): PresentResponse {
  return {
    as_of: NOW,
    charts: [],
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

/** `get_view`'s answer: the saved View with each binding's locations filled. */
export function savedViewOf(view: ViewDocument = viewOf()): ViewDocument {
  return {
    ...view,
    bindings: view.bindings.map((binding) => ({
      ...binding,
      source: { ...binding.source, locations: [] },
    })),
  };
}

/** `get_object`'s answer for a block of the dataset, as the bytes its digest names. */
export function objectBlock(request: Json): Json {
  let binary = '';
  for (const byte of datasetBytes) binary += String.fromCharCode(byte);
  return {
    data_base64: btoa(binary),
    has_more: false,
    media_type: 'application/json',
    offset: String(request.offset ?? '0'),
    sha256: String(request.object),
    total_size: String(datasetBytes.byteLength),
  };
}

/** `read_item` of the cited source at the revision asked. */
export function figuresRead(request: Json): ReadItemResponse {
  const at = request.at as { revision?: unknown } | undefined;
  const revision = typeof at?.revision === 'string' ? at.revision : CITED_AT;
  return readResponse(
    { ...figuresSummary, revision },
    '# Revenue figures\n\nQuarterly revenue as reported.',
  );
}

/**
 * The View operations and reads over the View item and its source; `others` answers every other
 * item's `get_item` and `read_item` (the workspace's own handlers).
 */
export function viewHandlers(others: Record<string, Handler>): Record<string, Handler> {
  const otherItem = others[ITEM];
  const otherRead = others[READ];
  return {
    [ITEM]: (body) => {
      const at = body.at as { revision?: unknown } | undefined;
      const revision = typeof at?.revision === 'string' ? at.revision : HEAD;
      if (body.item_id === VIEW) return ok(viewItemDocument(revision));
      if (body.item_id === FIGURES) return ok(itemDocument({ ...figuresSummary, revision }));
      return otherItem === undefined ? ok(itemDocument(viewSummary)) : otherItem(body);
    },
    [READ]: (body) => {
      if (body.item_id === FIGURES) return ok(figuresRead(body));
      if (body.item_id === VIEW) {
        const at = body.at as { revision?: unknown } | undefined;
        const revision = typeof at?.revision === 'string' ? at.revision : HEAD;
        return ok(readResponse({ ...viewSummary, revision }, viewFileOf().body));
      }
      return otherRead === undefined ? ok(figuresRead(body)) : otherRead(body);
    },
    [GET_VIEW]: () => ok(savedViewOf()),
    [RESOLVE_VIEW]: () => ok(presentationOf()),
    [GET_OBJECT]: (body) => ok(objectBlock(body)),
  };
}
