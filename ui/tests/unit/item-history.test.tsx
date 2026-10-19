/**
 * An item's own history on its page: its Timeline (with each version's Changes, a way to read
 * it, and Rewind of this item alone), Who changed each line, and a deleted item brought back by
 * Rewind, after which the page no longer remembers it as deleted (ruling R7; SPEC section 3:
 * "Attribution says who changed a passage, not who originated every fact").
 */
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { BlameLine, Commit } from '../../src/api/generated/types.gen';
import { useExplorerStore } from '../../src/features/explorer/store';
import { LOG_PAGE, WHO_PAGE } from '../../src/features/history/history';
import { type Handler, type Json, mockApi, ok } from './support/api';
import {
  BLAME,
  changeOf,
  DIFF,
  diffOf,
  LOG,
  OLDER,
  RESTORE,
  REWOUND,
  rewound,
  VERSIONS,
} from './support/history';
import {
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  OLD,
  readResponse,
  renderAt,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const ITEM = '/api/items/get-item';
const READ = '/api/reads/read-item';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const budgetDiff: Handler = (body) =>
  ok(
    diffOf(body, [
      changeOf('notes/meeting.md', 'The budget is draft.', 'The budget is published.'),
    ]),
  );

function setup(overrides: Record<string, Handler> = {}, who = writableWorkspace) {
  return mockApi(
    explorerHandlers({
      [OPEN]: () => ok(who),
      [LOG]: () => ok({ commits: VERSIONS.slice(0, 3), next_cursor: null }),
      [DIFF]: budgetDiff,
      ...overrides,
    }),
  );
}

async function openNote(url = `/w/${WS}/i/${NOTE}`) {
  renderAt(url);
  await screen.findByText('Text of Meeting notes');
}

describe('an item’s Timeline', { timeout: 30_000 }, () => {
  it('is shown on demand, asks for the item’s versions at the revision the tab reads, and keeps focus on its button', async () => {
    const api = setup();
    await openNote();
    expect(api.callsTo(LOG)).toEqual([]);
    const show = screen.getByRole('button', { name: 'Show this item’s Timeline' });
    act(() => show.focus());
    fireEvent.click(show);
    const timeline = await screen.findByRole('region', { name: 'Timeline of Meeting notes' });
    expect(within(timeline).getByRole('heading', { level: 3 })).toBeDefined();
    await within(timeline).findByText('Draft the budget');
    expect(show.getAttribute('aria-expanded')).toBe('true');
    expect(document.activeElement).toBe(show);
    expect(api.callsTo(LOG)).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'revision', revision: HEAD },
        item_id: NOTE,
        page: { limit: LOG_PAGE },
      },
    ]);
  });

  it('compares a version with the one before for this item only, and reads a version in the tab', async () => {
    const api = setup();
    await openNote();
    fireEvent.click(screen.getByRole('button', { name: 'Show this item’s Timeline' }));
    const timeline = await screen.findByRole('region', { name: 'Timeline of Meeting notes' });
    const entry = (await within(timeline).findByText('Draft the budget')).closest('li');
    if (entry === null) throw new Error('no entry');
    fireEvent.click(within(entry).getByRole('button', { name: 'Show its changes' }));
    await within(entry).findByRole('region', { name: 'Its changes' });
    expect(api.callsTo(DIFF)).toEqual([{ workspace_id: WS, from: OLDER, to: OLD, item_id: NOTE }]);
    const read = within(entry).getByRole('link', { name: 'Read this version' });
    expect(read.getAttribute('href')).toBe(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
  });

  it('rewinds this item alone on the head shown, and the tab then reads the version Rewind made', async () => {
    const api = setup({ [RESTORE]: () => ok(rewound()) });
    // Pinned to the head: a tab that cites the head as it was follows the person's own change.
    await openNote(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    fireEvent.click(screen.getByRole('button', { name: 'Show this item’s Timeline' }));
    const timeline = await screen.findByRole('region', { name: 'Timeline of Meeting notes' });
    const entry = (await within(timeline).findByText('Draft the budget')).closest('li');
    if (entry === null) throw new Error('no entry');
    fireEvent.click(within(entry).getByRole('button', { name: 'Rewind to this version…' }));
    const panel = await within(entry).findByRole('region', {
      name: `Rewind Meeting notes to revision ${OLD.slice(0, 10)}`,
    });
    expect(panel.textContent).toContain('Rewind makes this item as it was');
    await within(panel).findByRole('region', { name: 'What Rewind will change' });
    expect(api.callsTo(DIFF)).toContainEqual({
      workspace_id: WS,
      from: HEAD,
      to: OLD,
      item_id: NOTE,
    });
    fireEvent.click(within(panel).getByRole('button', { name: 'Rewind' }));
    await within(panel).findByText(/^Rewound: Meeting notes is as it was/);
    expect(api.callsTo(RESTORE).at(0)).toMatchObject({
      workspace_id: WS,
      base_revision: HEAD,
      target: OLD,
      item_id: NOTE,
    });
    // The tab cited the head as it was; it now cites the version the Rewind made.
    await waitFor(() =>
      expect(
        api.callsTo(ITEM).some((body) => (body.at as Json | undefined)?.revision === REWOUND),
      ).toBe(true),
    );
    expect(api.violations).toEqual([]);
  });

  it('offers no Rewind to a person who may only read, and says why', async () => {
    setup({}, workspace);
    await openNote();
    fireEvent.click(screen.getByRole('button', { name: 'Show this item’s Timeline' }));
    const timeline = await screen.findByRole('region', { name: 'Timeline of Meeting notes' });
    await within(timeline).findByText('Draft the budget');
    expect(within(timeline).queryByRole('button', { name: 'Rewind to this version…' })).toBeNull();
    expect(
      within(timeline).getByText(/^Rewind, which brings back an earlier version, needs/),
    ).toBeDefined();
  });
});

