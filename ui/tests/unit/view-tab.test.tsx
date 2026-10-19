/**
 * A saved View in its Explorer tab (M4b ruling B1): the tab draws the View (`SavedView` over the
 * Explorer's reads, `documents/view-data.ts`), resolved at the revision the tab reads and never
 * at "the latest"; the item's text stays one step away, rendered; each source the View cites
 * opens in the Explorer at the revision the View pins; and an archived workspace still shows it,
 * because every View read is allowed there (`ALLOWED_WHILE_ARCHIVED`).
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { GetObjectRequest, ReadItemRequest } from '../../src/api/generated/types.gen';
import { explorerViewData } from '../../src/features/documents/view-data';
import { fail, type Handler, mockApi, ok } from './support/api';
import { EXPORT_JOB, EXPORT_VIEW } from './support/exports';
import { accepted, jobOf } from './support/imports';
import {
  CITED_AT,
  DATASET_SHA,
  FIGURES,
  FIGURES_PATH,
  figuresRead,
  GET_OBJECT,
  GET_VIEW,
  objectBlock,
  READ,
  RESOLVE_VIEW,
  savedViewOf,
  VIEW,
  viewHandlers,
  viewOf,
} from './support/views-explorer';
import {
  explorerHandlers,
  HEAD,
  NOW,
  OLD,
  renderAt,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function setup(overrides: Record<string, Handler> = {}) {
  const base = explorerHandlers();
  return mockApi({ ...base, ...viewHandlers(base), ...overrides });
}

/** The View's table, once the tab has resolved it and read its dataset. */
const viewTable = () => screen.findByRole('table', { name: 'revenue' });

