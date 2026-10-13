/**
 * Snapshot: `commit_items` for the drafts the person selects, under a name; the shell adopting the
 * revision it made; and the per-item Conflict flow (what changed underneath as a diff, keep mine,
 * take theirs).
 */
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { REQUEST_TIMEOUT_MS } from '../../src/app/api-client';
import { itemOptions } from '../../src/features/documents/DocumentView';
import { readPatch } from '../../src/features/drafts/patch';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { fail, type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import { findTextBox, replaceText, textBox, textIn, typeInto } from './support/editor';
import {
  draftOf,
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  OTHER,
  otherSummary,
  RECEIPT,
  renderAt,
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
        const document = itemDocument(itemId === NOTE ? noteSummary : otherSummary);
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

/** Open the workspace with its two drafts listed, name the Snapshot, and press the button. */
async function pressSnapshot(name: string) {
  fireEvent.change(nameField(), { target: { value: name } });
  act(() => snapshotButton().focus());
  fireEvent.click(snapshotButton());
}

describe('reading a patch', () => {
  it('gives the lines of each hunk and leaves out Git’s header', () => {
    expect(readPatch(PATCH)).toEqual([
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
    expect(readPatch('@@ -3 +3 @@\n-a\n+b\n')).toEqual([
      {
        oldStart: 3,
        newStart: 3,
        lines: [
          { kind: 'removed', text: 'a' },
          { kind: 'added', text: 'b' },
        ],
      },
    ]);
    expect(readPatch('')).toEqual([]);
  });

  it('keeps a line that begins with a sign of its own', () => {
    const [hunk] = readPatch('@@ -1,2 +1,2 @@\n-- a list entry\n++ plus\n -5 degrees\n');
    expect(hunk?.lines).toEqual([
      { kind: 'removed', text: '- a list entry' },
      { kind: 'added', text: '+ plus' },
      { kind: 'same', text: '-5 degrees' },
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
      `Snapshot “Tidy the notes” taken: 2 items. The workspace now shows revision ${MADE.slice(0, 10)}, which includes your change.`,
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
    expect(textIn(textBox('Text of Meeting notes'))).toBe(`${DRAFT_BODY} second part`);
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)).toMatchObject({
      base_revision: MADE,
      body: `${DRAFT_BODY} second part`,
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

  it('shows a new conflict when the item changed again while the draft was being kept', async () => {
    const AGAIN = 'f'.repeat(40);
    let conflicts = 0;
    const { notice } = await openConflict(
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
