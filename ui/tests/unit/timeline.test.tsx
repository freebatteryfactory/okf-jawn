/**
 * The workspace's Timeline page: its versions at the head the workspace shows, each version's
 * Changes, and Rewind with what it will change, in a workspace the person may change, may only
 * read, or that is archived (SPEC section 8: "Rewind restores selected historical state in a new
 * snapshot and does not erase history").
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
import { LOG_PAGE } from '../../src/features/history/history';
import { formatInstant } from '../../src/lib/instant';
import { fail, type Handler, mockApi, ok, type Reply } from './support/api';
import {
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
  OLD,
  renderAt,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** Every version in one page, unless a test says otherwise. */
const allVersions: Handler = () => ok({ commits: VERSIONS, next_cursor: null });

/** A comparison of the budget line, whatever the two versions. */
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
      [LOG]: allVersions,
      [DIFF]: budgetDiff,
      ...overrides,
    }),
  );
}

async function openTimeline() {
  renderAt(`/w/${WS}/timeline`);
  return screen.findByRole('region', { name: 'Timeline' });
}

/** The entry of the version whose message is `message`. */
async function entryOf(message: string) {
  const text = await screen.findByText(message);
  const entry = text.closest('li');
  if (entry === null) throw new Error(`no entry for ${message}`);
  return entry;
}

describe('the Timeline page', { timeout: 30_000 }, () => {
  it.each(['pending', 'failed'] as const)(
    'never labels old Timeline pages as the new head while its listing is %s',
    async (outcome) => {
      // SPEC sections 3/7: each page stays tied to the revision it was read at. The old
      // list may remain for Rewind focus, but its label and cursor must not claim the new tip.
      const next = 'c'.repeat(40);
      let head = HEAD;
      let finish: ((reply: Reply) => void) | undefined;
      const api = setup({
        [OPEN]: () => ok({ ...writableWorkspace, head }),
        [LOG]: (body) =>
          (body.at as { revision: string }).revision === HEAD
            ? ok({ commits: VERSIONS.slice(0, 2), next_cursor: 'old-page' })
            : new Promise<Reply>((resolve) => {
                finish = resolve;
              }),
      });
      const page = await openTimeline();
      await entryOf('Publish the budget');
      head = next;
      fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
      await screen.findByText(`Refreshed: now showing revision ${next.slice(0, 10)}.`);
      await waitFor(() => expect(finish).toBeTypeOf('function'));
      if (outcome === 'failed') {
        // A non-retryable read failure settles immediately; the application timeout is unchanged.
        act(() => finish?.(fail(501, 'not_implemented', 'the new history could not be read')));
        await within(page).findByRole('alert');
        expect(within(page).queryByText('No versions to show.')).toBeNull();
      }
      const oldEntry = within(page).queryByText('Publish the budget');
      if (oldEntry !== null) {
        expect(page.querySelector('p.meta code')?.getAttribute('title')).toBe(HEAD);
      }
      const older = within(page).queryByRole('button', { name: 'Show older versions' });
      expect(older === null || older.getAttribute('aria-disabled') === 'true').toBe(true);
      expect(api.callsTo(LOG).at(-1)?.at).toEqual({ kind: 'revision', revision: next });
      expect(api.violations).toEqual([]);
      if (outcome === 'pending') act(() => finish?.(ok({ commits: [], next_cursor: null })));
    },
  );

  it('is in the workspace’s bar and lists the versions at the head shown, newest first, by message and local time', async () => {
    const api = setup();
    renderAt(`/w/${WS}`);
    const link = await screen.findByRole('link', { name: 'Timeline' });
    fireEvent.click(link);
    const page = await screen.findByRole('region', { name: 'Timeline' });
    expect(screen.getByRole('heading', { level: 2, name: 'Timeline' })).toBeDefined();
    await within(page).findByText('Publish the budget');
    const entries = within(page).getAllByRole('listitem');
    expect(entries.map((entry) => entry.querySelector('strong')?.textContent)).toEqual([
      'Publish the budget',
      'Draft the budget',
      'Add the meeting notes',
      'Start the workspace',
    ]);
    expect(entries.at(0)?.textContent).toContain(formatInstant('2026-01-01T12:00:00.000Z') ?? '');
    expect(entries.at(0)?.textContent).not.toContain(HEAD);
    expect(api.callsTo(LOG)).toEqual([
      { workspace_id: WS, at: { kind: 'revision', revision: HEAD }, page: { limit: LOG_PAGE } },
    ]);
    expect(api.violations).toEqual([]);
  });

  it('shows what a version changed against the one before it, without Git’s header, keeping focus on its button', async () => {
    const api = setup();
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    const show = within(entry).getByRole('button', { name: 'Show its changes' });
    act(() => show.focus());
    fireEvent.click(show);
    const changes = await within(entry).findByRole('region', { name: 'Its changes' });
    expect(show.getAttribute('aria-expanded')).toBe('true');
    expect(show.textContent).toBe('Hide its changes');
    expect(document.activeElement).toBe(show);
    expect(api.callsTo(DIFF)).toEqual([{ workspace_id: WS, from: OLDER, to: OLD }]);
    expect(changes.querySelector('ins')?.textContent).toBe('Added: + The budget is published.\n');
    expect(changes.textContent).not.toContain('diff --git');
    expect(changes.textContent).not.toContain('+++ b/');
  });

  it('says the first version has nothing earlier to compare, and asks nothing for it', async () => {
    const api = setup();
    await openTimeline();
    const entry = await entryOf('Start the workspace');
    fireEvent.click(within(entry).getByRole('button', { name: 'Show its changes' }));
    expect(
      await within(entry).findByText(
        'This is the first version: there is nothing earlier to compare it with.',
      ),
    ).toBeDefined();
    expect(api.callsTo(DIFF)).toEqual([]);
  });

  it('asks for older versions from the cursor and moves focus to the first one it brought', async () => {
    const api = setup({
      [LOG]: (body) => {
        const page = body.page as { cursor?: string | null };
        return page.cursor === 'later'
          ? ok({ commits: VERSIONS.slice(2), next_cursor: null })
          : ok({ commits: VERSIONS.slice(0, 2), next_cursor: 'later' });
      },
    });
    await openTimeline();
    await entryOf('Draft the budget');
    expect(screen.queryByText('Add the meeting notes')).toBeNull();
    const more = screen.getByRole('button', { name: 'Show older versions' });
    act(() => more.focus());
    fireEvent.click(more);
    const third = await entryOf('Add the meeting notes');
    await waitFor(() => expect(document.activeElement).toBe(third));
    expect(api.callsTo(LOG).at(1)?.page).toEqual({ limit: LOG_PAGE, cursor: 'later' });
    expect(screen.queryByRole('button', { name: 'Show older versions' })).toBeNull();
  });

  it('says why the versions could not be listed, never that there are none', async () => {
    setup({ [LOG]: () => fail(501, 'not_implemented', 'log_items is not built') });
    await openTimeline();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Listing the versions: Not available in this build.');
    expect(screen.queryByText('No versions to show.')).toBeNull();
  });
});