describe('a saved View in its tab', { timeout: 30_000 }, () => {
  it('draws the View, resolved at the revision the tab reads, and never asks for the latest', async () => {
    const api = setup();
    renderAt(`/w/${WS}/i/${VIEW}`);
    const table = await viewTable();
    const view = screen.getByRole('region', { name: 'The View' });
    expect(view.contains(table)).toBe(true);
    expect(
      within(table)
        .getAllByRole('row')
        .map((row) => row.textContent),
    ).toEqual(['quarterrevenue', 'Q11.2', 'Q21.4', 'Q31.9']);
    // The tab reads the head the workspace shows: that resolved revision, never `latest`.
    expect(api.callsTo(RESOLVE_VIEW).length).toBeGreaterThan(0);
    for (const sent of api.callsTo(RESOLVE_VIEW))
      expect(sent).toEqual({
        workspace_id: WS,
        item_id: VIEW,
        at: { kind: 'revision', revision: HEAD },
        refresh_live: false,
      });
    for (const sent of api.callsTo(GET_VIEW))
      expect(sent.at).toEqual({ kind: 'revision', revision: HEAD });
    // The dataset was read from the source it cites, block by block, at the pinned revision.
    expect(api.callsTo(GET_OBJECT).at(0)).toMatchObject({
      object: DATASET_SHA,
      offset: '0',
      source: { item_id: FIGURES, revision: CITED_AT },
    });
    expect(
      api.calls.some((call) => JSON.stringify(call.body.at ?? null) === '{"kind":"latest"}'),
    ).toBe(false);
    expect(api.violations).toEqual([]);
  });

  it('resolves a tab that cites a revision at that revision', async () => {
    const api = setup();
    renderAt(`/w/${WS}/i/${VIEW}?rev=${OLD}`);
    await viewTable();
    expect(api.callsTo(RESOLVE_VIEW).map((sent) => sent.at)).toContainEqual({
      kind: 'revision',
      revision: OLD,
    });
    for (const sent of api.callsTo(RESOLVE_VIEW))
      expect(sent.at).toEqual({ kind: 'revision', revision: OLD });
    expect(api.violations).toEqual([]);
  });

  it('keeps the item’s text one step away, rendered, and shows it when the address names a part of it', async () => {
    setup();
    renderAt(`/w/${WS}/i/${VIEW}`);
    await viewTable();
    const text = screen.getByRole('group', { name: 'Text of this View' });
    expect(text).toHaveProperty('open', false);
    // The text is the item's Markdown, rendered: its prose, and the View's block as code.
    await waitFor(() => expect(within(text).getByText('2025').tagName).toBe('STRONG'));
    expect(within(text).getByText(/"grammar": "json_render"/)).toBeDefined();
    cleanup();

    setup();
    const lines = { kind: 'lines', range: { start: 1, end: 1 } };
    renderAt(`/w/${WS}/i/${VIEW}?sel=${encodeURIComponent(JSON.stringify(lines))}`);
    await viewTable();
    expect(screen.getByRole('group', { name: 'Text of this View' })).toHaveProperty('open', true);
  });

  it('opens a cited source in the Explorer at the revision the View pins', async () => {
    const lines = { kind: 'lines' as const, range: { start: 2, end: 4 } };
    const saved = savedViewOf(
      viewOf({
        bindings: viewOf().bindings.map((binding) => ({
          ...binding,
          source: { ...binding.source, selection: lines },
        })),
      }),
    );
    const api = setup({ [GET_VIEW]: () => ok(saved) });
    const { router } = renderAt(`/w/${WS}/i/${VIEW}`);
    await viewTable();
    const cited = screen.getByRole('navigation', { name: 'Sources this View cites' });
    const link = within(cited).getByRole('link', {
      name: `${FIGURES_PATH}, lines 2–4, at revision ${CITED_AT.slice(0, 10)}`,
    });
    fireEvent.click(link);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${FIGURES}`));
    expect(router.state.location.search).toEqual({ rev: CITED_AT, sel: lines });
    await screen.findByRole('tab', { name: 'Revenue figures', selected: true });
    // The source's tab reads the pinned revision, not the head the workspace shows.
    await waitFor(() =>
      expect(api.callsTo(READ)).toContainEqual(
        expect.objectContaining({
          item_id: FIGURES,
          at: { kind: 'revision', revision: CITED_AT },
          selection: lines,
          cursor: null,
        }),
      ),
    );
    expect(api.violations).toEqual([]);
  });

  it('still shows the View and what it cites in an archived workspace', async () => {
    const api = setup({
      '/api/workspaces/open-workspace': () =>
        ok({ ...workspace, permissions: ['read', 'write'], archived_at: NOW }),
    });
    renderAt(`/w/${WS}/i/${VIEW}`);
    await viewTable();
    expect(
      within(screen.getByRole('navigation', { name: 'Sources this View cites' })).getAllByRole(
        'link',
      ),
    ).toHaveLength(1);
    expect(api.callsTo(RESOLVE_VIEW).length).toBeGreaterThan(0);
    expect(api.callsTo(GET_VIEW).length).toBeGreaterThan(0);
    expect(api.violations).toEqual([]);
  });

  it('says in plain words that a View cannot be reopened in a build without it', async () => {
    setup({ [RESOLVE_VIEW]: () => fail(501, 'not_implemented', 'resolve_view is not built') });
    renderAt(`/w/${WS}/i/${VIEW}`);
    const alert = await screen.findByText(/^Reopening this View: Not available in this build\./);
    expect(alert.closest('[role="alert"]')?.textContent).toContain('resolve_view is not built');
  });
});

describe('the Explorer’s reads of a View', () => {
  const read: ReadItemRequest = {
    workspace_id: WS,
    item_id: FIGURES,
    at: { kind: 'revision', revision: CITED_AT },
    view: 'text',
    selection: { kind: 'all' },
    max_bytes: 65_536,
    max_images: 0,
  };
  const block: GetObjectRequest = {
    source: {
      workspace_id: WS,
      item_id: FIGURES,
      path: FIGURES_PATH,
      revision: CITED_AT,
      selection: { kind: 'all' },
    },
    object: DATASET_SHA,
    offset: '0',
    length: 1_048_576,
  };

  it('send the generated request bodies as they are and hand back the answers', async () => {
    const api = mockApi({
      [READ]: (body) => ok(figuresRead(body)),
      [GET_OBJECT]: (body) => ok(objectBlock(body)),
    });
    const data = explorerViewData();
    expect(await data.readItem(read)).toEqual(figuresRead(read));
    expect(await data.getObject(block)).toEqual(objectBlock(block));
    expect(api.callsTo(READ)).toEqual([read]);
    expect(api.callsTo(GET_OBJECT)).toEqual([block]);
    expect(api.violations).toEqual([]);
  });

  it('refuse in plain words, with the service’s own', async () => {
    mockApi({
      [READ]: () => fail(404, 'not_found', 'no such revision'),
    });
    const data = explorerViewData();
    await expect(data.readItem(read)).rejects.toThrow(
      'This could not be found. The service said: “no such revision”',
    );
    await expect(data.getObject(block)).rejects.toThrow(
      'Not available in this build. The service said: “/api/reads/get-object is not implemented in this build”',
    );
  });

  it('refuse in plain words when the service cannot be reached', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new TypeError('Failed to fetch');
      }),
    );
    await expect(explorerViewData().readItem(read)).rejects.toThrow(
      'The workspace service could not be reached.',
    );
  });
});

describe('"Export visual" in a View\'s tab (M4b B3)', { timeout: 30_000 }, () => {
  it('exports the View at the revision the tab reads, never the latest', async () => {
    const api = setup({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      [EXPORT_VIEW]: () => accepted(jobOf('queued', { id: EXPORT_JOB, kind: 'export_view' })),
    });
    renderAt(`/w/${WS}/i/${VIEW}`);
    await viewTable();
    const view = screen.getByRole('region', { name: 'The View' });
    fireEvent.click(within(view).getByRole('button', { name: 'Export visual' }));
    await waitFor(() => expect(api.callsTo(EXPORT_VIEW)).toHaveLength(1));
    expect(api.callsTo(EXPORT_VIEW)[0]).toEqual({
      workspace_id: WS,
      item_id: VIEW,
      at: { kind: 'revision', revision: HEAD },
      idempotency_key: expect.any(String),
    });
    expect(api.violations).toEqual([]);
  });

  it('is not offered to a person who may only read, and says why', async () => {
    const api = setup();
    renderAt(`/w/${WS}/i/${VIEW}`);
    await viewTable();
    const view = screen.getByRole('region', { name: 'The View' });
    expect(within(view).queryByRole('button', { name: 'Export visual' })).toBeNull();
    expect(view.textContent).toContain(
      'Exporting this View needs permission to change this workspace, which you do not have.',
    );
    expect(api.callsTo(EXPORT_VIEW)).toEqual([]);
  });
});