/** The version each line was last changed in, as Who answers it. */
const LINE_VERSION: Commit = VERSIONS.at(1) as Commit;
const OTHER_VERSION: Commit = VERSIONS.at(0) as Commit;

/** Who answers the lines asked, each last changed in LINE_VERSION but the third. */
const whoAnswer: Handler = (body) => {
  const lines = body.lines as { start: number; end: number };
  const answered: BlameLine[] = [];
  for (let line = lines.start; line <= lines.end; line += 1) {
    answered.push({
      line,
      text: `Line ${line}`,
      commit: line === 3 ? OTHER_VERSION : LINE_VERSION,
    });
  }
  return ok({
    lines: answered,
    source: {
      workspace_id: WS,
      item_id: NOTE,
      path: noteSummary.path,
      revision: HEAD,
      selection: { kind: 'lines', range: lines },
      locations: [],
    },
  });
};

/** An item whose text has `count` lines, the last one ended by a line break. */
const linesItem =
  (count: number): Handler =>
  () =>
    ok({
      ...itemDocument(noteSummary),
      body: `${Array.from({ length: count }, (_, line) => `Line ${line + 1}`).join('\n')}\n`,
    });

describe('Who changed each line', { timeout: 30_000 }, () => {
  it('asks for the lines of the text at the revision read, and says it is who changed a line, not who wrote it first', async () => {
    const api = setup({ [ITEM]: linesItem(4), [BLAME]: whoAnswer });
    await openNote();
    fireEvent.click(screen.getByRole('button', { name: 'Show who changed each line' }));
    const who = await screen.findByRole('region', { name: 'Who changed each line' });
    await within(who).findByText('Line 1');
    expect(api.callsTo(BLAME)).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        at: { kind: 'revision', revision: HEAD },
        lines: { start: 1, end: 4 },
      },
    ]);
    expect(who.textContent).toContain('not who first wrote what it says');
    const rows = within(who).getAllByRole('row').slice(1);
    expect(rows).toHaveLength(4);
    // A run of lines changed in one version names it once; the next one names its own.
    expect(rows.at(0)?.textContent).toContain('Draft the budget');
    expect(rows.at(1)?.textContent).not.toContain('Draft the budget');
    expect(rows.at(1)?.textContent).toContain('the same version as the line above');
    expect(rows.at(2)?.textContent).toContain('Publish the budget');
    expect(rows.at(3)?.textContent).toContain('Draft the budget');
  });

  it('asks a page of lines at a time, and the next page from the line after', async () => {
    const api = setup({ [ITEM]: linesItem(WHO_PAGE + 5), [BLAME]: whoAnswer });
    await openNote();
    fireEvent.click(screen.getByRole('button', { name: 'Show who changed each line' }));
    const who = await screen.findByRole('region', { name: 'Who changed each line' });
    await within(who).findByText(`Line ${WHO_PAGE}`);
    expect(api.callsTo(BLAME).at(0)?.lines).toEqual({ start: 1, end: WHO_PAGE });
    const more = within(who).getByRole('button', { name: 'Show the next lines' });
    act(() => more.focus());
    fireEvent.click(more);
    await within(who).findByText(`Line ${WHO_PAGE + 5}`);
    expect(api.callsTo(BLAME).at(1)?.lines).toEqual({ start: WHO_PAGE + 1, end: WHO_PAGE + 5 });
    // Focus goes to the first line it brought, never to the page body.
    await waitFor(() =>
      expect(document.activeElement?.textContent).toContain(`Line ${WHO_PAGE + 1}`),
    );
    expect(within(who).queryByRole('button', { name: 'Show the next lines' })).toBeNull();
  });

  it('asks nothing for an item with no text, and says so', async () => {
    const api = setup({ [ITEM]: () => ok({ ...itemDocument(noteSummary), body: '' }) });
    await openNote();
    fireEvent.click(screen.getByRole('button', { name: 'Show who changed each line' }));
    const who = await screen.findByRole('region', { name: 'Who changed each line' });
    expect(who.textContent).toContain('This item has no text');
    expect(api.callsTo(BLAME)).toEqual([]);
  });
});