describe('Rewind from the Timeline', { timeout: 30_000 }, () => {
  it('is not offered to a person who may only read, and the page says why', async () => {
    setup({}, workspace);
    await openTimeline();
    await entryOf('Draft the budget');
    expect(screen.queryByRole('button', { name: 'Rewind to this version…' })).toBeNull();
    expect(
      screen.getByText(
        'Rewind, which brings back an earlier version, needs permission to change this workspace, which you do not have.',
      ),
    ).toBeDefined();
  });

  it('is offered for every version but the one the workspace shows', async () => {
    setup();
    await openTimeline();
    const head = await entryOf('Publish the budget');
    expect(within(head).queryByRole('button', { name: 'Rewind to this version…' })).toBeNull();
    for (const message of ['Draft the budget', 'Add the meeting notes', 'Start the workspace']) {
      const entry = await entryOf(message);
      expect(within(entry).getByRole('button', { name: 'Rewind to this version…' })).toBeDefined();
    }
  });

  it('says in plain words that nothing is erased, shows what it will change, then rewinds on the head shown and adopts the version it made', async () => {
    const api = setup({ [RESTORE]: () => ok(rewound()) });
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    fireEvent.click(within(entry).getByRole('button', { name: 'Rewind to this version…' }));
    const panel = await within(entry).findByRole('region', {
      name: `Rewind the workspace to revision ${OLD.slice(0, 10)}`,
    });
    // The panel takes focus on its heading, so its words are read first.
    expect(document.activeElement).toBe(within(panel).getByRole('heading'));
    expect(panel.textContent).toContain('No version is erased');
    const preview = await within(panel).findByRole('region', { name: 'What Rewind will change' });
    expect(preview.querySelector('del')?.textContent).toBe('Removed: − The budget is draft.\n');
    expect(api.callsTo(DIFF)).toEqual([{ workspace_id: WS, from: HEAD, to: OLD }]);
    const button = within(panel).getByRole('button', { name: 'Rewind' });
    act(() => button.focus());
    fireEvent.click(button);
    const done = await within(panel).findByText(/^Rewound:/);
    expect(done.textContent).toContain('Every earlier version is kept.');
    expect(done.textContent).toContain('The workspace now shows the new version.');
    const sent = api.callsTo(RESTORE);
    expect(sent).toHaveLength(1);
    expect(sent.at(0)).toMatchObject({
      workspace_id: WS,
      base_revision: HEAD,
      target: OLD,
      message: `Rewind the workspace to revision ${OLD.slice(0, 10)}`,
    });
    expect(sent.at(0)).not.toHaveProperty('item_id');
    // The workspace shows the version the Rewind made, without a Refresh.
    await screen.findByText(REWOUND.slice(0, 10), { selector: '.head-line code' });
    // Focus left with the form and is on the words that say what was done, never the body.
    await waitFor(() => expect(document.activeElement).toBe(done.parentElement));
    expect(api.violations).toEqual([]);
  });

  it('gives focus back to the button that opened it when it is closed, by Cancel or Escape', async () => {
    setup();
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    const opener = within(entry).getByRole('button', { name: 'Rewind to this version…' });
    fireEvent.click(opener);
    const panel = await within(entry).findByRole('region', { name: /^Rewind the workspace/ });
    fireEvent.click(within(panel).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(document.activeElement).toBe(opener));
    fireEvent.click(opener);
    const again = await within(entry).findByRole('region', { name: /^Rewind the workspace/ });
    fireEvent.keyDown(within(again).getByRole('textbox'), { key: 'Escape' });
    await waitFor(() => expect(document.activeElement).toBe(opener));
    expect(within(entry).queryByRole('region', { name: /^Rewind the workspace/ })).toBeNull();
  });

  it('repeats a Rewind that failed under the same key, and sends a changed one under a new key', async () => {
    let answers = 0;
    const api = setup({
      [RESTORE]: () => {
        answers += 1;
        return answers === 1 ? fail(503, 'unavailable', 'busy') : ok(rewound());
      },
    });
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    fireEvent.click(within(entry).getByRole('button', { name: 'Rewind to this version…' }));
    const panel = await within(entry).findByRole('region', { name: /^Rewind the workspace/ });
    await within(panel).findByRole('region', { name: 'What Rewind will change' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Rewind' }));
    const retry = await within(panel).findByRole('button', { name: 'Try again' });
    fireEvent.click(retry);
    await within(panel).findByText(/^Rewound:/);
    const keys = api.callsTo(RESTORE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys.at(1)).toBe(keys.at(0));
  });

  it('offers nothing to rewind when nothing differs from the version shown, and sends nothing', async () => {
    const api = setup({ [DIFF]: (body) => ok(diffOf(body, [])) });
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    fireEvent.click(within(entry).getByRole('button', { name: 'Rewind to this version…' }));
    const panel = await within(entry).findByRole('region', { name: /^Rewind the workspace/ });
    await within(panel).findByText(/there is nothing to rewind/);
    const button = within(panel).getByRole('button', { name: 'Rewind' });
    expect(button.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(button);
    expect(api.callsTo(RESTORE)).toEqual([]);
  });

  it('stays offered in an archived workspace and says why it cannot run, sending nothing', async () => {
    const archivedAt = '2026-01-01T08:00:00.000Z';
    const api = setup({}, { ...writableWorkspace, archived_at: archivedAt });
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    const opener = within(entry).getByRole('button', { name: 'Rewind to this version…' });
    const words = `This workspace was archived on ${formatInstant(archivedAt)}: nothing in it can be changed until it is unarchived. It can still be read.`;
    expect(screen.getAllByText(words).length).toBeGreaterThan(0);
    fireEvent.click(opener);
    const panel = await within(entry).findByRole('region', { name: /^Rewind the workspace/ });
    await within(panel).findByRole('region', { name: 'What Rewind will change' });
    const button = within(panel).getByRole('button', { name: 'Rewind' });
    expect(button.getAttribute('aria-disabled')).toBe('true');
    const describedBy = button.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(describedBy)?.textContent).toBe(words);
    fireEvent.click(button);
    expect(api.callsTo(RESTORE)).toEqual([]);
  });

  it('words the service’s refusal of an archived workspace when the page did not know it', async () => {
    setup({
      [RESTORE]: () => ({
        status: 409,
        body: {
          code: 'conflict',
          message: 'the workspace is archived',
          detail: { kind: 'workspace_archived', archived_at: '2026-01-01T08:00:00.000Z' },
        },
      }),
    });
    await openTimeline();
    const entry = await entryOf('Draft the budget');
    fireEvent.click(within(entry).getByRole('button', { name: 'Rewind to this version…' }));
    const panel = await within(entry).findByRole('region', { name: /^Rewind the workspace/ });
    await within(panel).findByRole('region', { name: 'What Rewind will change' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Rewind' }));
    const alert = await within(panel).findByRole('alert');
    expect(alert.textContent).toContain('Rewinding: This workspace is archived.');
  });
});
