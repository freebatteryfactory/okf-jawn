/**
 * Snapshot: `commit_items` for the drafts the person selects, under a name; the shell adopting the
 * revision it made; and the per-item Conflict flow (what changed underneath as a diff, keep mine,
 * take theirs).
 */
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { REQUEST_TIMEOUT_MS } from '../../src/app/api-client';
import { itemOptions } from '../../src/features/documents/DocumentView';
import { MAX_DIFF_LINES, MAX_LINE_CHARS, readPatch } from '../../src/features/drafts/patch';
import {
  AUTOSAVE_DELAY_MS,
  MAX_BODY_BYTES,
  resetDraftSessions,
} from '../../src/features/drafts/saving';
import { fail, type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import { findTextBox, replaceText, textBox, textIn, typeInto } from './support/editor';
import {
  draftOf,
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  RECEIPT,
  renderAt,
  SOURCE,
  sourceSummary,
  summary,
  WS,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const SAVE = '/api/items/save-draft';
const DISCARD = '/api/items/discard-draft';
const DRAFTS = '/api/items/list-drafts';
const ITEM = '/api/items/get-item';
const COMMIT = '/api/history/commit-items';
const LIST = '/api/items/list-items';
const LATER = '2026-01-02T08:15:00.000Z';
/** The revision a Snapshot makes. */
const MADE = 'c'.repeat(40);
/** A revision someone else made meanwhile. */
const THEIRS = 'e'.repeat(40);
const DRAFT_BODY = '# Meeting notes\n\nMy draft text';

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** The patch libgit2 writes for one changed file: its header, then two hunks. */
const PATCH = [
  'diff --git a/notes/meeting.md b/notes/meeting.md',
  'index 83db48f..bf269f4 100644',
  '--- a/notes/meeting.md',
  '+++ b/notes/meeting.md',
  '@@ -1,4 +1,4 @@',
  ' ---',
  '-owner: Ada',
  '+owner: Grace',
  ' type: meeting-note',
  ' ---',
  '@@ -9,3 +9,4 @@ # Meeting notes',
  ' The group met.',
  ' ',
  '-It agreed nothing.',
  '+It agreed to publish.',
  '+The budget follows.',
  '\\ No newline at end of file',
  '',
].join('\n');

const conflictOf = (itemId: string, extra: Json = {}) => ({
  item_id: itemId,
  draft_base: HEAD,
  current_revision: THEIRS,
  deleted: false,
  changes: [
    {
      kind: 'modified',
      binary: false,
      old_path: 'notes/meeting.md',
      new_path: 'notes/meeting.md',
      patch: PATCH,
    },
  ],
  ...extra,
});

const conflict = (items: Json[]): Reply => ({
  status: 409,
  body: {
    code: 'conflict',
    message: 'a selected item changed since its draft was started',
    detail: { kind: 'draft_conflict', items },
  },
});

const committed = (revision = MADE, warnings: Json[] = []): Reply =>
  ok({ revision, receipt_id: RECEIPT, warnings });

/**
 * A small service that remembers: which items have a draft of the person, and its head. Drafts
 * are held for the note and the plan unless `drafts` says otherwise.
 */
function service(overrides: Record<string, Handler> = {}, drafts: string[] = [NOTE, OTHER]) {
  const state = { head: HEAD, drafts: new Set(drafts) };
  const draftContent = (itemId: string) => ({
    body: itemId === NOTE ? DRAFT_BODY : '# Plan\n\nMy plan',
    properties: { type: itemId === NOTE ? 'meeting-note' : 'plan', okf_jawn: { id: itemId } },
    draft: draftOf(itemId, HEAD, LATER),
  });
  const api = mockApi(
    explorerHandlers({
      [OPEN]: () => ok({ ...writableWorkspace, head: state.head }),
      [ITEM]: (body) => {
        const itemId = String(body.item_id);
        const document = itemDocument(
          itemId === NOTE ? noteSummary : itemId === SOURCE ? sourceSummary : otherSummary,
        );
        return ok(
          state.drafts.has(itemId) ? { ...document, draft: draftContent(itemId) } : document,
        );
      },
      [DRAFTS]: () =>
        ok({ items: [...state.drafts].map((id) => draftOf(id, HEAD, LATER)), next_cursor: null }),
      [SAVE]: (body) => {
        state.drafts.add(String(body.item_id));
        return ok(draftOf(String(body.item_id), String(body.base_revision), LATER));
      },
      [DISCARD]: (body) => {
        state.drafts.delete(String(body.item_id));
        return ok(draftOf(String(body.item_id), HEAD, LATER));
      },
      [COMMIT]: (body) => {
        const ids = Array.isArray(body.item_ids) ? body.item_ids.map(String) : [];
        for (const id of ids) state.drafts.delete(id);
        state.head = MADE;
        return committed();
      },
      ...overrides,
    }),
  );
  return { api, state };
}

const pass = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

async function eventually(check: () => void) {
  for (let turn = 0; turn < 200; turn += 1) {
    try {
      check();
      return;
    } catch {
      await pass(0);
    }
  }
  check();
}

const panel = () => screen.getByRole('region', { name: /^Drafts/ });
const said = () => panel().querySelector('.snapshot-said')?.textContent ?? '';
const nameField = () => within(panel()).getByRole('textbox', { name: 'Snapshot name' });
const snapshotButton = () => within(panel()).getByRole('button', { name: /^(Snapshot \d|Taking)/ });
const headLine = () => document.querySelector('.head-line')?.textContent ?? '';
/** What the status adds when the text of the one snapshotted item went on meanwhile. */
const STILL_A_DRAFT = 'What you typed in it while it was taken is not in it, and is still a draft.';

/** Open the workspace with its two drafts listed, name the Snapshot, and press the button. */
async function pressSnapshot(name: string) {
  fireEvent.change(nameField(), { target: { value: name } });
  act(() => snapshotButton().focus());
  fireEvent.click(snapshotButton());
}

describe('reading a patch', () => {
  it('gives the lines of each hunk and leaves out Git’s header', () => {
    expect(readPatch(PATCH).hunks).toEqual([
      {
        oldStart: 1,
        newStart: 1,
        lines: [
          { kind: 'same', text: '---' },
          { kind: 'removed', text: 'owner: Ada' },
          { kind: 'added', text: 'owner: Grace' },
          { kind: 'same', text: 'type: meeting-note' },
          { kind: 'same', text: '---' },
        ],
      },
      {
        oldStart: 9,
        newStart: 9,
        lines: [
          { kind: 'same', text: 'The group met.' },
          { kind: 'same', text: '' },
          { kind: 'removed', text: 'It agreed nothing.' },
          { kind: 'added', text: 'It agreed to publish.' },
          { kind: 'added', text: 'The budget follows.' },
        ],
      },
    ]);
  });

  it('reads a hunk of one line, and nothing from an empty patch', () => {
    expect(readPatch('@@ -3 +3 @@\n-a\n+b\n').hunks).toEqual([
      {
        oldStart: 3,
        newStart: 3,
        lines: [
          { kind: 'removed', text: 'a' },
          { kind: 'added', text: 'b' },
        ],
      },
    ]);
    expect(readPatch('')).toEqual({ hunks: [], shown: 0, more: 0 });
  });

  it('keeps a line that begins with a sign of its own', () => {
    const [hunk] = readPatch('@@ -1,2 +1,2 @@\n-- a list entry\n++ plus\n -5 degrees\n').hunks;
    expect(hunk?.lines).toEqual([
      { kind: 'removed', text: '- a list entry' },
      { kind: 'added', text: '+ plus' },
      { kind: 'same', text: '-5 degrees' },
    ]);
  });
});

describe('the bounds of a diff', () => {
  const lines = (count: number, from = 0) =>
    Array.from({ length: count }, (_, index) => `+line ${from + index}`).join('\n');

  it('keeps exactly the lines there is room for and counts the rest, across hunks', () => {
    const patch = `@@ -1 +1,3 @@\n${lines(3)}\n@@ -9 +11,3 @@\n${lines(3, 3)}\n`;
    expect(readPatch(patch, 6)).toMatchObject({ shown: 6, more: 0 });
    const read = readPatch(patch, 4);
    expect(read).toMatchObject({ shown: 4, more: 2 });
    expect(read.hunks.map((hunk) => hunk.lines.length)).toEqual([3, 1]);
    // With no room left, a further hunk is not opened at all.
    expect(readPatch(patch, 3).hunks).toHaveLength(1);
    expect(readPatch(patch, 3)).toMatchObject({ shown: 3, more: 3 });
    expect(readPatch(patch).shown).toBe(6);
    expect(MAX_DIFF_LINES).toBe(2_000);
  });

  it('cuts a line longer than the bound, and no shorter one', () => {
    const read = readPatch(
      `@@ -1 +1,2 @@\n+${'x'.repeat(MAX_LINE_CHARS)}\n+${'y'.repeat(MAX_LINE_CHARS + 1)}\n`,
    );
    expect(read.hunks.at(0)?.lines).toEqual([
      { kind: 'added', text: 'x'.repeat(MAX_LINE_CHARS) },
      { kind: 'added', text: 'y'.repeat(MAX_LINE_CHARS), cut: true },
    ]);
  });

  it('shows no carriage return of a file with Windows line ends', () => {
    const read = readPatch('@@ -1,2 +1,2 @@\r\n same\r\n-old\r\n+new\r\n');
    expect(read.hunks.at(0)?.lines).toEqual([
      { kind: 'same', text: 'same' },
      { kind: 'removed', text: 'old' },
      { kind: 'added', text: 'new' },
    ]);
  });
});

describe('taking a Snapshot', () => {
  it('is not offered to a person who may only read', async () => {
    mockApi(
      explorerHandlers({
        [DRAFTS]: () => ok({ items: [draftOf(NOTE, HEAD, LATER)], next_cursor: null }),
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (1)' });
    expect(within(panel()).queryByRole('textbox', { name: 'Snapshot name' })).toBeNull();
    expect(within(panel()).queryByRole('checkbox')).toBeNull();
    expect(within(panel()).queryByRole('button', { name: /Snapshot/ })).toBeNull();
  });

  it('asks for a name before anything is sent', async () => {
    const { api } = service();
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    fireEvent.click(snapshotButton());
    expect(api.callsTo(COMMIT)).toHaveLength(0);
    expect(nameField().getAttribute('aria-invalid')).toBe('true');
    const described = nameField().getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(described)?.textContent).toContain('Give the Snapshot a name');
    expect(document.activeElement).toBe(nameField());
    // Spaces are not a name.
    fireEvent.change(nameField(), { target: { value: '   ' } });
    fireEvent.click(snapshotButton());
    expect(api.callsTo(COMMIT)).toHaveLength(0);
    fireEvent.change(nameField(), { target: { value: 'Tidy' } });
    expect(nameField().getAttribute('aria-invalid')).toBe('false');
  });

  it('commits the selected drafts, shows the revision it made without a Refresh, and says so', async () => {
    let answer: () => void = () => {};
    const { api, state } = service({
      [COMMIT]: () =>
        new Promise<Reply>((resolve) => {
          answer = () => {
            state.drafts.clear();
            state.head = MADE;
            resolve(committed());
          };
        }),
    });
    const { queryClient } = renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await findTextBox('Text of Meeting notes');
    expect(headLine()).toContain(HEAD.slice(0, 10));
    const opened = api.callsTo(OPEN).length;
    await pressSnapshot('  Tidy the notes ');
    await screen.findByRole('button', { name: 'Taking the Snapshot…' });
    expect(snapshotButton().getAttribute('aria-disabled')).toBe('true');
    expect(document.activeElement).toBe(snapshotButton());
    expect(said()).toBe('Taking the Snapshot…');
    // A second press asks nothing more.
    fireEvent.click(snapshotButton());
    expect(api.callsTo(COMMIT)).toHaveLength(1);
    expect(api.callsTo(COMMIT).at(0)).toMatchObject({
      workspace_id: WS,
      item_ids: [NOTE, OTHER],
      message: 'Tidy the notes',
    });
    expect(String(api.callsTo(COMMIT).at(0)?.idempotency_key)).toMatch(/^[0-9a-f-]{36}$/);

    act(() => answer());
    await screen.findByRole('region', { name: 'Drafts (0)' });
    expect(said()).toBe(
      `Snapshot “Tidy the notes” taken: 2 items. The workspace now shows revision ${MADE.slice(0, 10)}, which the Snapshot made.`,
    );
    // The shell shows the revision the commit made; the tree and the pane read it.
    expect(headLine()).toContain(MADE.slice(0, 10));
    expect(headLine()).not.toContain('A newer version');
    await screen.findByText('Text of Meeting notes');
    expect(api.callsTo(LIST).some((body) => JSON.stringify(body.at).includes(MADE))).toBe(true);
    expect(api.callsTo(ITEM).some((body) => JSON.stringify(body.at).includes(MADE))).toBe(true);
    // The checks against the latest head ran again; they found nothing newer.
    expect(api.callsTo(OPEN).length).toBeGreaterThan(opened);

    // The folders on screen (the root, and the open note's own) are listed at that revision.
    const listedAt = (folder: string) =>
      api
        .callsTo(LIST)
        .filter((body) => body.folder === folder)
        .map((body) => JSON.stringify(body.at));
    await eventuallyReal(() => {
      expect(listedAt('').at(-1)).toContain(MADE);
      expect(listedAt('notes').at(-1)).toContain(MADE);
    });
    // What the commit changed in the cache: the committed items have no draft any more.
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
    expect(
      queryClient.getQueryData(itemOptions(WS, NOTE, HEAD).queryKey)?.draft ?? null,
    ).toBeNull();
    // The form is ready for the next one, and the button never left.
    expect(nameField()).toHaveProperty('value', '');
    expect(document.activeElement).toBe(snapshotButton());
    expect(snapshotButton().textContent).toBe('Snapshot 0 drafts');
    expect(snapshotButton().getAttribute('aria-disabled')).toBe('true');
    expect(api.violations).toEqual([]);
  });

  // Keyboard focus never falls to the page body when a Snapshot takes away what had it.
  describe('focus when a Snapshot answers', () => {
    /** A Snapshot of every draft, held in flight until the test answers it. */
    function heldSnapshot(drafts: string[]) {
      let answer: () => void = () => {};
      const made = service(
        {
          [COMMIT]: (body) =>
            new Promise<Reply>((resolve) => {
              answer = () => {
                const ids = Array.isArray(body.item_ids) ? body.item_ids.map(String) : [];
                for (const id of ids) made.state.drafts.delete(id);
                made.state.head = MADE;
                resolve(committed());
              };
            }),
        },
        drafts,
      );
      return { ...made, answer: () => act(() => answer()) };
    }

    it('goes from the editor to the pane’s switch', async () => {
      const made = heldSnapshot([NOTE]);
      renderAt(`/w/${WS}/i/${NOTE}`);
      const box = await findTextBox('Text of Meeting notes');
      await screen.findByRole('region', { name: 'Drafts (1)' });
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(made.api.callsTo(COMMIT)).toHaveLength(1));
      // The person goes back into the text while the Snapshot is taken, and types nothing.
      act(() => box.focus());
      made.answer();
      await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken/));
      await eventuallyReal(() => expect(box.isConnected).toBe(false));
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit' }));
    });

    it.each([
      [
        'the row now in its place',
        [NOTE, OTHER, SOURCE],
        ['Meeting notes', 'Annual report'],
        'Annual report',
      ],
      ['the row before, when it was the last', [NOTE, OTHER], ['Meeting notes'], 'Meeting notes'],
    ] as const)('goes from a row that leaves to %s', async (_where, drafts, kept, to) => {
      // Only the plan is snapshotted; the others are unticked and stay listed.
      const made = heldSnapshot([...drafts]);
      renderAt(`/w/${WS}`);
      await screen.findByRole('region', { name: `Drafts (${drafts.length})` });
      const row = await within(panel()).findByRole('button', { name: 'Plan' });
      for (const name of kept) {
        fireEvent.click(
          await within(panel()).findByRole('checkbox', { name: `Include ${name} in the Snapshot` }),
        );
      }
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(made.api.callsTo(COMMIT)).toHaveLength(1));
      expect(made.api.callsTo(COMMIT).at(0)?.item_ids).toEqual([OTHER]);
      act(() => row.focus());
      made.answer();
      await eventuallyReal(() => expect(row.isConnected).toBe(false));
      // (The row's title is read again at the new head; the row itself is the same one.)
      expect(document.activeElement).toBe(await within(panel()).findByRole('button', { name: to }));
    });

    it('goes from the only row to the panel’s heading', async () => {
      const made = heldSnapshot([NOTE]);
      renderAt(`/w/${WS}`);
      await screen.findByRole('region', { name: 'Drafts (1)' });
      const row = await within(panel()).findByRole('button', { name: 'Meeting notes' });
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(made.api.callsTo(COMMIT)).toHaveLength(1));
      act(() => row.focus());
      made.answer();
      await eventuallyReal(() => expect(row.isConnected).toBe(false));
      expect(document.activeElement).toBe(within(panel()).getByRole('heading'));
    });
  });

  it('sends nothing when no draft is selected, though the dimmed button is pressed', async () => {
    const { api } = service();
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    for (const name of ['Meeting notes', 'Plan']) {
      fireEvent.click(
        await within(panel()).findByRole('checkbox', { name: `Include ${name} in the Snapshot` }),
      );
    }
    expect(snapshotButton().textContent).toBe('Snapshot 0 drafts');
    expect(snapshotButton().getAttribute('aria-disabled')).toBe('true');
    await pressSnapshot('Nothing');
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(api.callsTo(COMMIT)).toHaveLength(0);
    expect(said()).toBe('');
    expect(api.violations).toEqual([]);
  });

  it('leaves out a draft whose box is unticked', async () => {
    const { api } = service();
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    const box = await within(panel()).findByRole('checkbox', {
      name: 'Include Meeting notes in the Snapshot',
    });
    expect(box).toHaveProperty('checked', true);
    fireEvent.click(box);
    expect(snapshotButton().textContent).toBe('Snapshot 1 draft');
    await pressSnapshot('Only the plan');
    await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
    expect(api.callsTo(COMMIT).at(0)?.item_ids).toEqual([OTHER]);
  });

  it('saves the text on screen first, and asks nothing when it cannot be saved', async () => {
    let failing = true;
    const { api } = service(
      {
        [SAVE]: (body) =>
          failing
            ? fail(503, 'unavailable', 'busy')
            : ok(draftOf(String(body.item_id), HEAD, LATER)),
      },
      [NOTE],
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    const box = await findTextBox('Text of Meeting notes');
    vi.useFakeTimers();
    typeInto(box, ' and more');
    await pressSnapshot('Too early');
    await eventually(() => expect(within(panel()).getByRole('alert')).toBeDefined());
    expect(within(panel()).getByRole('alert').textContent).toContain('Nothing was snapshotted.');
    expect(within(panel()).getByRole('alert').textContent).toContain(
      '1 selected item has text that is not saved',
    );
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(api.callsTo(COMMIT)).toHaveLength(0);

    failing = false;
    fireEvent.click(snapshotButton());
    await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
    // The save went first: the Snapshot is of the text on screen.
    const order = api.calls
      .map((call) => call.path)
      .filter((path) => path === SAVE || path === COMMIT);
    expect(order).toEqual([SAVE, SAVE, COMMIT]);
    expect(api.callsTo(SAVE).at(-1)?.body).toBe(`${DRAFT_BODY} and more`);
  });

  it('never commits while a save’s outcome is unknown, even with the text put back', async () => {
    let lost = false;
    let holds = DRAFT_BODY;
    const { api, state } = service(
      {
        [SAVE]: (body) => {
          // The service applies the save whether or not its answer arrives.
          holds = String(body.body);
          return lost ? new Promise<never>(() => {}) : ok(draftOf(NOTE, HEAD, LATER));
        },
        [COMMIT]: () => {
          state.drafts.clear();
          state.head = MADE;
          return committed();
        },
      },
      [NOTE],
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    const box = await findTextBox('Text of Meeting notes');
    vi.useFakeTimers();
    lost = true;
    typeInto(box, ' WRONG TURN');
    await pass(AUTOSAVE_DELAY_MS);
    await pass(REQUEST_TIMEOUT_MS);
    expect(holds).toBe(`${DRAFT_BODY} WRONG TURN`);
    replaceText(box, DRAFT_BODY);
    // The Snapshot saves first; that save has no answer either, so nothing is committed.
    await pressSnapshot('Tidy');
    await pass(REQUEST_TIMEOUT_MS + 50);
    await eventually(() =>
      expect(within(panel()).getByRole('alert').textContent).toContain('Nothing was snapshotted.'),
    );
    expect(api.callsTo(COMMIT)).toHaveLength(0);

    // Once a save answers, the service holds what the editor shows, and that is committed.
    lost = false;
    fireEvent.click(snapshotButton());
    await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
    expect(holds).toBe(DRAFT_BODY);
    const order = api.calls
      .map((call) => call.path)
      .filter((path) => path === SAVE || path === COMMIT);
    expect(order.slice(-2)).toEqual([SAVE, COMMIT]);
  });

  it('keeps text typed while the Snapshot is taken, as a new draft on the revision it made', async () => {
    let answer: () => void = () => {};
    const { api, state } = service(
      {
        [COMMIT]: () =>
          new Promise<Reply>((resolve) => {
            answer = () => {
              state.drafts.clear();
              state.head = MADE;
              resolve(committed());
            };
          }),
      },
      [NOTE],
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    const box = await findTextBox('Text of Meeting notes');
    vi.useFakeTimers();
    await pressSnapshot('First part');
    await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
    typeInto(box, ' second part');
    act(() => answer());
    await eventually(() => expect(said()).toMatch(/^Snapshot “First part” taken/));
    expect(said()).toContain(STILL_A_DRAFT);
    expect(textIn(textBox('Text of Meeting notes'))).toBe(`${DRAFT_BODY} second part`);
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)).toMatchObject({
      base_revision: MADE,
      body: `${DRAFT_BODY} second part`,
    });
  });

  it('is not undone by a read of an item that began before it and answers after it', async () => {
    const read: { hold: boolean; answer?: () => void } = { hold: false };
    const { api, state } = service(
      {
        [ITEM]: (body) => {
          const itemId = String(body.item_id);
          const document = itemDocument(itemId === NOTE ? noteSummary : otherSummary);
          const reply = ok(
            state.drafts.has(itemId)
              ? {
                  ...document,
                  draft: {
                    body: DRAFT_BODY,
                    properties: document.properties,
                    draft: draftOf(itemId, HEAD, LATER),
                  },
                }
              : document,
          );
          if (itemId !== NOTE || !read.hold) return reply;
          read.hold = false;
          return new Promise<Reply>((resolve) => {
            read.answer = () => resolve(reply);
          });
        },
      },
      [NOTE],
    );
    // The tab cites an older revision on purpose, so the Snapshot leaves it where it is and its
    // read of the item stays on screen. (A tab that follows the head moves to the new revision;
    // the read it leaves has no reader then, and the library stops it by itself.) A draft is not
    // bound to a revision: that read carries it too.
    const { queryClient } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await screen.findByText('Text of Meeting notes');
    await screen.findByText(/You have a draft of this item\./);
    await screen.findByRole('region', { name: 'Drafts (1)' });
    const key = itemOptions(WS, NOTE, OLD).queryKey;
    const atOld = () =>
      api.callsTo(ITEM).filter((sent) => JSON.stringify(sent.at).includes(OLD)).length;
    read.hold = true;
    act(() => {
      void queryClient.invalidateQueries({ queryKey: key });
    });
    await eventuallyReal(() => expect(read.answer).toBeDefined());
    const before = atOld();
    await pressSnapshot('Tidy');
    await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 1 item\./));
    // The old answer arrives now, with the draft that was just committed in it.
    act(() => read.answer?.());
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    // The read was still watched when the answer came: only stopping it kept the draft out.
    expect(
      queryClient.getQueryCache().find({ queryKey: key })?.getObserversCount(),
    ).toBeGreaterThan(0);
    expect(queryClient.getQueryData(key)?.draft ?? null).toBeNull();
    expect(screen.queryByText(/You have a draft of this item\./)).toBeNull();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    // It was asked again after the Snapshot: that answer has the Snapshot in it.
    expect(atOld()).toBeGreaterThan(before);
    expect(api.callsTo(SAVE)).toHaveLength(0);
  });

  // The same, against a service that holds the draft as the real one does, in each order the
  // save and the commit can answer.
  describe('text that went on while the Snapshot was taken', () => {
    /** A digest that differs with the text, as the service's does. */
    const digestOf = (text: string) => text.length.toString(16).padStart(64, '0');

    /**
     * A service that holds one draft of the note as the real one does. `save_draft` replaces it;
     * a first save records the request's base, a later one moves the base only onto the head.
     * `commit_items` commits the draft it read when it started and removes the draft only if it
     * is still that content (crates/core/src/application/history.rs: "A draft saved again in
     * between ... stays in place"). With `holdSaves` a save is applied when the test answers it.
     */
    function remembering(holdSaves = false) {
      const held = {
        head: HEAD,
        committed: '# Meeting notes',
        draft: DRAFT_BODY as string | null,
        base: HEAD,
      };
      const properties = { type: 'meeting-note', okf_jawn: { id: NOTE } };
      const draftNow = () => ({
        ...draftOf(NOTE, held.base, LATER),
        content_digest: digestOf(held.draft ?? ''),
      });
      let commit: () => void = () => {};
      const saves: Array<() => void> = [];
      const { api } = service(
        {
          [OPEN]: () => ok({ ...writableWorkspace, head: held.head }),
          [ITEM]: (body) => {
            if (body.item_id !== NOTE) return ok(itemDocument(otherSummary));
            const document = { ...itemDocument(noteSummary), body: held.committed };
            return ok(
              held.draft === null
                ? document
                : { ...document, draft: { body: held.draft, properties, draft: draftNow() } },
            );
          },
          [DRAFTS]: () => ok({ items: held.draft === null ? [] : [draftNow()], next_cursor: null }),
          [SAVE]: (body) => {
            const apply = () => {
              const base = String(body.base_revision);
              if (held.draft === null || base === held.head) held.base = base;
              held.draft = String(body.body);
              return ok(draftNow());
            };
            if (!holdSaves) return apply();
            return new Promise<Reply>((resolve) => {
              saves.push(() => resolve(apply()));
            });
          },
          [COMMIT]: () => {
            const read = held.draft;
            return new Promise<Reply>((resolve) => {
              commit = () => {
                held.committed = read ?? held.committed;
                if (held.draft === read) held.draft = null;
                held.head = MADE;
                resolve(committed());
              };
            });
          },
        },
        [NOTE],
      );
      return {
        api,
        held,
        answerCommit: () => act(() => commit()),
        answerSave: () =>
          act(() => {
            saves.shift()?.();
          }),
      };
    }

    it('saved before the Snapshot answers: it stays on screen, and its draft is moved onto the revision the Snapshot made', async () => {
      const { api, held, answerCommit } = remembering();
      renderAt(`/w/${WS}/i/${NOTE}`);
      const box = await findTextBox('Text of Meeting notes');
      await screen.findByRole('region', { name: 'Drafts (1)' });
      vi.useFakeTimers();
      await pressSnapshot('First part');
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
      typeInto(box, ' second part');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      // Saved in between, on the draft's old base: the service keeps this draft at the commit.
      expect(api.callsTo(SAVE).at(0)).toMatchObject({ base_revision: HEAD });
      await pass(10);
      answerCommit();
      await eventually(() => expect(said()).toMatch(/^Snapshot “First part” taken: 1 item\./));
      expect(said()).toContain(STILL_A_DRAFT);
      // At once, not at the next keystroke: the draft the service holds is on the Snapshot's revision.
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      expect(api.callsTo(SAVE).at(1)).toMatchObject({
        base_revision: MADE,
        body: `${DRAFT_BODY} second part`,
      });
      await eventually(() =>
        expect(held).toEqual({
          head: MADE,
          committed: DRAFT_BODY,
          draft: `${DRAFT_BODY} second part`,
          base: MADE,
        }),
      );
      await pass(50);
      const boxes = screen.getAllByRole('textbox', { name: 'Text of Meeting notes' });
      expect(boxes.map((each) => textIn(each))).toEqual([`${DRAFT_BODY} second part`]);
      expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
      await eventually(() =>
        expect(screen.getByRole('region', { name: 'Drafts (1)' })).toBeDefined(),
      );
      // The next change is saved on that revision too.
      typeInto(textBox('Text of Meeting notes'), '!');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(3));
      expect(api.callsTo(SAVE).at(2)).toMatchObject({
        base_revision: MADE,
        body: `${DRAFT_BODY} second part!`,
      });
    });

    it('being saved when the Snapshot answers: once the save has answered, the draft is moved onto that revision', async () => {
      const { api, held, answerCommit, answerSave } = remembering(true);
      renderAt(`/w/${WS}/i/${NOTE}`);
      const box = await findTextBox('Text of Meeting notes');
      await screen.findByRole('region', { name: 'Drafts (1)' });
      vi.useFakeTimers();
      await pressSnapshot('First part');
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
      typeInto(box, ' second part');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      answerCommit();
      await eventually(() => expect(said()).toMatch(/^Snapshot “First part” taken: 1 item\./));
      expect(said()).toContain(STILL_A_DRAFT);
      // One save of the item at a time: the next waits for the one in flight.
      await pass(AUTOSAVE_DELAY_MS * 2);
      expect(api.callsTo(SAVE)).toHaveLength(1);
      // The save lands after the commit: a first save, which records the old base it carried.
      answerSave();
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      expect(held.base).toBe(HEAD);
      const [first, second] = api.callsTo(SAVE);
      expect(first).toMatchObject({ base_revision: HEAD, body: `${DRAFT_BODY} second part` });
      expect(second).toMatchObject({ base_revision: MADE, body: `${DRAFT_BODY} second part` });
      expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
      answerSave();
      await eventually(() => expect(held.base).toBe(MADE));
      await eventually(() =>
        expect(document.querySelector('.save-state')?.textContent).toMatch(/^Saved as a draft at/),
      );
      await pass(AUTOSAVE_DELAY_MS * 2);
      expect(api.callsTo(SAVE)).toHaveLength(2);
    });

    it('put back, while the Snapshot is taken, to what it was before the draft: that is saved, on what was committed', async () => {
      const { api, held, answerCommit } = remembering();
      renderAt(`/w/${WS}/i/${NOTE}`);
      const box = await findTextBox('Text of Meeting notes');
      await screen.findByRole('region', { name: 'Drafts (1)' });
      vi.useFakeTimers();
      await pressSnapshot('First part');
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
      // The draft is being committed; the person takes it all back.
      replaceText(box, '# Meeting notes');
      answerCommit();
      await eventually(() => expect(said()).toMatch(/^Snapshot “First part” taken: 1 item\./));
      // What was committed is the draft, so the text on screen differs from it and is a draft.
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(api.callsTo(SAVE).at(0)).toMatchObject({
        base_revision: MADE,
        body: '# Meeting notes',
      });
      await eventually(() =>
        expect(held).toMatchObject({ committed: DRAFT_BODY, draft: '# Meeting notes', base: MADE }),
      );
      await eventually(() =>
        expect(document.querySelector('.save-state')?.textContent).toMatch(/^Saved as a draft at/),
      );
    });

    it('an item opened while the Snapshot is taken, on the draft being committed, ends with it', async () => {
      const { api, held, answerCommit } = remembering();
      renderAt(`/w/${WS}`);
      await screen.findByRole('region', { name: 'Drafts (1)' });
      const row = await within(panel()).findByRole('button', { name: 'Meeting notes' });
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
      fireEvent.click(row);
      await findTextBox('Text of Meeting notes');
      answerCommit();
      await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 1 item\./));
      expect(said()).not.toContain(STILL_A_DRAFT);
      await screen.findByRole('region', { name: 'Drafts (0)' });
      await eventuallyReal(() =>
        expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull(),
      );
      expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
      // Nothing was typed: nothing is saved, and the service holds no draft.
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(held.draft).toBeNull();
    });
  });

  it('shows a failure in plain words and repeats the same request on "Try again"', async () => {
    let failing = true;
    const { api, state } = service({
      [COMMIT]: () => {
        if (failing) return fail(503, 'unavailable', 'the store is busy');
        state.drafts.clear();
        state.head = MADE;
        return committed();
      },
    });
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await pressSnapshot('Tidy');
    const retry = await within(panel()).findByRole('button', { name: 'Try again' });
    expect(
      within(panel()).getByText('Taking the Snapshot: The service is temporarily unavailable.'),
    ).toBeDefined();
    expect(said()).toBe('');
    expect(headLine()).toContain(HEAD.slice(0, 10));
    failing = false;
    fireEvent.click(retry);
    await screen.findByRole('region', { name: 'Drafts (0)' });
    const [first, second] = api.callsTo(COMMIT);
    expect(api.callsTo(COMMIT)).toHaveLength(2);
    expect(second).toEqual(first);
  });

  it('sends another selection or another name as a new request', async () => {
    const { api } = service({ [COMMIT]: () => fail(503, 'unavailable', 'busy') });
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await pressSnapshot('Tidy');
    await within(panel()).findByRole('button', { name: 'Try again' });
    // The button pressed again with nothing changed is the same request, like the retry.
    fireEvent.click(snapshotButton());
    await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(2));
    await within(panel()).findByRole('button', { name: 'Try again' });
    expect(api.callsTo(COMMIT).at(1)).toEqual(api.callsTo(COMMIT).at(0));
    fireEvent.change(nameField(), { target: { value: 'Tidy more' } });
    fireEvent.click(snapshotButton());
    await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(3));
    await within(panel()).findByRole('button', { name: 'Try again' });
    fireEvent.click(
      within(panel()).getByRole('checkbox', { name: 'Include Plan in the Snapshot' }),
    );
    fireEvent.click(snapshotButton());
    await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(4));
    const keys = api.callsTo(COMMIT).map((sent) => sent.idempotency_key);
    expect(new Set(keys).size).toBe(3);
  });

  // The service commits the drafts it holds: which they are is part of what the request says.
  describe('the key of a Snapshot', () => {
    /** A digest that differs with the text, as the service's does. */
    const digestOf = (text: string) => text.length.toString(16).padStart(64, '0');
    const keys = (api: ReturnType<typeof service>['api']) =>
      api.callsTo(COMMIT).map((sent) => String(sent.idempotency_key));

    it('is a new one when text was saved since a Snapshot that had no answer', async () => {
      let lost = true;
      const { api, state } = service(
        {
          [SAVE]: (body) =>
            ok({ ...draftOf(NOTE, HEAD, LATER), content_digest: digestOf(String(body.body)) }),
          [COMMIT]: () => {
            if (lost) return new Promise<never>(() => {});
            state.drafts.clear();
            state.head = MADE;
            return committed();
          },
        },
        [NOTE],
      );
      renderAt(`/w/${WS}/i/${NOTE}`);
      const box = await findTextBox('Text of Meeting notes');
      await screen.findByRole('region', { name: 'Drafts (1)' });
      vi.useFakeTimers();
      await pressSnapshot('Tidy');
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
      await pass(REQUEST_TIMEOUT_MS + 50);
      await eventually(() => within(panel()).getByRole('button', { name: 'Try again' }));
      // Tried again with nothing saved since: the same request under the same key.
      fireEvent.click(within(panel()).getByRole('button', { name: 'Try again' }));
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(2));
      await pass(REQUEST_TIMEOUT_MS + 50);
      await eventually(() => within(panel()).getByRole('button', { name: 'Try again' }));
      expect(api.callsTo(COMMIT).at(1)).toEqual(api.callsTo(COMMIT).at(0));
      // The person goes on typing; the text is saved. What a Snapshot commits is now another text.
      typeInto(box, ' and one more thought');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      lost = false;
      fireEvent.click(within(panel()).getByRole('button', { name: 'Try again' }));
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(3));
      const [first, second, third] = keys(api);
      expect(second).toBe(first);
      expect(third).not.toBe(first);
      expect(api.callsTo(COMMIT).at(2)).toMatchObject({ item_ids: [NOTE], message: 'Tidy' });
    });

    it('is a new one for the same drafts under the same name after a Snapshot that was taken', async () => {
      // The drafts are still listed afterwards: the person saved the same text again elsewhere.
      const { api, state } = service({
        [COMMIT]: () => {
          state.head = MADE;
          return committed();
        },
      });
      renderAt(`/w/${WS}`);
      await screen.findByRole('region', { name: 'Drafts (2)' });
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 2 items\./));
      await eventuallyReal(() => expect(snapshotButton().textContent).toBe('Snapshot 2 drafts'));
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(2));
      const [first, second] = api.callsTo(COMMIT);
      expect({ ...second, idempotency_key: null }).toEqual({ ...first, idempotency_key: null });
      expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
    });
  });

  it('reads "not available in this build" where Snapshot is not built, and adopts nothing', async () => {
    const { [COMMIT]: _unbuilt, ...handlers } = explorerHandlers({
      [OPEN]: () => ok(writableWorkspace),
      [DRAFTS]: () => ok({ items: [draftOf(NOTE, HEAD, LATER)], next_cursor: null }),
    });
    mockApi(handlers);
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (1)' });
    await pressSnapshot('Tidy');
    expect(
      await within(panel()).findByText('Taking the Snapshot: Not available in this build.'),
    ).toBeDefined();
    expect(within(panel()).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(said()).toBe('');
    expect(headLine()).toContain(HEAD.slice(0, 10));
    expect(screen.getByRole('region', { name: 'Drafts (1)' })).toBeDefined();
  });

  it('shows the warnings the service recorded', async () => {
    const { state } = service({
      [COMMIT]: () => {
        state.drafts.clear();
        state.head = MADE;
        return committed(MADE, [
          { code: 'okf_lint', message: 'notes/meeting.md has no description' },
        ]);
      },
    });
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await pressSnapshot('Tidy');
    expect(await within(panel()).findByText('notes/meeting.md has no description')).toBeDefined();
  });

  it('leaves a head someone else made after it to Refresh', async () => {
    const { state } = service({
      [COMMIT]: () => {
        state.drafts.clear();
        // Someone else's commit landed right after the person's.
        state.head = THEIRS;
        return committed();
      },
    });
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await pressSnapshot('Tidy');
    await screen.findByRole('region', { name: 'Drafts (0)' });
    await eventuallyReal(() =>
      expect(headLine()).toContain('A newer version of this workspace exists'),
    );
    // The shell shows the person's own revision; the other one is announced, not adopted.
    expect(headLine()).toContain(`revision ${MADE.slice(0, 10)}`);
    expect(headLine()).not.toContain(THEIRS.slice(0, 10));
  });

  it('moves a background tab that cited the old head as well', async () => {
    service();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    await findTextBox('Text of Meeting notes');
    act(() => {
      void router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
        search: { rev: HEAD },
      });
    });
    await findTextBox('Text of Plan');
    await pressSnapshot('Tidy');
    await screen.findByRole('region', { name: 'Drafts (0)' });
    await eventuallyReal(() => expect(router.state.location.search).toEqual({ rev: MADE }));
    fireEvent.click(screen.getByRole('tab', { name: 'Meeting notes' }));
    await eventuallyReal(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(router.state.location.search).toEqual({ rev: MADE });
  });

  // Only a tab that cited the head the Snapshot replaced follows it.
  it('leaves alone a tab that cites an older revision on purpose', async () => {
    service();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await screen.findByText('Text of Meeting notes');
    await pressSnapshot('Tidy');
    await screen.findByRole('region', { name: 'Drafts (0)' });
    await eventuallyReal(() => expect(headLine()).toContain(MADE.slice(0, 10)));
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`);
    expect(router.state.location.search).toEqual({ rev: OLD });
  });

  it('leaves alone a background tab that cites an older revision on purpose', async () => {
    service();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await screen.findByText('Text of Meeting notes');
    act(() => {
      void router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
      });
    });
    await findTextBox('Text of Plan');
    await pressSnapshot('Tidy');
    await screen.findByRole('region', { name: 'Drafts (0)' });
    await eventuallyReal(() => expect(headLine()).toContain(MADE.slice(0, 10)));
    fireEvent.click(screen.getByRole('tab', { name: 'Meeting notes' }));
    await eventuallyReal(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(router.state.location.search).toEqual({ rev: OLD });
  });

  it('moves a tab that cited the old head to the revision the Snapshot made', async () => {
    service();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await findTextBox('Text of Meeting notes');
    await pressSnapshot('Tidy');
    await screen.findByRole('region', { name: 'Drafts (0)' });
    await eventuallyReal(() => expect(router.state.location.search).toEqual({ rev: MADE }));
    // It reads what was just committed, and can be edited again.
    expect(await screen.findByRole('button', { name: 'Edit' })).toBeDefined();
  });
});

// This workspace edits only Notes as drafts, and the service refuses any other draft at a
// Snapshot. One made elsewhere is listed all the same: the person has to be able to see it go.
describe('a draft of a Source or a View made elsewhere', () => {
  const KINDS = [
    ['a Source', 'source', 'Source'],
    ['a View', 'view', 'View'],
  ] as const;

  /** The person has a draft of the note, and one of an item of another kind. */
  function withOther(kind: 'source' | 'view', overrides: Record<string, Handler> = {}) {
    const item = summary(SOURCE, 'things/it.md', 'Annual report', 'report', kind);
    const made = service(
      {
        [ITEM]: (body) => {
          const itemId = String(body.item_id);
          const document = itemDocument(
            itemId === NOTE ? noteSummary : itemId === SOURCE ? item : otherSummary,
          );
          return ok(
            made.state.drafts.has(itemId)
              ? {
                  ...document,
                  draft: {
                    body: 'a draft made elsewhere',
                    properties: document.properties,
                    draft: draftOf(itemId, HEAD, LATER),
                  },
                }
              : document,
          );
        },
        ...overrides,
      },
      [NOTE, SOURCE],
    );
    return made;
  }
  const row = () =>
    within(panel()).getByRole('button', { name: 'Annual report' }).closest('li')?.textContent ?? '';
  const box = () =>
    within(panel()).getByRole('checkbox', { name: 'Include Annual report in the Snapshot' });
  const discardButton = () =>
    within(panel()).getByRole('button', { name: 'Discard draft of Annual report' });

  async function listed(kind: 'source' | 'view', overrides: Record<string, Handler> = {}) {
    const made = withOther(kind, overrides);
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    // The row's item has been read: its kind is known.
    await eventuallyReal(() => expect(row()).toContain('cannot be snapshotted'));
    return made;
  }

  it.each(KINDS)(
    'of %s: is listed with its kind, cannot be ticked, says why, and a Snapshot leaves it out',
    async (_what, kind, word) => {
      const { api } = await listed(kind);
      expect(row()).toContain(`${word} · saved`);
      expect(row()).toContain(
        `A draft of a ${word} cannot be snapshotted: only a Note is edited as a draft. It can be discarded here.`,
      );
      expect(box()).toHaveProperty('checked', false);
      expect(box().getAttribute('aria-disabled')).toBe('true');
      fireEvent.click(box());
      expect(box()).toHaveProperty('checked', false);
      // The Note beside it is a draft like any other: ticked, and with no discard of its own here.
      expect(
        within(panel()).getByRole('checkbox', { name: 'Include Meeting notes in the Snapshot' }),
      ).toHaveProperty('checked', true);
      expect(within(panel()).getAllByRole('button', { name: /^Discard draft/ })).toHaveLength(1);
      expect(snapshotButton().textContent).toBe('Snapshot 1 draft');
      await pressSnapshot('Tidy');
      await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 1 item\./));
      expect(api.callsTo(COMMIT).map((sent) => sent.item_ids)).toEqual([[NOTE]]);
      // It is still there to be discarded.
      expect(await screen.findByRole('region', { name: 'Drafts (1)' })).toBeDefined();
      expect(row()).toContain('cannot be snapshotted');
      expect(api.violations).toEqual([]);
    },
  );

  it('offers "Discard draft", which asks first; then the draft is gone and focus is on a row', async () => {
    const { api, state } = await listed('source');
    act(() => discardButton().focus());
    fireEvent.click(discardButton());
    const question = within(panel()).getByRole('group', {
      name: 'Discard the draft of Annual report?',
    });
    const keep = within(question).getByRole('button', { name: 'Keep the draft' });
    expect(document.activeElement).toBe(keep);
    expect(api.callsTo(DISCARD)).toHaveLength(0);
    fireEvent.click(keep);
    expect(document.activeElement).toBe(discardButton());
    expect(api.callsTo(DISCARD)).toHaveLength(0);

    fireEvent.click(discardButton());
    const confirm = within(panel()).getByRole('button', { name: 'Discard the draft' });
    act(() => confirm.focus());
    fireEvent.click(confirm);
    await screen.findByRole('region', { name: 'Drafts (1)' });
    // This page had no session of the item: the service was asked.
    expect(api.callsTo(DISCARD)).toHaveLength(1);
    expect(api.callsTo(DISCARD).at(0)).toMatchObject({ workspace_id: WS, item_id: SOURCE });
    expect(state.drafts.has(SOURCE)).toBe(false);
    expect(within(panel()).queryByRole('button', { name: 'Annual report' })).toBeNull();
    expect(said()).toBe('Your draft of Annual report was discarded.');
    // It was the last row: focus goes to the one before it, not to the page.
    expect(document.activeElement).toBe(
      within(panel()).getByRole('button', { name: 'Meeting notes' }),
    );
    expect(api.violations).toEqual([]);
  });

  it('keeps the row when the discard fails, says so there, and repeats it under the same key', async () => {
    let failing = true;
    const { api, state } = await listed('view', {
      [DISCARD]: (body) => {
        if (failing) return fail(503, 'unavailable', 'busy');
        state.drafts.delete(String(body.item_id));
        return ok(draftOf(String(body.item_id), HEAD, LATER));
      },
    });
    fireEvent.click(discardButton());
    fireEvent.click(within(panel()).getByRole('button', { name: 'Discard the draft' }));
    expect(
      await within(panel()).findByText(
        'Discarding the draft of Annual report: The service is temporarily unavailable.',
      ),
    ).toBeDefined();
    expect(row()).toContain('cannot be snapshotted');
    failing = false;
    fireEvent.click(within(panel()).getByRole('button', { name: 'Try again' }));
    await screen.findByRole('region', { name: 'Drafts (1)' });
    const [first, second] = api.callsTo(DISCARD);
    expect(api.callsTo(DISCARD)).toHaveLength(2);
    expect(second).toEqual(first);
  });

  it('is listed with its reason for a person who may only read, with nothing to press', async () => {
    await listed('source', { [OPEN]: () => ok({ ...writableWorkspace, permissions: ['read'] }) });
    expect(row()).toContain(
      'A draft of a Source cannot be snapshotted: only a Note is edited as a draft.',
    );
    expect(row()).not.toContain('It can be discarded here.');
    expect(within(panel()).queryByRole('button', { name: /^Discard draft/ })).toBeNull();
    expect(within(panel()).queryByRole('checkbox')).toBeNull();
  });
});

describe('a Snapshot that conflicts', () => {
  /** The service answers a conflict for the note while its draft is based on the old head. */
  function withConflict(extra: Json = {}, overrides: Record<string, Handler> = {}) {
    const bases = new Map<string, string>();
    const made = service({
      [SAVE]: (body) => {
        bases.set(String(body.item_id), String(body.base_revision));
        return ok(draftOf(String(body.item_id), String(body.base_revision), LATER));
      },
      [COMMIT]: (body) => {
        const ids = Array.isArray(body.item_ids) ? body.item_ids.map(String) : [];
        if (ids.includes(NOTE) && bases.get(NOTE) !== THEIRS) {
          made.state.head = THEIRS;
          return conflict([conflictOf(NOTE, extra)]);
        }
        for (const id of ids) made.state.drafts.delete(id);
        made.state.head = MADE;
        return committed();
      },
      ...overrides,
    });
    return made;
  }

  it('snapshots nothing, says so, marks the item, and takes the others at one press', async () => {
    const { api } = withConflict();
    renderAt(`/w/${WS}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await pressSnapshot('Tidy');
    const alert = await within(panel()).findByRole('alert');
    expect(alert.textContent).toContain('Nothing was snapshotted.');
    expect(alert.textContent).toContain('1 of the 2 selected items changed since your draft');
    expect(alert.textContent).toContain(
      'The other 1 draft is still selected: press Snapshot to take it now.',
    );
    expect(said()).toBe('');
    expect(headLine()).toContain(HEAD.slice(0, 10));
    // A conflict means a newer head exists: the shell's own check says so; it does not move.
    await eventuallyReal(() =>
      expect(headLine()).toContain('A newer version of this workspace exists'),
    );
    const row = within(panel()).getByRole('button', { name: 'Meeting notes' }).closest('li');
    expect(row?.textContent).toContain('changed since your draft: open it to resolve');
    const box = within(panel()).getByRole('checkbox', {
      name: 'Include Meeting notes in the Snapshot',
    });
    expect(box).toHaveProperty('checked', false);
    expect(box.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(box);
    expect(box).toHaveProperty('checked', false);
    // The name is still there; one press takes the draft that did not conflict.
    expect(nameField()).toHaveProperty('value', 'Tidy');
    expect(snapshotButton().textContent).toBe('Snapshot 1 draft');
    fireEvent.click(snapshotButton());
    await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 1 item\./));
    const [first, second] = api.callsTo(COMMIT);
    expect(first?.item_ids).toEqual([NOTE, OTHER]);
    expect(second?.item_ids).toEqual([OTHER]);
    expect(second?.message).toBe('Tidy');
    expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
    // The conflicting draft is still there, still to be resolved.
    expect(await screen.findByRole('region', { name: 'Drafts (1)' })).toBeDefined();
    expect(api.violations).toEqual([]);
  });

  it('conflicts for a draft begun from a view that was already behind, and shows the Conflict flow', async () => {
    // The service as ruled: a first save records the base the request carries, so a draft begun
    // from a view that is behind is known to be behind. A later save moves the base only onto
    // the head.
    const held = { base: null as string | null };
    const made = service(
      {
        [SAVE]: (body) => {
          const carried = String(body.base_revision);
          if (held.base === null || carried === made.state.head) held.base = carried;
          made.state.drafts.add(NOTE);
          return ok(draftOf(NOTE, held.base, LATER));
        },
        [COMMIT]: () => {
          if (held.base !== made.state.head) {
            return conflict([
              conflictOf(NOTE, { draft_base: held.base, current_revision: made.state.head }),
            ]);
          }
          made.state.drafts.clear();
          made.state.head = MADE;
          return committed();
        },
      },
      [],
    );
    const { api } = made;
    renderAt(`/w/${WS}/i/${NOTE}`);
    const edit = await screen.findByRole('button', { name: 'Edit' });
    await screen.findByText('Text of Meeting notes');
    // Someone else changes the note: the service's head moves. The page shows the head it resolved.
    made.state.head = THEIRS;
    vi.useFakeTimers();
    fireEvent.click(edit);
    await eventually(() => textBox('Text of Meeting notes'));
    typeInto(textBox('Text of Meeting notes'), ' and mine');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)).toMatchObject({ base_revision: HEAD });
    await eventually(() =>
      expect(screen.getByRole('region', { name: 'Drafts (1)' })).toBeDefined(),
    );
    await pressSnapshot('Mine');
    await eventually(() =>
      expect(
        screen.getByRole('region', { name: 'Meeting notes changed since your draft was started' }),
      ).toBeDefined(),
    );
    const notice = screen.getByRole('region', {
      name: 'Meeting notes changed since your draft was started',
    });
    expect(notice.textContent).toContain(`Your draft started from revision ${HEAD.slice(0, 10)}.`);
    expect(notice.textContent).toContain(`The item is now at revision ${THEIRS.slice(0, 10)}.`);
    expect(within(panel()).getByRole('alert').textContent).toContain('Nothing was snapshotted.');
    expect(notice.querySelectorAll('ins').length).toBeGreaterThan(0);
    // Their change is not written over in silence: the person chooses.
    fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
    await eventually(() => expect(said()).toMatch(/^Snapshot “Mine” taken: 1 item\./));
    expect(api.callsTo(SAVE).at(-1)).toMatchObject({ base_revision: THEIRS });
    expect(held.base).toBe(THEIRS);
  });

  /** Open the note's pane with its conflict on screen. */
  async function openConflict(extra: Json = {}, overrides: Record<string, Handler> = {}) {
    const made = withConflict(extra, overrides);
    const view = renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    await findTextBox('Text of Meeting notes');
    fireEvent.click(
      within(panel()).getByRole('checkbox', { name: 'Include Plan in the Snapshot' }),
    );
    await pressSnapshot('Tidy');
    const notice = await screen.findByRole('region', {
      name: 'Meeting notes changed since your draft was started',
    });
    return { ...made, ...view, notice };
  }

  it('shows what changed underneath as a diff, without Git’s own header', async () => {
    const { notice } = await openConflict();
    expect(notice.textContent).toContain(`Your draft started from revision ${HEAD.slice(0, 10)}.`);
    expect(notice.textContent).toContain(`The item is now at revision ${THEIRS.slice(0, 10)}.`);
    expect(notice.textContent).toContain('The Snapshot “Tidy” did not include it.');
    expect(within(notice).getByRole('heading', { name: 'What changed underneath' })).toBeDefined();
    expect(notice.textContent).toContain('Changed: notes/meeting.md');
    const removed = [...notice.querySelectorAll('del')].map((line) => line.textContent);
    const added = [...notice.querySelectorAll('ins')].map((line) => line.textContent);
    expect(removed).toEqual(['Removed: − owner: Ada\n', 'Removed: − It agreed nothing.\n']);
    expect(added).toEqual([
      'Added: + owner: Grace\n',
      'Added: + It agreed to publish.\n',
      'Added: + The budget follows.\n',
    ]);
    expect(notice.textContent).toContain('From line 9 of the file');
    expect(notice.textContent).toContain('The group met.');
    expect(notice.textContent).not.toContain('diff --git');
    expect(notice.textContent).not.toContain('@@');
    expect(notice.textContent).not.toContain('No newline at end of file');
    // The person's own text is still in the editor, beside what changed.
    expect(textIn(textBox('Text of Meeting notes'))).toBe(DRAFT_BODY);
  });

  it('puts at most 2,000 lines of a diff on the page, counts the rest, and marks a cut line', async () => {
    const change = (path: string, patch: string) => ({
      kind: 'modified',
      binary: false,
      old_path: path,
      new_path: path,
      patch,
    });
    const added = (count: number) => Array.from({ length: count }, () => '+a line').join('\n');
    const { notice } = await openConflict({
      changes: [
        change(
          'notes/meeting.md',
          `@@ -1 +1,1500 @@\n+${'z'.repeat(MAX_LINE_CHARS + 5)}\n${added(1_499)}\n`,
        ),
        change('notes/meeting.appearance.md', `@@ -1 +1,505 @@\n${added(505)}\n`),
      ],
    });
    // The bound is over the whole conflict, not per file.
    expect(notice.querySelectorAll('ins')).toHaveLength(MAX_DIFF_LINES);
    expect(notice.textContent).toContain(
      'and 5 more lines: this change is too long to show whole.',
    );
    const first = notice.querySelector('ins')?.textContent ?? '';
    expect(first).toBe(`Added: + ${'z'.repeat(MAX_LINE_CHARS)} … (a long line, cut here)\n`);
  });

  it('says nothing of more lines when the whole diff is shown', async () => {
    const { notice } = await openConflict();
    expect(notice.textContent).not.toContain('more line');
  });

  it('keep mine: saves the draft on the current revision, then takes the Snapshot of it', async () => {
    const { api, notice } = await openConflict();
    const before = api.calls.length;
    const keep = within(notice).getByRole('button', { name: 'Keep mine' });
    act(() => keep.focus());
    fireEvent.click(keep);
    await eventuallyReal(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 1 item\./));
    const after = api.calls
      .slice(before)
      .filter((call) => call.path === SAVE || call.path === COMMIT);
    expect(after.map((call) => call.path)).toEqual([SAVE, COMMIT]);
    expect(after.at(0)?.body).toMatchObject({
      workspace_id: WS,
      item_id: NOTE,
      // The rebase: the base is the revision the item is at now, not the draft's old base.
      base_revision: THEIRS,
      body: DRAFT_BODY,
      properties: { type: 'meeting-note', okf_jawn: { id: NOTE } },
    });
    expect(after.at(1)?.body).toMatchObject({ item_ids: [NOTE], message: 'Tidy' });
    // The draft is on another revision now: its Snapshot is another request than any before it.
    const commitKeys = api.callsTo(COMMIT).map((sent) => sent.idempotency_key);
    expect(new Set(commitKeys).size).toBe(commitKeys.length);
    // The conflict is over: the pane reads what was committed, and focus is on its switch.
    expect(
      screen.queryByRole('region', { name: 'Meeting notes changed since your draft was started' }),
    ).toBeNull();
    await screen.findByText('Text of Meeting notes');
    expect(document.activeElement).toBe(await screen.findByRole('button', { name: 'Edit' }));
    expect(headLine()).toContain(MADE.slice(0, 10));
    expect(api.violations).toEqual([]);
  });

  it('take theirs: asks first, then discards the draft and commits nothing', async () => {
    const { api, notice } = await openConflict();
    const commits = api.callsTo(COMMIT).length;
    const theirs = within(notice).getByRole('button', { name: 'Take theirs' });
    act(() => theirs.focus());
    fireEvent.click(theirs);
    const question = within(notice).getByRole('group', {
      name: 'Discard your draft and take the newer version?',
    });
    const keepDraft = within(question).getByRole('button', { name: 'Keep my draft' });
    expect(document.activeElement).toBe(keepDraft);
    expect(api.callsTo(DISCARD)).toHaveLength(0);
    fireEvent.click(keepDraft);
    expect(document.activeElement).toBe(
      within(notice).getByRole('button', { name: 'Take theirs' }),
    );
    expect(api.callsTo(DISCARD)).toHaveLength(0);

    fireEvent.click(within(notice).getByRole('button', { name: 'Take theirs' }));
    fireEvent.click(within(notice).getByRole('button', { name: 'Discard my draft' }));
    await screen.findByText('Text of Meeting notes');
    expect(api.callsTo(DISCARD)).toHaveLength(1);
    expect(api.callsTo(DISCARD).at(0)).toMatchObject({ workspace_id: WS, item_id: NOTE });
    expect(api.callsTo(COMMIT)).toHaveLength(commits);
    expect(
      screen.queryByRole('region', { name: 'Meeting notes changed since your draft was started' }),
    ).toBeNull();
    expect(document.activeElement).toBe(await screen.findByRole('button', { name: 'Edit' }));
    expect(said()).toBe(
      'Your draft of Meeting notes was discarded. The newer version is shown once the workspace is refreshed.',
    );
    // Someone else's head still waits for Refresh.
    expect(headLine()).toContain(HEAD.slice(0, 10));
    expect(await screen.findByRole('tab', { name: 'Meeting notes' })).toBeDefined();
  });

  /** "Keep mine" pressed once, and the item had changed again: the new conflict is on screen. */
  async function changedAgain() {
    const AGAIN = 'f'.repeat(40);
    let conflicts = 0;
    const { api, notice } = await openConflict(
      {},
      {
        [COMMIT]: () => {
          conflicts += 1;
          return conflict([
            conflictOf(
              NOTE,
              conflicts === 1 ? {} : { draft_base: THEIRS, current_revision: AGAIN },
            ),
          ]);
        },
      },
    );
    fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
    await eventuallyReal(() =>
      expect(notice.textContent).toContain(`The item is now at revision ${AGAIN.slice(0, 10)}.`),
    );
    expect(notice.textContent).toContain(
      `Your draft started from revision ${THEIRS.slice(0, 10)}.`,
    );
    expect(within(notice).getByRole('status').textContent).toContain('changed again');
    expect(within(notice).queryByRole('alert')).toBeNull();
    expect(within(notice).getByRole('button', { name: 'Keep mine' })).toBeDefined();
    return { api, notice, AGAIN };
  }

  it('shows a new conflict when the item changed again while the draft was being kept', async () => {
    await changedAgain();
  });

  it('asks each Snapshot of a draft kept on another revision under a key of its own', async () => {
    const { api, notice, AGAIN } = await changedAgain();
    // The first Snapshot of this one item (the plan was left out), then one after each "Keep mine".
    fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
    await eventuallyReal(() => expect(api.callsTo(COMMIT)).toHaveLength(3));
    const commits = api.callsTo(COMMIT);
    expect(commits.map((sent) => sent.item_ids)).toEqual([[NOTE], [NOTE], [NOTE]]);
    expect(new Set(commits.map((sent) => sent.idempotency_key)).size).toBe(3);
    // Each "Keep mine" saved the draft on the revision the item was at then, as a request of its own.
    expect(api.callsTo(SAVE).map((sent) => sent.base_revision)).toEqual([THEIRS, AGAIN]);
    expect(new Set(api.callsTo(SAVE).map((sent) => sent.idempotency_key)).size).toBe(2);
  });

  // "Keep mine" saves through the session's own save: one request per item at a time, and a key
  // that belongs to exactly one text on one base.
  describe('keep mine and the session’s own save', () => {
    /** A digest that differs with the text, as the service's does. */
    const digestOf = (text: string) => text.length.toString(16).padStart(64, '0');
    const savedAs = (body: Json): Reply =>
      ok({
        ...draftOf(NOTE, String(body.base_revision), LATER),
        content_digest: digestOf(String(body.body)),
      });
    /** The Snapshot from the panel conflicts; each one after it does what `then` says. */
    const commits = (then: (nth: number) => Reply): Handler => {
      let nth = 0;
      return () => {
        nth += 1;
        return nth === 1 ? conflict([conflictOf(NOTE)]) : then(nth);
      };
    };
    /** No key was sent with two requests that differ. */
    const reusedKeys = (sent: Json[]) => {
      const byKey = new Map<string, Set<string>>();
      for (const call of sent) {
        const key = String(call.idempotency_key);
        const said = byKey.get(key) ?? new Set<string>();
        said.add(JSON.stringify({ ...call, idempotency_key: null }));
        byKey.set(key, said);
      }
      return [...byKey.entries()].filter(([, said]) => said.size > 1).map(([key]) => key);
    };

    it('tried again after more was typed, sends no key with two different requests', async () => {
      const { api, notice } = await openConflict(
        {},
        {
          [SAVE]: savedAs,
          [COMMIT]: commits((nth) => (nth === 2 ? fail(503, 'unavailable', 'busy') : committed())),
        },
      );
      fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
      await within(notice).findByRole('button', { name: 'Try again' });
      // The first half was applied: the draft is on the newer revision. The person goes on typing.
      vi.useFakeTimers();
      typeInto(textBox('Text of Meeting notes'), ' and one more thought');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      fireEvent.click(within(notice).getByRole('button', { name: 'Try again' }));
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(3));
      // The draft is already on that revision with that text: it is not sent a third time.
      expect(api.callsTo(SAVE).map((sent) => [sent.base_revision, sent.body])).toEqual([
        [THEIRS, DRAFT_BODY],
        [THEIRS, `${DRAFT_BODY} and one more thought`],
      ]);
      expect(reusedKeys(api.callsTo(SAVE))).toEqual([]);
      expect(new Set(api.callsTo(SAVE).map((sent) => sent.idempotency_key)).size).toBe(2);
      // The Snapshot tried again commits another text than the one that failed: another request.
      expect(new Set(api.callsTo(COMMIT).map((sent) => sent.idempotency_key)).size).toBe(3);
    });

    it('never puts two saves of the item in flight when text is typed while the draft is kept', async () => {
      let flying = 0;
      let most = 0;
      const answers: Array<() => void> = [];
      const { api, notice } = await openConflict(
        {},
        {
          [SAVE]: (body) => {
            flying += 1;
            most = Math.max(most, flying);
            return new Promise<Reply>((resolve) => {
              answers.push(() => {
                flying -= 1;
                resolve(savedAs(body));
              });
            });
          },
          [COMMIT]: commits(() => committed()),
        },
      );
      const answerOne = () =>
        act(() => {
          answers.shift()?.();
        });
      vi.useFakeTimers();
      fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      typeInto(textBox('Text of Meeting notes'), ' and more');
      // The autosave's moment passes while the first save has no answer: it waits its turn.
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(1);
      answerOne();
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      // Nothing is committed before the text on screen is saved.
      expect(api.callsTo(COMMIT)).toHaveLength(1);
      answerOne();
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(2));
      expect(most).toBe(1);
      expect(api.callsTo(SAVE).map((sent) => [sent.base_revision, sent.body])).toEqual([
        [THEIRS, DRAFT_BODY],
        [THEIRS, `${DRAFT_BODY} and more`],
      ]);
      expect(reusedKeys(api.callsTo(SAVE))).toEqual([]);
    });

    // Typed while the draft is being kept, and that save answers before the autosave's moment.
    describe('text typed while the draft is kept, inside the autosave delay', () => {
      async function typedWhileKept(second: (body: Json) => Reply) {
        const answers: Array<() => void> = [];
        let saves = 0;
        const made = await openConflict(
          {},
          {
            [SAVE]: (body) => {
              saves += 1;
              // The save that keeps the draft waits for the test; any after it answers at once.
              return saves === 1
                ? new Promise<Reply>((resolve) => {
                    answers.push(() => resolve(savedAs(body)));
                  })
                : second(body);
            },
            [COMMIT]: commits(() => committed()),
          },
        );
        vi.useFakeTimers();
        fireEvent.click(within(made.notice).getByRole('button', { name: 'Keep mine' }));
        await eventually(() => expect(made.api.callsTo(SAVE)).toHaveLength(1));
        typeInto(textBox('Text of Meeting notes'), ' and more');
        // The clock is not moved: the autosave of what was typed has not come due.
        act(() => {
          answers.shift()?.();
        });
        return made;
      }

      it('is saved before the Snapshot is taken, and nothing is called too long', async () => {
        const { api } = await typedWhileKept(savedAs);
        await eventually(() => expect(said()).toMatch(/^Snapshot “Tidy” taken: 1 item\./));
        expect(api.callsTo(SAVE).map((sent) => [sent.base_revision, sent.body])).toEqual([
          [THEIRS, DRAFT_BODY],
          [THEIRS, `${DRAFT_BODY} and more`],
        ]);
        // The Snapshot was asked once the text on screen was saved, and not before.
        expect(api.callsTo(COMMIT)).toHaveLength(2);
        expect(document.body.textContent).not.toContain('too long');
        expect(document.body.textContent).not.toContain('Your draft was not kept.');
        expect(reusedKeys(api.callsTo(SAVE))).toEqual([]);
      });

      it('says a save of it that fails as that save’s failure, and takes no Snapshot', async () => {
        const { api, notice } = await typedWhileKept(() => fail(503, 'unavailable', 'busy'));
        await eventually(() =>
          expect(
            within(notice).getByText('Keeping your draft: The service is temporarily unavailable.'),
          ).toBeDefined(),
        );
        expect(api.callsTo(SAVE)).toHaveLength(2);
        expect(api.callsTo(COMMIT)).toHaveLength(1);
        expect(notice.textContent).not.toContain('too long');
        expect(notice.textContent).not.toContain('could not be reached');
        expect(within(notice).getByRole('button', { name: 'Try again' })).toBeDefined();
      });
    });

    it('sends a save that failed on the old base under a new key on the new one', async () => {
      let failing = false;
      const { api, notice } = await openConflict(
        {},
        {
          [SAVE]: (body) => (failing ? fail(503, 'unavailable', 'busy') : savedAs(body)),
          [COMMIT]: commits(() => committed()),
        },
      );
      vi.useFakeTimers();
      failing = true;
      typeInto(textBox('Text of Meeting notes'), ' and more');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() =>
        expect(screen.getByText(/^Saving this draft: The service is temporarily/)).toBeDefined(),
      );
      failing = false;
      fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
      await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(2));
      const [failed, kept] = api.callsTo(SAVE);
      expect(api.callsTo(SAVE)).toHaveLength(2);
      expect(failed).toMatchObject({ base_revision: HEAD, body: `${DRAFT_BODY} and more` });
      expect(kept).toMatchObject({ base_revision: THEIRS, body: `${DRAFT_BODY} and more` });
      expect(kept?.idempotency_key).not.toBe(failed?.idempotency_key);
    });

    it('says why a draft whose text is not in the page was not kept, and asks nothing', async () => {
      // The list of drafts names the note, and the Snapshot of it conflicts; `get_item` carries
      // no draft of it (it was discarded elsewhere in between), so there is no text here to keep.
      const { api } = withConflict(
        {},
        { [ITEM]: (body) => ok(itemDocument(body.item_id === NOTE ? noteSummary : otherSummary)) },
      );
      renderAt(`/w/${WS}`);
      await screen.findByRole('region', { name: 'Drafts (2)' });
      await pressSnapshot('Tidy');
      await within(panel()).findByRole('alert');
      fireEvent.click(within(panel()).getByRole('button', { name: 'Meeting notes' }));
      const notice = await screen.findByRole('region', {
        name: 'Meeting notes changed since your draft was started',
      });
      await screen.findByText('Text of Meeting notes');
      expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
      const before = api.calls.length;
      fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
      const alert = await within(notice).findByRole('alert');
      expect(alert.textContent).toContain('Your draft was not kept.');
      expect(alert.textContent).toContain('This page has no draft of this item to keep');
      expect(alert.textContent).not.toContain('editor below');
      const asked = api.calls.slice(before).map((call) => call.path);
      expect(asked.filter((path) => path === SAVE || path === COMMIT)).toEqual([]);
      expect(within(notice).getByRole('button', { name: 'Take theirs' })).toBeDefined();
    });

    it('take theirs still asks the service to discard a draft whose text is not in the page', async () => {
      // As above: the draft is listed and conflicts, and this page has no session of it. Whether
      // the service holds it is not this page's to know, so the discard is asked.
      const { api, state } = withConflict(
        {},
        { [ITEM]: (body) => ok(itemDocument(body.item_id === NOTE ? noteSummary : otherSummary)) },
      );
      renderAt(`/w/${WS}`);
      await screen.findByRole('region', { name: 'Drafts (2)' });
      await pressSnapshot('Tidy');
      await within(panel()).findByRole('alert');
      fireEvent.click(within(panel()).getByRole('button', { name: 'Meeting notes' }));
      const notice = await screen.findByRole('region', {
        name: 'Meeting notes changed since your draft was started',
      });
      fireEvent.click(within(notice).getByRole('button', { name: 'Take theirs' }));
      fireEvent.click(within(notice).getByRole('button', { name: 'Discard my draft' }));
      await eventuallyReal(() =>
        expect(
          screen.queryByRole('region', {
            name: 'Meeting notes changed since your draft was started',
          }),
        ).toBeNull(),
      );
      expect(api.callsTo(DISCARD)).toHaveLength(1);
      expect(api.callsTo(DISCARD).at(0)).toMatchObject({ workspace_id: WS, item_id: NOTE });
      // The service no longer holds it, and the list says so.
      expect(state.drafts.has(NOTE)).toBe(false);
      expect(await screen.findByRole('region', { name: 'Drafts (1)' })).toBeDefined();
      expect(within(panel()).queryByRole('button', { name: 'Meeting notes' })).toBeNull();
    });

    it('says why a draft too long to save was not kept, and asks nothing', async () => {
      const { api, notice } = await openConflict();
      vi.useFakeTimers();
      replaceText(textBox('Text of Meeting notes'), 'x'.repeat(MAX_BODY_BYTES + 1));
      const before = api.calls.length;
      fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
      await eventually(() => expect(within(notice).getByRole('alert')).toBeDefined());
      const alert = within(notice).getByRole('alert').textContent ?? '';
      expect(alert).toContain('Your draft was not kept.');
      expect(alert).toContain('Its text is too long to save');
      const asked = api.calls.slice(before).map((call) => call.path);
      expect(asked.filter((path) => path === SAVE || path === COMMIT)).toEqual([]);
      // The conflict is still there to resolve.
      expect(within(notice).getByRole('button', { name: 'Keep mine' })).toBeDefined();
    });
  });

  it('offers only "take theirs" for an item that was deleted', async () => {
    const { notice } = await openConflict({ deleted: true, changes: [] });
    expect(notice.textContent).toContain(
      `The item was deleted by revision ${THEIRS.slice(0, 10)}.`,
    );
    expect(notice.textContent).toContain('A draft of a deleted item cannot be snapshotted.');
    expect(within(notice).queryByRole('button', { name: 'Keep mine' })).toBeNull();
    expect(within(notice).getByRole('button', { name: 'Take theirs' })).toBeDefined();
    expect(textIn(textBox('Text of Meeting notes'))).toBe(DRAFT_BODY);
  });

  it('keeps the draft when taking theirs fails, and repeats the discard under the same key', async () => {
    let failing = true;
    const { api, notice } = await openConflict(
      {},
      {
        [DISCARD]: (body) =>
          failing
            ? fail(503, 'unavailable', 'busy')
            : ok(draftOf(String(body.item_id), HEAD, LATER)),
      },
    );
    fireEvent.click(within(notice).getByRole('button', { name: 'Take theirs' }));
    fireEvent.click(within(notice).getByRole('button', { name: 'Discard my draft' }));
    const retry = await within(notice).findByRole('button', { name: 'Try again' });
    expect(
      within(notice).getByText('Discarding your draft: The service is temporarily unavailable.'),
    ).toBeDefined();
    expect(textIn(textBox('Text of Meeting notes'))).toBe(DRAFT_BODY);
    failing = false;
    fireEvent.click(retry);
    await screen.findByText('Text of Meeting notes');
    const [first, second] = api.callsTo(DISCARD);
    expect(api.callsTo(DISCARD)).toHaveLength(2);
    expect(second).toEqual(first);
  });

  it('keeps the conflict on screen when keeping fails, and repeats the same requests', async () => {
    let failing = true;
    const { api, notice } = await openConflict(
      {},
      {
        [SAVE]: (body) =>
          failing
            ? fail(503, 'unavailable', 'busy')
            : ok(draftOf(String(body.item_id), String(body.base_revision), LATER)),
      },
    );
    const before = api.callsTo(SAVE).length;
    fireEvent.click(within(notice).getByRole('button', { name: 'Keep mine' }));
    const retry = await within(notice).findByRole('button', { name: 'Try again' });
    expect(
      within(notice).getByText('Keeping your draft: The service is temporarily unavailable.'),
    ).toBeDefined();
    expect(api.callsTo(COMMIT)).toHaveLength(1);
    failing = false;
    fireEvent.click(retry);
    await eventuallyReal(() => expect(api.callsTo(SAVE).length).toBe(before + 2));
    const [first, second] = api.callsTo(SAVE).slice(before);
    expect(second).toEqual(first);
  });
});

/** Under the real clock: wait until `check` holds. */
async function eventuallyReal(check: () => void) {
  for (let turn = 0; turn < 200; turn += 1) {
    try {
      check();
      return;
    } catch {
      await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
    }
  }
  check();
}
