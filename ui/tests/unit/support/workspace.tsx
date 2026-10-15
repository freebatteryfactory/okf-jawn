/** Contract-valid answers for one small workspace, and a renderer of the whole app at a URL. */
import { createMemoryHistory } from '@tanstack/react-router';
import { render } from '@testing-library/react';
import type {
  Draft,
  GetSourcesResponse,
  ItemDocument,
  ItemSummary,
  ListItemsResponse,
  ReadItemResponse,
  Selection,
  Workspace,
} from '../../../src/api/generated/types.gen';
import { createQueryClient } from '../../../src/app/query-client';
import { App, createAppRouter } from '../../../src/app/router';
import { resetDraftSessions } from '../../../src/features/drafts/saving';
import { emptyView, useExplorerStore } from '../../../src/features/explorer/store';
import { draftsPage, type Handler, type Json, knowItems, ok } from './api';

export const WS = '11111111-1111-4111-8111-111111111111';
export const HEAD = 'a'.repeat(40);
export const OLD = 'b'.repeat(40);
export const NOTE = '22222222-2222-4222-8222-222222222222';
export const OTHER = '33333333-3333-4333-8333-333333333333';
export const SOURCE = '44444444-4444-4444-8444-444444444444';
export const RECEIPT = '55555555-5555-4555-8555-555555555555';
export const DIGEST = 'c'.repeat(64);
export const OBJECT = 'd'.repeat(64);
export const NOW = '2026-01-01T23:30:00.000Z';

export const workspace: Workspace = {
  id: WS,
  name: 'Workshop notes',
  description: 'Notes from the workshop',
  head: HEAD,
  created_at: NOW,
  permissions: ['read'],
};

/** The same workspace for a person who may write in it. */
export const writableWorkspace: Workspace = { ...workspace, permissions: ['read', 'write'] };

/** The draft the service holds after a save: what `save_draft` and `list_drafts` answer. */
export function draftOf(itemId: string, base: string = HEAD, savedAt: string = NOW): Draft {
  return {
    item_id: itemId,
    base_revision: base,
    content_digest: DIGEST,
    editor: 'ada',
    saved_at: savedAt,
  };
}

export function summary(
  id: string,
  path: string,
  title: string,
  typeName: string,
  kind: ItemSummary['kind'] = 'note',
): ItemSummary {
  return {
    id,
    path,
    title,
    type_name: typeName,
    kind,
    description: `${title} description`,
    archived: false,
    revision: HEAD,
    status: 'stable',
    ...(kind === 'source'
      ? {
          extraction: { status: 'partial', text_origin: 'converter', corrected: false },
          media_type: 'application/pdf',
        }
      : {}),
  };
}

export const noteSummary = summary(NOTE, 'notes/meeting.md', 'Meeting notes', 'meeting-note');
export const otherSummary = summary(OTHER, 'notes/plan.md', 'Plan', 'plan');
export const sourceSummary = summary(SOURCE, 'report.pdf.md', 'Annual report', 'report', 'source');

// A listing of drafts finds these items, unless a test says otherwise.
knowItems((itemId) =>
  [noteSummary, otherSummary, sourceSummary].find((item) => item.id === itemId),
);

export function listing(
  folders: string[],
  items: ItemSummary[],
  nextCursor: string | null = null,
): ListItemsResponse {
  return { folders, items, revision: HEAD, next_cursor: nextCursor };
}

export function itemDocument(
  item: ItemSummary,
  properties: Record<string, unknown> = {},
): ItemDocument {
  return {
    summary: item,
    body: `# ${item.title}`,
    content_digest: DIGEST,
    properties: { type: item.type_name, okf_jawn: { id: item.id }, ...properties },
  };
}

export function readResponse(
  item: ItemSummary,
  markdown: string,
  extra: Partial<ReadItemResponse> = {},
  selection: Selection = { kind: 'all' },
): ReadItemResponse {
  return {
    markdown,
    media: [],
    outline: [],
    receipt_id: RECEIPT,
    source: {
      workspace_id: WS,
      item_id: item.id,
      path: item.path,
      revision: item.revision,
      selection,
      locations: [],
    },
    truncated: false,
    view: 'text',
    warnings: [],
    ...extra,
  };
}

export const sourcesOfNote: GetSourcesResponse = {
  revision: HEAD,
  sources: [
    {
      workspace_id: WS,
      item_id: SOURCE,
      path: 'report.pdf.md',
      revision: HEAD,
      digest: DIGEST,
      selection: { kind: 'pages', range: { start: 2, end: 3 } },
      locations: [
        { provenance: 'direct', locator: { kind: 'page', page_no: 2 } },
        { provenance: 'inferred', locator: { kind: 'page', page_no: 3 } },
        { provenance: 'unresolved', reason: { kind: 'ambiguous_match', occurrences: 4 } },
      ],
    },
  ],
  declared: [
    { entry: { id: 'report', path: 'report.pdf.md' }, outcome: { kind: 'cited', index: 0 } },
    { entry: 'https://example.org/elsewhere', outcome: { kind: 'uncited', reason: 'external' } },
  ],
};

/**
 * Render the whole app (router, query cache, Explorer) at `url`, with a fresh Explorer store and no
 * editing session left from another test.
 */
export function renderAt(url: string) {
  resetDraftSessions();
  useExplorerStore.setState({
    views: { [WS]: emptyView() },
    layouts: {},
    nextPane: 2,
    focusRequest: null,
    treeReveal: null,
    editRequest: null,
    deleted: {},
  });
  const router = createAppRouter(createMemoryHistory({ initialEntries: [url] }));
  const queryClient = createQueryClient();
  const view = render(<App router={router} queryClient={queryClient} />);
  return { router, queryClient, ...view };
}

/** Answers for the whole Explorer over this workspace; a test overrides the operations it studies. */
export function explorerHandlers(overrides: Record<string, Handler> = {}): Record<string, Handler> {
  const itemOf = (body: Json) =>
    body.item_id === NOTE ? noteSummary : body.item_id === SOURCE ? sourceSummary : otherSummary;
  return {
    '/api/workspaces/open-workspace': () => ok(workspace),
    '/api/items/list-items': (body) =>
      body.folder === 'notes'
        ? ok(listing([], [noteSummary, otherSummary]))
        : ok(listing(['notes'], [sourceSummary])),
    '/api/items/get-item': (body) => ok(itemDocument(itemOf(body), { owner: 'Ada' })),
    '/api/reads/read-item': (body) => {
      const item = itemOf(body);
      return ok(readResponse(item, `# ${item.title}\n\nText of ${item.title}`));
    },
    '/api/reads/get-sources': () => ok(sourcesOfNote),
    // The person has no drafts, and the workspace defines no types, unless a test says so.
    '/api/items/list-drafts': () => ok(draftsPage([])),
    '/api/types/list-types': () => ok({ items: [] }),
    ...overrides,
  };
}