describe('a deleted item brought back', { timeout: 30_000 }, () => {
  /** The head the delete made. */
  const MADE = '9'.repeat(40);

  it('is offered from its tab, rewinds the item to the version before the delete, and the page forgets it was deleted', async () => {
    let restored = false;
    const api = setup({
      [OPEN]: () => ok({ ...writableWorkspace, head: MADE }),
      [ITEM]: (body) => {
        const at = (body.at as Json | undefined)?.revision;
        return at === MADE
          ? { status: 404, body: { code: 'not_found', message: 'no such item at this revision' } }
          : ok(itemDocument(noteSummary));
      },
      [READ]: () => ok(readResponse(noteSummary, '# Meeting notes\n\nText of Meeting notes')),
      [DIFF]: (body) =>
        ok(
          diffOf(body, [
            {
              new_path: 'notes/meeting.md',
              kind: 'added',
              binary: false,
              patch: '@@ -0,0 +1,1 @@\n+# Meeting notes\n',
            },
          ]),
        ),
      [RESTORE]: () => {
        restored = true;
        return ok(rewound());
      },
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    // The person deleted the item in this page: the head shown is the one the delete made.
    act(() =>
      useExplorerStore
        .getState()
        .markDeleted(WS, NOTE, { title: 'Meeting notes', at: HEAD, made: MADE }),
    );
    await screen.findByText('You deleted this item.');
    const back = screen.getByRole('button', { name: 'Bring it back…' });
    fireEvent.click(back);
    const panel = await screen.findByRole('region', {
      name: `Rewind Meeting notes to revision ${HEAD.slice(0, 10)}`,
    });
    await within(panel).findByRole('region', { name: 'What Rewind will change' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Rewind' }));
    await waitFor(() => expect(restored).toBe(true));
    expect(api.callsTo(RESTORE).at(0)).toMatchObject({
      base_revision: MADE,
      target: HEAD,
      item_id: NOTE,
    });
    // At the head the Rewind made the item answers again: it is no longer remembered as deleted.
    await waitFor(() => expect(screen.queryByText('You deleted this item.')).toBeNull());
    // Focus was on the words of the Rewind, inside the notice that left: it is on the page's
    // heading, never on the body.
    expect(document.activeElement).toBe(
      screen.getByRole('heading', { level: 1, name: 'Workshop notes' }),
    );
    expect(useExplorerStore.getState().deleted[`${WS}/${NOTE}`]).toBeUndefined();
    expect(
      api.callsTo(ITEM).some((body) => (body.at as Json | undefined)?.revision === REWOUND),
    ).toBe(true);
    await screen.findByText('Text of Meeting notes');
  });

  it('is not offered to a person who may not change the workspace', async () => {
    setup({ [OPEN]: () => ok({ ...workspace, head: MADE }) });
    renderAt(`/w/${WS}/i/${NOTE}`);
    act(() =>
      useExplorerStore
        .getState()
        .markDeleted(WS, NOTE, { title: 'Meeting notes', at: HEAD, made: MADE }),
    );
    await screen.findByText('You deleted this item.');
    expect(screen.queryByRole('button', { name: 'Bring it back…' })).toBeNull();
  });
});
