/**
 * A Source's properties, changed through a draft whose text stays as committed (carried to UI M3:
 * progress.md, sweep-1 item 7 ruling: "`save_draft` refuses a changed body for a source card ...
 * and still takes a properties-only draft"; SPEC section 5: "Generated extraction and human
 * corrections remain distinguishable"). Its text changes only through corrections, which are not
 * offered here; a draft of it that the service holds is shown when the Source is opened.
 */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { type Handler, mockApi, ok } from './support/api';
import {
  draftOf,
  explorerHandlers,
  HEAD,
  itemDocument,
  OLD,
  renderAt,
  SOURCE,
  sourceSummary,
  WS,
  writableWorkspace,
} from './support/workspace';

const SAVE = '/api/items/save-draft';
const TYPES = '/api/types/list-types';
const ITEM = '/api/items/get-item';
const LATER = '2026-01-02T08:15:00.000Z';

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** The Source's type: one property. */
const report = {
  name: 'report',
  schema_version: 1,
  properties_schema: {
    type: 'object',
    properties: { owner: { type: 'string', title: 'Owner' } },
  },
  ui_schema: {},
};

const COMMITTED = '# Annual report';
const stored = { type: 'report', owner: 'Ada', okf_jawn: { id: SOURCE } };

function setup(overrides: Record<string, Handler> = {}) {
  return mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      [ITEM]: () => ok({ ...itemDocument(sourceSummary), body: COMMITTED, properties: stored }),
      [TYPES]: () => ok({ items: [report] }),
      [SAVE]: (body) => ok(draftOf(String(body.item_id), String(body.base_revision), LATER)),
      ...overrides,
    }),
  );
}

const field = (label: string) => screen.getByLabelText(new RegExp(`^${label}\\*?$`));

describe('a Source’s properties draft', { timeout: 30_000 }, () => {
  it('is offered as an edit of its properties, with no text box, and saves them with its text unchanged', async () => {
    const api = setup();
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const edit = await screen.findByRole('button', { name: 'Edit properties' });
    await screen.findByText('Text of Annual report');
    expect(screen.queryByRole('button', { name: 'Edit' })).toBeNull();
    fireEvent.click(edit);
    await waitFor(() => field('Owner'));
    // Its text is not edited here: no text box, and the draft says why.
    expect(screen.queryByRole('textbox', { name: 'Text of Annual report' })).toBeNull();
    expect(
      screen.getByText(
        'A Source’s text changes only through corrections, so this draft changes its properties alone; its text stays as committed.',
      ),
    ).toBeDefined();
    vi.useFakeTimers();
    fireEvent.change(field('Owner'), { target: { value: 'Grace' } });
    await act(() => vi.advanceTimersByTimeAsync(AUTOSAVE_DELAY_MS + 50));
    vi.useRealTimers();
    await waitFor(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)).toMatchObject({
      workspace_id: WS,
      item_id: SOURCE,
      base_revision: HEAD,
      body: COMMITTED,
      properties: { ...stored, owner: 'Grace' },
    });
    expect(api.violations).toEqual([]);
  });

  it('opens on a properties draft the service holds, marked as a draft', async () => {
    setup({
      [ITEM]: () =>
        ok({
          ...itemDocument(sourceSummary),
          body: COMMITTED,
          properties: stored,
          draft: {
            body: COMMITTED,
            properties: { ...stored, owner: 'Grace' },
            draft: draftOf(SOURCE),
          },
        }),
    });
    renderAt(`/w/${WS}/i/${SOURCE}`);
    await waitFor(() => expect((field('Owner') as HTMLInputElement).value).toBe('Grace'));
    expect(screen.getByText('Draft.')).toBeDefined();
    expect(screen.getByRole('button', { name: 'Read the committed text' })).toBeDefined();
    expect(screen.queryByRole('textbox', { name: 'Text of Annual report' })).toBeNull();
  });

  it('is started from the version the workspace shows: a tab at another revision says so, and offers it', async () => {
    setup();
    renderAt(`/w/${WS}/i/${SOURCE}?rev=${OLD}`);
    await screen.findByText('Text of Annual report');
    await screen.findByText(/Editing starts from the version this workspace shows/);
    expect(screen.queryByRole('button', { name: 'Edit properties' })).toBeNull();
    expect(
      screen.getByRole('button', { name: 'Open the version this workspace shows' }),
    ).toBeDefined();
  });
});
