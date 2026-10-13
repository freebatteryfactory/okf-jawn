/**
 * Editing an item's text as a draft: the editor in place of the reader, autosave through
 * `save_draft` with the base revision, the save state always said, typed text never lost, the
 * person's draft shown when an item is reopened, the drafts listed and marked, and discarding.
 */
import { QueryObserver } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { REQUEST_TIMEOUT_MS, UPLOAD_BYTES_PER_SECOND } from '../../src/app/api-client';
import { createQueryClient } from '../../src/app/query-client';
import { itemOptions } from '../../src/features/documents/DocumentView';
import { draftsOptions, rememberDraft } from '../../src/features/drafts/drafts-list';
import {
  AUTOSAVE_DELAY_MS,
  DraftNotKept,
  MAX_BODY_BYTES,
  rebaseSession,
  resetDraftSessions,
  STILL_SENDING_AFTER_MS,
} from '../../src/features/drafts/saving';
import { sessionKey, useDraftSessions } from '../../src/features/drafts/sessions';
import { fail, type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import { findTextBox, replaceText, textBox, textIn, typeInto } from './support/editor';
import {
  draftOf,
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  NOW,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  readResponse,
  renderAt,
  SOURCE,
  summary,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const SAVE = '/api/items/save-draft';
const DISCARD = '/api/items/discard-draft';
const DRAFTS = '/api/items/list-drafts';
const ITEM = '/api/items/get-item';
const COMMITTED = '# Meeting notes';
const LATER = '2026-01-02T08:15:00.000Z';

afterEach(() => {
  // The sessions go first, so the page's last act is not a save sent to a service that is gone.
  act(() => resetDraftSessions());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** What `save_draft` answers for a request: the draft it now holds. */
const saved = (body: Json, base?: string): Reply =>
  ok(draftOf(String(body.item_id), base ?? String(body.base_revision), LATER));

/** The Explorer for a person who may write, with drafts saved as asked. */
function writable(overrides: Record<string, Handler> = {}) {
  return mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      [SAVE]: (body) => saved(body),
      ...overrides,
    }),
  );
}

/** Let answers and renders finish, moving the test's clock on by `ms`. */
const pass = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

/** Wait, without moving the clock, until `check` holds. */
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

/** Open the note, press "Edit", and hand the clock to the test. */
async function startEditing(url = `/w/${WS}/i/${NOTE}`) {
  const view = renderAt(url);
  const edit = await screen.findByRole('button', { name: 'Edit' });
  await screen.findByText('Text of Meeting notes');
  vi.useFakeTimers();
  fireEvent.click(edit);
  await eventually(() => textBox('Text of Meeting notes'));
  return { ...view, box: textBox('Text of Meeting notes') };
}

const saveState = () => document.querySelector('.save-state')?.textContent ?? '';

describe('who may edit, and where', () => {
  it('offers no editing to a person who may only read', async () => {
    mockApi(explorerHandlers({ '/api/workspaces/open-workspace': () => ok(workspace) }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    expect(screen.queryByRole('button', { name: /Edit/ })).toBeNull();
    expect(screen.queryByText(/Editing starts from/)).toBeNull();
  });

  it('puts the editor in place of the reading, with the committed text and focus in it', async () => {
    const api = writable();
    const { box } = await startEditing();
    expect(textIn(box)).toBe(COMMITTED);
    expect(document.activeElement).toBe(box);
    expect(box.getAttribute('aria-multiline')).toBe('true');
    // The reading is gone: the committed text is not shown beside the draft.
    expect(screen.queryByText('Text of Meeting notes')).toBeNull();
    expect(screen.getByRole('region', { name: 'Draft of Meeting notes' })).toBeDefined();
    expect(screen.getByText('Draft.')).toBeDefined();
    expect(saveState()).toBe('No changes to save.');
    // The text box is described by how its save stands.
    const describedBy = box.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(describedBy)?.getAttribute('role')).toBe('status');
    expect(api.callsTo(SAVE)).toHaveLength(0);
    // Nothing typed is not a draft: no mark, and nothing listed.
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    expect(screen.getByRole('region', { name: 'Drafts (0)' })).toBeDefined();
  });

  it('starts a draft only from the version the workspace shows, and offers that version', async () => {
    writable();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await screen.findByText('Text of Meeting notes');
    expect(screen.queryByRole('button', { name: 'Edit' })).toBeNull();
    expect(screen.getByText(/Editing starts from the version this workspace shows/)).toBeDefined();
    fireEvent.click(screen.getByRole('button', { name: 'Open the version this workspace shows' }));
    await screen.findByRole('button', { name: 'Edit' });
    expect(router.state.location.search).toEqual({});
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`);
  });

  // A Source's text is changed through corrections and a View has its own editor.
  it.each([
    ['a Source', 'source', 'Annual report'],
    ['a View', 'view', 'Quarterly'],
  ] as const)('offers no editing for %s, even one that has a draft', async (_what, kind, title) => {
    const item = summary(SOURCE, 'things/it.md', title, 'thing', kind);
    const api = writable({
      [ITEM]: () =>
        ok({
          ...itemDocument(item),
          draft: {
            body: 'a draft made elsewhere',
            properties: { type: 'thing', okf_jawn: { id: SOURCE } },
            draft: draftOf(SOURCE, HEAD, LATER),
          },
        }),
      '/api/reads/read-item': () => ok(readResponse(item, `# ${title}\n\nCommitted text`)),
    });
    renderAt(`/w/${WS}/i/${SOURCE}`);
    await screen.findByText('Committed text');
    await screen.findByRole('tab', { name: `${title} (draft)` });
    expect(screen.queryByRole('button', { name: /Edit/ })).toBeNull();
    expect(screen.queryByRole('textbox', { name: `Text of ${title}` })).toBeNull();
    expect(screen.queryByText(/Editing starts from/)).toBeNull();
    expect(api.callsTo(SAVE)).toHaveLength(0);
  });

  // The line that leads to editing is for Notes too: a pane of a Source or a View that reads
  // another revision is not offered a way to an editor it does not have.
  it.each([
    ['a Source', 'source', 'Annual report'],
    ['a View', 'view', 'Quarterly'],
  ] as const)(
    'says nothing of editing for %s that is read at another revision',
    async (_what, kind, title) => {
      const item = summary(SOURCE, 'things/it.md', title, 'thing', kind);
      writable({
        [ITEM]: () => ok(itemDocument(item)),
        '/api/reads/read-item': () => ok(readResponse(item, `# ${title}\n\nCommitted text`)),
      });
      renderAt(`/w/${WS}/i/${SOURCE}?rev=${OLD}`);
      await screen.findByText('Committed text');
      // `get_item` has answered: the header says what kind of item this is.
      await screen.findByText(new RegExp(`${kind === 'source' ? 'Source' : 'View'} · type`));
      expect(screen.queryByText(/Editing starts from/)).toBeNull();
      expect(
        screen.queryByRole('button', { name: 'Open the version this workspace shows' }),
      ).toBeNull();
      expect(screen.queryByRole('button', { name: /Edit/ })).toBeNull();
    },
  );

  it('lets a tab that cites the shown version be edited', async () => {
    writable();
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    expect(await screen.findByRole('button', { name: 'Edit' })).toBeDefined();
  });

  // Nothing says an item's kind stays what it was: it is what the last read of the item answered.
  describe('an item whose kind is another at the next revision', () => {
    const NEWER = 'd'.repeat(40);
    const asSource = summary(NOTE, 'notes/meeting.md', 'Meeting notes', 'meeting-note', 'source');
    const asView = summary(NOTE, 'notes/meeting.md', 'Meeting notes', 'meeting-note', 'view');

    /**
     * The note at the head shown; at the newer head the same item is `as` (a Source, unless the
     * test says otherwise); at any other revision it is a Note.
     */
    function becomesSource(as = asSource, overrides: Record<string, Handler> = {}) {
      const held = { head: HEAD };
      const api = writable({
        '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: held.head }),
        [ITEM]: (body) =>
          ok(
            itemDocument(
              body.item_id !== NOTE
                ? otherSummary
                : JSON.stringify(body.at).includes(NEWER)
                  ? as
                  : noteSummary,
            ),
          ),
        ...overrides,
      });
      return { api, held };
    }

    /** Press Refresh once the workspace's head is `revision`, and let the item be read there. */
    async function refreshTo(held: { head: string }, revision: string) {
      held.head = revision;
      fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
      await eventually(() =>
        expect(
          screen.getByText(`Refreshed: now showing revision ${revision.slice(0, 10)}.`),
        ).toBeDefined(),
      );
      await pass(500);
    }

    const keptText = () => screen.getByLabelText('Your text, read only');

    /**
     * The editor is open, " and more" was typed in it and waits for its autosave, and then a read
     * says the item is `as`: the session is refused, and the kept text is on screen.
     */
    async function refused(as = asSource, overrides: Record<string, Handler> = {}) {
      const made = becomesSource(as, overrides);
      const view = await startEditing();
      act(() => view.box.focus());
      typeInto(view.box, ' and more');
      await pass(100);
      await refreshTo(made.held, NEWER);
      const kept = await eventuallyGet(keptText);
      return { ...made, ...view, kept };
    }

    /**
     * A bound on a loop that never yields. A loop that asks a refused session again and again
     * (`save` answers at once, with nothing sent) runs in microtasks, which no timer interrupts:
     * the test's time limit would never fire and the runner would hang. Every turn of such a loop
     * reads the sessions, so from here on the reads are counted, and past the bound a read throws.
     */
    function spinGuard(limit = 2_000) {
      const read = useDraftSessions.getState;
      let reads = 0;
      const spy = vi.spyOn(useDraftSessions, 'getState').mockImplementation(() => {
        reads += 1;
        if (reads > limit) throw new Error('a loop is spinning on the session');
        return read();
      });
      return { spun: () => reads > limit, release: () => spy.mockRestore() };
    }

    const session = () => useDraftSessions.getState().sessions[sessionKey(WS, NOTE)];

    it('stops offering "Edit" once a read says it is a Source, and focus on the switch goes to the page’s heading', async () => {
      const { held } = becomesSource();
      renderAt(`/w/${WS}/i/${NOTE}`);
      const edit = await screen.findByRole('button', { name: 'Edit' });
      await screen.findByText('Text of Meeting notes');
      act(() => edit.focus());
      held.head = NEWER;
      fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
      await screen.findByText(`Refreshed: now showing revision ${NEWER.slice(0, 10)}.`);
      await screen.findByText(/Source · type/);
      expect(screen.queryByRole('button', { name: /Edit/ })).toBeNull();
      expect(edit.isConnected).toBe(false);
      // The page's heading, by its name: once the item's text is read at the new revision the
      // reader shows the document's own level-one heading as well.
      expect(document.activeElement).toBe(
        screen.getByRole('heading', { level: 1, name: 'Workshop notes' }),
      );
      expect(document.activeElement).not.toBe(document.body);
    });

    it('keeps the switch, with focus, while the item is read again', async () => {
      const read: { hold: boolean; answer?: () => void } = { hold: false };
      const held = { head: HEAD };
      writable({
        '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: held.head }),
        [ITEM]: (body) => {
          const reply = ok(itemDocument(body.item_id === NOTE ? noteSummary : otherSummary));
          if (body.item_id !== NOTE || !read.hold) return reply;
          return new Promise<Reply>((resolve) => {
            read.answer = () => resolve(reply);
          });
        },
      });
      renderAt(`/w/${WS}/i/${NOTE}`);
      const edit = await screen.findByRole('button', { name: 'Edit' });
      await screen.findByText('Text of Meeting notes');
      act(() => edit.focus());
      held.head = NEWER;
      read.hold = true;
      fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
      await screen.findByText(`Refreshed: now showing revision ${NEWER.slice(0, 10)}.`);
      await waitFor(() => expect(read.answer).toBeDefined());
      // The read at the new revision has no answer yet: what the last one said stands.
      expect(edit.isConnected).toBe(true);
      expect(document.activeElement).toBe(edit);
      expect(edit.getAttribute('aria-disabled')).toBe('true');
      act(() => read.answer?.());
      await waitFor(() => expect(edit.getAttribute('aria-disabled')).toBe('false'));
      expect(document.activeElement).toBe(edit);
    });

    // Whatever is not a Note is refused: a Source and a View alike.
    it.each([
      ['a Source', asSource, 'Source'],
      ['a View', asView, 'View'],
    ] as const)(
      'an open editor of what is now %s stops saving, says why in a line, and keeps the typed text read-only on screen',
      async (_what, as, word) => {
        // A change still waiting for its autosave when the read says what the item is now.
        const { api, kept } = await refused(as);
        expect(kept).toHaveProperty('value', `${COMMITTED} and more`);
        expect(kept).toHaveProperty('readOnly', true);
        expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
        expect(
          screen.getByText(
            `This item is now a ${word}, which is not edited as a draft, so your text is no longer saved. It is kept here: copy what you need, then discard the draft.`,
          ),
        ).toBeDefined();
        // Focus that was in the editor goes to the kept text, not to the page body.
        expect(document.activeElement).toBe(kept);
        // Nothing is sent, not by the waiting autosave and not later.
        await pass(AUTOSAVE_DELAY_MS * 15);
        expect(api.callsTo(SAVE)).toHaveLength(0);
        expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
        // Nor when the kept text is left for the committed text, where the pane says it too.
        fireEvent.click(screen.getByRole('button', { name: 'Read the committed text' }));
        await eventually(() =>
          expect(
            screen.getByText(
              `You have unsaved changes to this item. Below is the committed text. This item is now a ${word}, so your draft of it is no longer saved.`,
            ),
          ).toBeDefined(),
        );
        await pass(AUTOSAVE_DELAY_MS * 15);
        expect(api.callsTo(SAVE)).toHaveLength(0);
        // The switch leads back to the kept text, and says so: there is nothing to edit.
        expect(screen.queryByRole('button', { name: /^Edit/ })).toBeNull();
        fireEvent.click(screen.getByRole('button', { name: 'Show your draft' }));
        await eventually(() => expect(keptText()).toHaveProperty('value', `${COMMITTED} and more`));
      },
      20_000,
    );

    it('ends a session in which nothing was typed instead of refusing it: nothing is kept, and "Edit" starts afresh once it is a Note again', async () => {
      const { api, held } = becomesSource();
      const { box } = await startEditing();
      act(() => box.focus());
      await refreshTo(held, NEWER);
      await eventually(() => expect(screen.getByText(/Source · type/)).toBeDefined());
      expect(screen.queryByLabelText('Your text, read only')).toBeNull();
      expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
      expect(session()).toBeUndefined();
      // Focus that was in the editor is on the page's heading, not on the page body.
      expect(document.activeElement).toBe(
        screen.getByRole('heading', { level: 1, name: 'Workshop notes' }),
      );
      expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
      // A Note again: the pane reads the committed text, and offers to edit it.
      await refreshTo(held, 'f'.repeat(40));
      await eventually(() => expect(screen.getByText('Text of Meeting notes')).toBeDefined());
      expect(screen.getByRole('button', { name: 'Edit' })).toBeDefined();
      expect(api.callsTo(SAVE)).toHaveLength(0);
    }, 20_000);

    describe('the kept text offers "Discard draft"', () => {
      const answered = (body: Json): Reply => ok(draftOf(String(body.item_id), HEAD, LATER));

      it('asks as the editor asks, then asks the service, and the pane shows the item as it is now with focus on the page’s heading', async () => {
        // The text was saved before the item was read as a Source: the service holds a draft.
        const { api, held } = becomesSource(asSource, { [DISCARD]: answered });
        const { box } = await startEditing();
        typeInto(box, ' and more');
        await pass(AUTOSAVE_DELAY_MS);
        await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
        await refreshTo(held, NEWER);
        const kept = await eventuallyGet(keptText);
        const region = screen.getByRole('region', { name: 'Draft of Meeting notes, not saved' });
        const discard = within(region).getByRole('button', { name: 'Discard draft' });
        fireEvent.click(discard);
        const question = within(region).getByRole('group', { name: 'Discard this draft?' });
        expect(question.textContent).toContain(
          'Your text here will be removed. The committed text stays as it is.',
        );
        const keep = within(question).getByRole('button', { name: 'Keep the draft' });
        await eventually(() => expect(document.activeElement).toBe(keep));
        fireEvent.click(keep);
        await eventually(() =>
          expect(document.activeElement).toBe(
            within(region).getByRole('button', { name: 'Discard draft' }),
          ),
        );
        expect(api.callsTo(DISCARD)).toHaveLength(0);
        expect(kept.isConnected).toBe(true);
        fireEvent.click(within(region).getByRole('button', { name: 'Discard draft' }));
        const confirm = within(region).getByRole('button', { name: 'Discard the draft' });
        act(() => confirm.focus());
        fireEvent.click(confirm);
        await eventually(() => expect(screen.queryByLabelText('Your text, read only')).toBeNull());
        // A discard is the one request a refused session still sends.
        expect(api.callsTo(DISCARD)).toHaveLength(1);
        expect(api.callsTo(DISCARD).at(0)).toMatchObject({ workspace_id: WS, item_id: NOTE });
        expect(api.callsTo(SAVE)).toHaveLength(1);
        expect(session()).toBeUndefined();
        expect(screen.queryByRole('button', { name: /Edit|Show your draft/ })).toBeNull();
        expect(document.activeElement).toBe(
          screen.getByRole('heading', { level: 1, name: 'Workshop notes' }),
        );
        expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
        expect(api.violations).toEqual([]);
      }, 20_000);

      it('keeps the text when the discard fails, and says so with its retry', async () => {
        const { api, held } = becomesSource(asSource, {
          [DISCARD]: () => fail(503, 'unavailable', 'busy'),
        });
        const { box } = await startEditing();
        typeInto(box, ' and more');
        await pass(AUTOSAVE_DELAY_MS);
        await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
        await refreshTo(held, NEWER);
        const kept = await eventuallyGet(keptText);
        fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
        fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
        await eventually(() =>
          expect(
            screen.getByText('Discarding this draft: The service is temporarily unavailable.'),
          ).toBeDefined(),
        );
        expect(kept.isConnected).toBe(true);
        expect(kept).toHaveProperty('value', `${COMMITTED} and more`);
        expect(screen.getByRole('button', { name: 'Try again' })).toBeDefined();
        expect(api.callsTo(DISCARD).length).toBeGreaterThan(0);
      }, 20_000);
    });

    // The third re-check's probe: a Snapshot's conflict open on an item that is then refused.
    // "Keep mine" saves until the text is saved, and nothing of a refused session is saved.
    const conflictOfNote = {
      item_id: NOTE,
      draft_base: HEAD,
      current_revision: NEWER,
      deleted: false,
      changes: [],
    };

    it('offers no "Keep mine" and no "Take theirs" while it is refused: the conflict notice gives way to the kept text', async () => {
      const { api, kept } = await refused();
      act(() => useDraftSessions.getState().setConflicts(WS, 'Tidy', [conflictOfNote]));
      await pass(0);
      expect(screen.queryByRole('button', { name: 'Keep mine' })).toBeNull();
      expect(screen.queryByRole('button', { name: 'Take theirs' })).toBeNull();
      expect(
        screen.queryByRole('heading', { name: /changed since your draft was started/ }),
      ).toBeNull();
      expect(kept.isConnected).toBe(true);
      await pass(3_000);
      expect(api.callsTo(SAVE)).toHaveLength(0);
    }, 20_000);

    it('ends "Keep mine" at once for a refused session, as not kept: nothing is sent and no base is named', async () => {
      const { api, queryClient } = await refused();
      const guard = spinGuard();
      try {
        const outcome = await rebaseSession(queryClient, WS, NOTE, NEWER).then(
          () => 'kept',
          (thrown: unknown) => thrown,
        );
        expect(outcome).toBeInstanceOf(DraftNotKept);
        expect(outcome).toHaveProperty('reason', 'refused');
        expect(guard.spun()).toBe(false);
      } finally {
        guard.release();
      }
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(session()?.baseRevision).toBe(HEAD);
    }, 20_000);

    it('ends a "Keep mine" that was waiting for a save when the refusal came, as not kept', async () => {
      const answers: Array<() => void> = [];
      const { api, held } = becomesSource(asSource, {
        [SAVE]: (body) => new Promise<Reply>((resolve) => answers.push(() => resolve(saved(body)))),
      });
      const { box, queryClient } = await startEditing();
      typeInto(box, ' and more');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      // "Keep mine" waits for the save in flight; the item is read as a Source meanwhile.
      let outcome: unknown = 'waiting';
      void rebaseSession(queryClient, WS, NOTE, NEWER).then(
        () => {
          outcome = 'kept';
        },
        (thrown: unknown) => {
          outcome = thrown;
        },
      );
      await refreshTo(held, NEWER);
      await eventuallyGet(keptText);
      expect(outcome).toBe('waiting');
      const guard = spinGuard();
      try {
        act(() => answers.at(0)?.());
        await eventually(() => expect(outcome).not.toBe('waiting'));
        expect(guard.spun()).toBe(false);
      } finally {
        guard.release();
      }
      expect(outcome).toBeInstanceOf(DraftNotKept);
      expect(outcome).toHaveProperty('reason', 'refused');
      // The save that was under way answered; nothing was sent after it.
      expect(api.callsTo(SAVE)).toHaveLength(1);
    }, 20_000);

    describe('when a later read says it is a Note again', () => {
      /** A head after the newer one, where the item is a Note once more. */
      const AGAIN = 'f'.repeat(40);

      it('lifts the refusal: the editor is back with the text and focus, the save owed is sent, and the next change is saved', async () => {
        const { api, held, kept } = await refused();
        expect(document.activeElement).toBe(kept);
        expect(api.callsTo(SAVE)).toHaveLength(0);
        await refreshTo(held, AGAIN);
        const box = await eventuallyGet(() => textBox('Text of Meeting notes'));
        expect(textIn(box)).toBe(`${COMMITTED} and more`);
        expect(screen.queryByLabelText('Your text, read only')).toBeNull();
        expect(screen.queryByText(/This item is now a Source/)).toBeNull();
        // Focus that was in the kept text is in the text box, not on the page body.
        await eventually(() => expect(document.activeElement).toBe(box));
        // The save the refusal held up is sent as after a change, with nothing typed since.
        await pass(AUTOSAVE_DELAY_MS);
        await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
        expect(api.callsTo(SAVE).at(0)?.body).toBe(`${COMMITTED} and more`);
        await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
        typeInto(box, ' again');
        await pass(AUTOSAVE_DELAY_MS);
        await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
        expect(api.callsTo(SAVE).at(1)?.body).toBe(`${COMMITTED} and more again`);
        expect(api.violations).toEqual([]);
      }, 20_000);

      it('does not send by itself a save that had failed before the refusal: its notice is back with the retry', async () => {
        const made = becomesSource(asSource, { [SAVE]: () => fail(503, 'unavailable', 'busy') });
        const { box } = await startEditing();
        typeInto(box, ' and more');
        await pass(AUTOSAVE_DELAY_MS);
        await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
        await refreshTo(made.held, NEWER);
        await eventuallyGet(keptText);
        await refreshTo(made.held, AGAIN);
        await eventuallyGet(() => textBox('Text of Meeting notes'));
        expect(saveState()).toBe('Not saved. Your text is kept here.');
        await pass(AUTOSAVE_DELAY_MS * 12);
        expect(made.api.callsTo(SAVE)).toHaveLength(1);
        // The person's own retry sends it.
        fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
        await eventually(() => expect(made.api.callsTo(SAVE)).toHaveLength(2));
      }, 20_000);

      it('draws the conflict notice again: the conflict stayed with the item', async () => {
        const { held } = await refused();
        act(() => useDraftSessions.getState().setConflicts(WS, 'Tidy', [conflictOfNote]));
        await pass(0);
        expect(screen.queryByRole('button', { name: 'Keep mine' })).toBeNull();
        await refreshTo(held, AGAIN);
        await eventuallyGet(() => screen.getByRole('button', { name: 'Keep mine' }));
        expect(screen.getByRole('button', { name: 'Take theirs' })).toBeDefined();
      }, 20_000);
    });
  });
});

describe('autosave', () => {
  it('saves a moment after the last change, not before, carrying the base revision', async () => {
    const api = writable();
    const { box } = await startEditing();
    typeInto(box, '\n\nFirst');
    expect(saveState()).toBe('Unsaved changes.');
    await pass(AUTOSAVE_DELAY_MS - 1);
    typeInto(box, ' and second');
    await pass(AUTOSAVE_DELAY_MS - 1);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    await pass(1);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    const sent = api.callsTo(SAVE).at(0);
    expect(sent).toMatchObject({
      workspace_id: WS,
      item_id: NOTE,
      base_revision: HEAD,
      body: `${COMMITTED}\n\nFirst and second`,
      // The complete property map, as `get_item` answered it.
      properties: { type: 'meeting-note', okf_jawn: { id: NOTE }, owner: 'Ada' },
    });
    expect(String(sent?.idempotency_key)).toMatch(/^[0-9a-f-]{36}$/);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at .+\.$/));
    expect(document.querySelector('.save-state time')?.getAttribute('datetime')).toBe(LATER);
    expect(api.violations).toEqual([]);
  });

  it('saves at the latest ten seconds after the first unsaved change, however fast the typing', async () => {
    const api = writable();
    const { box } = await startEditing();
    // A key every half second: the one-second pause never comes.
    for (let typed = 0; typed < 20; typed += 1) {
      typeInto(box, 'a');
      await pass(typed < 19 ? 500 : 499);
    }
    // 9,999 ms after the first change: nothing sent yet.
    expect(api.callsTo(SAVE)).toHaveLength(0);
    await pass(1);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(api.callsTo(SAVE).at(0)?.body).toBe(`${COMMITTED}${'a'.repeat(20)}`);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    // The next change starts its own wait: one second after it, not ten after the first.
    typeInto(box, 'b');
    await pass(AUTOSAVE_DELAY_MS - 1);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    await pass(1);
    expect(api.callsTo(SAVE)).toHaveLength(2);
  });

  it('says "Saving…" while the save is in flight', async () => {
    let answer: (reply: Reply) => void = () => {};
    const api = writable({
      [SAVE]: (body) =>
        new Promise<Reply>((resolve) => {
          answer = () => resolve(saved(body));
        }),
    });
    const { box } = await startEditing();
    typeInto(box, ' more');
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(saveState()).toBe('Saving…');
    answer(ok(null));
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
  });

  it('saves one request at a time: a change made meanwhile is saved when the first settles', async () => {
    const answers: Array<() => void> = [];
    const api = writable({
      [SAVE]: (body) => new Promise<Reply>((resolve) => answers.push(() => resolve(saved(body)))),
    });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    typeInto(box, ' two');
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(saveState()).toBe('Saving…');
    answers.at(0)?.();
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
    const [first, second] = api.callsTo(SAVE);
    expect(first?.body).toBe(`${COMMITTED} one`);
    expect(second?.body).toBe(`${COMMITTED} one two`);
    expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
    answers.at(1)?.();
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(api.violations).toEqual([]);
  });

  it('keeps the base its first save carried for later saves, never the head on screen', async () => {
    const NEWER = 'd'.repeat(40);
    // The service as ruled: a first save records the base the request carries; a later one keeps
    // the draft's base unless it carries the current head.
    const held = { head: HEAD, base: null as string | null };
    const api = writable({
      '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: held.head }),
      [SAVE]: (body) => {
        const carried = String(body.base_revision);
        if (held.base === null || carried === held.head) held.base = carried;
        return saved(body, held.base);
      },
    });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    // Someone else makes a newer head and the person refreshes: the pane now shows that one.
    held.head = NEWER;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    // The clock is the test's: the answer of the Refresh needs it to move.
    await pass(50);
    await eventually(() =>
      expect(
        screen.getByText(`Refreshed: now showing revision ${NEWER.slice(0, 10)}.`),
      ).toBeDefined(),
    );
    await eventually(() => textBox('Text of Meeting notes'));
    typeInto(textBox('Text of Meeting notes'), ' two');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
    // The draft is still on the revision it was begun from: a save on the head would be a rebase.
    expect(api.callsTo(SAVE).map((sent) => sent.base_revision)).toEqual([HEAD, HEAD]);
    expect(held.base).toBe(HEAD);
    expect(screen.getByText('Draft.').parentElement?.textContent).toContain(HEAD.slice(0, 7));
  });

  it('tells the cache what the service now holds, and lists the drafts again only for a new one', async () => {
    const api = writable();
    const { box, queryClient } = await startEditing();
    const cached = () => queryClient.getQueryData(itemOptions(WS, NOTE, HEAD).queryKey)?.draft;
    expect(cached() ?? null).toBeNull();
    expect(api.callsTo(DRAFTS)).toHaveLength(1);
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(cached()?.body).toBe(`${COMMITTED} one`));
    expect(cached()?.draft).toEqual(draftOf(NOTE, HEAD, LATER));
    // A draft the list did not have: it is listed again.
    await eventually(() => expect(api.callsTo(DRAFTS)).toHaveLength(2));
  });

  it('tells the cache about the item that was saved, and no other', async () => {
    const api = writable();
    // Two tabs: the plan, then the note. Each tab keeps its item's `get_item` in the cache.
    const { queryClient, router } = renderAt(`/w/${WS}/i/${OTHER}`);
    await screen.findByText('Text of Plan');
    act(() => {
      void router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
      });
    });
    // The note's own switch: the plan's is gone once the note's text is on screen.
    await screen.findByText('Text of Meeting notes');
    const edit = await screen.findByRole('button', { name: 'Edit' });
    vi.useFakeTimers();
    fireEvent.click(edit);
    await eventually(() => textBox('Text of Meeting notes'));
    typeInto(textBox('Text of Meeting notes'), ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    const cachedDraft = (itemId: string) =>
      queryClient.getQueryData(itemOptions(WS, itemId, HEAD).queryKey)?.draft ?? null;
    await eventually(() => expect(cachedDraft(NOTE)?.body).toBe(`${COMMITTED} one`));
    expect(queryClient.getQueryData(itemOptions(WS, OTHER, HEAD).queryKey)).toBeDefined();
    expect(cachedDraft(OTHER)).toBeNull();
    // The other item's tab is not marked, and it opens on its own committed text.
    expect(screen.getByRole('tab', { name: 'Plan' })).toBeDefined();
    expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
  });

  it('takes a later save of a listed draft into the list in place', async () => {
    let savedAt = NOW;
    const api = writable({
      [SAVE]: (body) => ok(draftOf(String(body.item_id), HEAD, savedAt)),
      [DRAFTS]: () => ok({ items: [draftOf(NOTE, HEAD, NOW)], next_cursor: null }),
    });
    const { box, queryClient } = await startEditing();
    const listed = () =>
      queryClient.getQueryData(draftsOptions(WS).queryKey)?.pages.flatMap((page) => page.items);
    expect(listed()).toEqual([draftOf(NOTE, HEAD, NOW)]);
    savedAt = LATER;
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(listed()).toEqual([draftOf(NOTE, HEAD, LATER)]));
    expect(api.callsTo(DRAFTS)).toHaveLength(1);
  });

  it('does not save text that is back to what it was', async () => {
    const api = writable();
    const { box } = await startEditing();
    typeInto(box, ' more');
    replaceText(box, COMMITTED);
    await pass(AUTOSAVE_DELAY_MS * 3);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    expect(saveState()).toBe('No changes to save.');
  });
});

describe('a save that fails', () => {
  it('keeps the text, says it is not saved, and repeats the same request on "Try again"', async () => {
    let failing = true;
    const api = writable({
      [SAVE]: (body) => (failing ? fail(503, 'unavailable', 'the store is busy') : saved(body)),
    });
    const { box } = await startEditing();
    typeInto(box, ' precious words');
    await pass(AUTOSAVE_DELAY_MS);
    const retry = await screenButton('Try again');
    expect(saveState()).toBe('Not saved. Your text is kept here.');
    expect(saveState()).not.toMatch(/Saved as a draft/);
    expect(
      screen.getByText('Saving this draft: The service is temporarily unavailable.'),
    ).toBeDefined();
    expect(textIn(box)).toBe(`${COMMITTED} precious words`);
    expect(screen.getByRole('tab', { name: 'Meeting notes (not saved)' })).toBeDefined();

    failing = false;
    act(() => retry.focus());
    fireEvent.click(retry);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    const [first, second] = api.callsTo(SAVE);
    expect(api.callsTo(SAVE)).toHaveLength(2);
    // The same request under the same key: a save that did land is not applied twice.
    expect(second).toEqual(first);
    expect(screen.queryByRole('button', { name: /Try again|Trying again/ })).toBeNull();
    expect(document.activeElement).toBe(box);
    expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
  });

  it('sends changed text as a new request under a new key', async () => {
    let failing = true;
    const api = writable({
      [SAVE]: (body) => (failing ? fail(503, 'unavailable', 'busy') : saved(body)),
    });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await screenButton('Try again');
    failing = false;
    typeInto(box, ' two');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    const [first, second] = api.callsTo(SAVE);
    expect(second?.body).toBe(`${COMMITTED} one two`);
    expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
    expect(api.violations).toEqual([]);
  });

  it('moves focus to the text box when the next change’s own save cures it', async () => {
    let failing = true;
    writable({ [SAVE]: (body) => (failing ? fail(503, 'unavailable', 'busy') : saved(body)) });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    const retry = await screenButton('Try again');
    failing = false;
    typeInto(box, ' two');
    act(() => retry.focus());
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(retry.isConnected).toBe(false);
    expect(document.activeElement).toBe(box);
  });

  it('never leaves "Saving…" for a save that does not answer', async () => {
    writable({ [SAVE]: () => new Promise<never>(() => {}) });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    expect(saveState()).toBe('Saving…');
    await pass(REQUEST_TIMEOUT_MS - 1);
    expect(saveState()).toBe('Saving…');
    await pass(1);
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    expect(
      screen.getByText('Saving this draft: The workspace service did not answer in time.'),
    ).toBeDefined();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeDefined();
    expect(textIn(box)).toBe(`${COMMITTED} one`);
  });

  it('reads "not available in this build" where saving is not built, never as saved', async () => {
    const { [SAVE]: _unbuilt, ...handlers } = explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
    });
    mockApi(handlers);
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() =>
      expect(screen.getByText('Saving this draft: Not available in this build.')).toBeDefined(),
    );
    expect(saveState()).toBe('Not saved. Your text is kept here.');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(textIn(box)).toBe(`${COMMITTED} one`);
  });
});

// A save that failed or timed out may have been applied: what the service holds is then unknown.
describe('a save with no answer is not forgotten', () => {
  it('saves the text again when it is put back, and is never called clean before that answers', async () => {
    let holds: string | null = null;
    let mode: 'lost' | 'held' | 'ok' = 'lost';
    const answers: Array<() => void> = [];
    const api = writable({
      [SAVE]: (body) => {
        // The service applies the save whether or not its answer arrives.
        holds = String(body.body);
        if (mode === 'lost') return new Promise<never>(() => {});
        if (mode === 'ok') return saved(body);
        return new Promise<Reply>((resolve) => answers.push(() => resolve(saved(body))));
      },
    });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await pass(REQUEST_TIMEOUT_MS);
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    expect(holds).toBe(`${COMMITTED} one`);

    mode = 'held';
    replaceText(box, COMMITTED);
    // The text is what it was, and it is still not called clean: the service may hold " one".
    expect(saveState()).toBe('Not saved. Your text is kept here.');
    expect(screen.getByRole('tab', { name: 'Meeting notes (not saved)' })).toBeDefined();
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE)).toHaveLength(2);
    expect(api.callsTo(SAVE).at(1)?.body).toBe(COMMITTED);
    expect(saveState()).toBe('Saving…');
    answers.at(0)?.();
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(holds).toBe(textIn(box));
  });

  it('saves the text again when the save in flight fails after the text was put back', async () => {
    const answers: Array<(reply: Reply) => void> = [];
    const api = writable({
      [SAVE]: () => new Promise<Reply>((resolve) => answers.push(resolve)),
    });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    replaceText(box, COMMITTED);
    await pass(AUTOSAVE_DELAY_MS);
    answers.at(0)?.(fail(503, 'unavailable', 'the store is busy'));
    // It failed, so it may have been applied: the text on screen is saved, not taken as clean.
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
    expect(api.callsTo(SAVE).at(1)?.body).toBe(COMMITTED);
    expect(saveState()).not.toBe('No changes to save.');
    answers.at(1)?.(ok(draftOf(NOTE, HEAD, LATER)));
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
  });

  it('keeps the failure through a text that is too long to send', async () => {
    let failing = true;
    const api = writable({
      [SAVE]: (body) => (failing ? fail(503, 'unavailable', 'busy') : saved(body)),
    });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    replaceText(box, 'x'.repeat(MAX_BODY_BYTES + 1));
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('Not saved: the text is too long.'));
    failing = false;
    replaceText(box, COMMITTED);
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(api.callsTo(SAVE).at(-1)?.body).toBe(COMMITTED);
  });
});

describe('a text too long to save', () => {
  it('sends a body of exactly the bound, and never one byte more', async () => {
    const api = writable();
    const { box } = await startEditing();
    replaceText(box, 'x'.repeat(MAX_BODY_BYTES));
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(api.callsTo(SAVE)).toHaveLength(1);
    typeInto(box, 'x');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('Not saved: the text is too long.'));
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(screen.getByRole('alert').textContent).toContain('This text is too long to save.');
    expect(screen.getByRole('alert').textContent).toContain('Nothing was sent.');
    expect(screen.getByRole('tab', { name: 'Meeting notes (not saved)' })).toBeDefined();
    expect(api.violations).toEqual([]);
  });

  it('is no longer called too long once the text is back to what is saved', async () => {
    const api = writable();
    const { box } = await startEditing();
    replaceText(box, 'x'.repeat(MAX_BODY_BYTES + 1));
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('Not saved: the text is too long.'));
    // Pasted too long and undone: the text is what the service has, and there is nothing to send.
    replaceText(box, COMMITTED);
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('No changes to save.'));
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    expect(api.callsTo(SAVE)).toHaveLength(0);
  });

  it('counts bytes, not characters', async () => {
    const api = writable();
    const { box } = await startEditing();
    // Two bytes each in UTF-8: fewer characters than the bound, more bytes.
    replaceText(box, 'é'.repeat(MAX_BODY_BYTES / 2 + 1));
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('Not saved: the text is too long.'));
    expect(api.callsTo(SAVE)).toHaveLength(0);
    replaceText(box, 'short again');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

// A large text on a slow link may take long (the wait allows for what is sent): the person is
// told, and may stop it.
describe('a save that takes long', () => {
  /** Large enough that its wait is four seconds longer than an ordinary request's. */
  const LARGE = 'x'.repeat(4 * UPLOAD_BYTES_PER_SECOND);
  const stop = () => screen.queryByRole('button', { name: 'Stop' });
  /** The signals of the `save_draft` requests sent so far. */
  const saveSignals = () =>
    vi
      .mocked(globalThis.fetch)
      .mock.calls.filter(([input]) => input instanceof Request && input.url.endsWith(SAVE))
      .map(([, init]) => init?.signal);

  /** A large text is being saved, and `STILL_SENDING_AFTER_MS` less one millisecond have passed. */
  async function sending(save: Handler) {
    const api = writable({ [SAVE]: save });
    const { box } = await startEditing();
    replaceText(box, LARGE);
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    await pass(STILL_SENDING_AFTER_MS - 1);
    return { api, box };
  }

  it('says it is still sending after 30 seconds, and offers "Stop": not before, and focus stays', async () => {
    const { box } = await sending(() => new Promise<never>(() => {}));
    expect(STILL_SENDING_AFTER_MS).toBe(30_000);
    expect(saveState()).toBe('Saving…');
    expect(stop()).toBeNull();
    await pass(1);
    expect(saveState()).toBe('Still sending…');
    expect(stop()).not.toBeNull();
    // Nothing takes focus from where the person put it.
    expect(document.activeElement).toBe(box);
    expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
  });

  it('stops the request at "Stop": the text is not saved, a save is owed, and focus is in the text box', async () => {
    let answering = false;
    const { api, box } = await sending((body) =>
      answering ? saved(body) : new Promise<never>(() => {}),
    );
    await pass(1);
    // A change made meanwhile has come due, and waits for the save in flight.
    typeInto(box, ' and more');
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    const button = stop();
    if (button === null) throw new Error('no "Stop"');
    act(() => button.focus());
    fireEvent.click(button);
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    // The request itself was aborted, not only forgotten.
    expect(saveSignals().map((signal) => signal?.aborted)).toEqual([true]);
    expect(
      screen.getByText('Saving this draft: You stopped this before the service answered.'),
    ).toBeDefined();
    expect(screen.getByText(/Whether the service carried it out is not known\./)).toBeDefined();
    expect(stop()).toBeNull();
    expect(document.activeElement).toBe(box);
    expect(textIn(box)).toBe(`${LARGE} and more`);
    expect(screen.getByRole('tab', { name: 'Meeting notes (not saved)' })).toBeDefined();
    // Stopped is stopped: nothing is sent by itself, though a change was waiting.
    await pass(AUTOSAVE_DELAY_MS * 5);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    // The save is owed: the retry sends the text on screen, as a save like any other.
    answering = true;
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
    expect(api.callsTo(SAVE).at(1)?.body).toBe(`${LARGE} and more`);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(api.violations).toEqual([]);
  });

  it('sends nothing by itself after Stop, for a change still inside the autosave delay', async () => {
    const { api, box } = await sending(() => new Promise<never>(() => {}));
    await pass(1);
    // A change made while the save is in flight, its autosave still pending when Stop is pressed.
    typeInto(box, ' and more');
    await pass(100);
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    await pass(AUTOSAVE_DELAY_MS * 5);
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(textIn(box)).toBe(`${LARGE} and more`);
  });

  // After "Stop" nothing is sent without the person. The autosave's timer is the test above;
  // these are the other ways a save is sent with nobody asking, and then the person acting.
  describe('after "Stop"', () => {
    /** A large save was stopped; what the next `save_draft` answers is the test's to say. */
    async function stopped(next: Handler = (body) => saved(body)) {
      let first = true;
      const made = await sending((body) => {
        if (!first) return next(body);
        first = false;
        return new Promise<never>(() => {});
      });
      await pass(1);
      fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
      await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
      expect(made.api.callsTo(SAVE)).toHaveLength(1);
      return made;
    }

    /** The page is hidden: another browser tab, a minimised window. */
    function hidePage() {
      const hidden = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
      document.dispatchEvent(new Event('visibilitychange'));
      hidden.mockRestore();
    }

    it('sends nothing when the page is hidden', async () => {
      const { api, box } = await stopped();
      hidePage();
      await pass(AUTOSAVE_DELAY_MS * 5);
      expect(api.callsTo(SAVE)).toHaveLength(1);
      expect(textIn(box)).toBe(LARGE);
    });

    it('sends nothing when the page is about to leave, and still asks before it does', async () => {
      const { api } = await stopped();
      const leaving = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(leaving);
      expect(leaving.defaultPrevented).toBe(true);
      await pass(AUTOSAVE_DELAY_MS * 5);
      expect(api.callsTo(SAVE)).toHaveLength(1);
    });

    it('sends nothing when the editor is left, and keeps the text', async () => {
      const { api } = await stopped();
      fireEvent.click(screen.getByRole('button', { name: 'Read the committed text' }));
      await eventually(() =>
        expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull(),
      );
      await pass(AUTOSAVE_DELAY_MS * 5);
      expect(api.callsTo(SAVE)).toHaveLength(1);
      fireEvent.click(screen.getByRole('button', { name: 'Edit your draft' }));
      await eventually(() => textBox('Text of Meeting notes'));
      expect(textIn(textBox('Text of Meeting notes'))).toBe(LARGE);
      expect(saveState()).toBe('Not saved. Your text is kept here.');
    });

    it('is over once the person types again: a hidden page then saves at once', async () => {
      const { api, box } = await stopped();
      typeInto(box, ' and more');
      hidePage();
      // No wait for the autosave's moment: the page being hidden is the moment again.
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      expect(api.callsTo(SAVE).at(1)?.body).toBe(`${LARGE} and more`);
    });

    it('is over once the person tries again: a retry that fails is a failed save like any other, which a hidden page sends', async () => {
      const { api } = await stopped(() => fail(503, 'unavailable', 'busy'));
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      await eventually(() =>
        expect(
          screen.getByText('Saving this draft: The service is temporarily unavailable.'),
        ).toBeDefined(),
      );
      hidePage();
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(3));
    });
  });

  it('repeats a stopped save under its key when the text is the same, and starts it as "Saving…"', async () => {
    const { api } = await sending(() => new Promise<never>(() => {}));
    await pass(1);
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
    // It may have been applied: the same request, so the service does not apply it twice.
    expect(api.callsTo(SAVE).at(1)).toEqual(api.callsTo(SAVE).at(0));
    // A new save starts as one: it is not "still" being sent, and there is nothing to stop yet.
    expect(saveState()).toBe('Saving…');
    expect(stop()).toBeNull();
  });

  it('moves focus from "Stop" to the text box when the save answers by itself', async () => {
    const answers: Array<() => void> = [];
    const { box } = await sending(
      (body) => new Promise<Reply>((resolve) => answers.push(() => resolve(saved(body)))),
    );
    await pass(1);
    const button = stop();
    if (button === null) throw new Error('no "Stop"');
    act(() => button.focus());
    act(() => answers.at(0)?.());
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(button.isConnected).toBe(false);
    expect(document.activeElement).toBe(box);
  });

  it('times out an ordinary save at that same moment, with nothing to stop', async () => {
    writable({ [SAVE]: () => new Promise<never>(() => {}) });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await pass(STILL_SENDING_AFTER_MS);
    await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
    expect(stop()).toBeNull();
    expect(
      screen.getByText('Saving this draft: The workspace service did not answer in time.'),
    ).toBeDefined();
  });
});

/** The item as `get_item` answers it while the person has a draft of it. */
const withDraft = (base = HEAD) => ({
  ...itemDocument(noteSummary, { owner: 'Ada' }),
  draft: {
    body: '# Meeting notes\n\nMy draft text',
    properties: { type: 'meeting-note', okf_jawn: { id: NOTE }, owner: 'Grace' },
    draft: draftOf(NOTE, base, LATER),
  },
});

describe('reopening an item that has a draft', () => {
  it('shows the draft, marked as a draft and not as the committed text', async () => {
    const api = writable({
      [ITEM]: (body) => (body.item_id === NOTE ? ok(withDraft()) : ok(itemDocument(otherSummary))),
      [DRAFTS]: () => ok({ items: [draftOf(NOTE, HEAD, LATER)], next_cursor: null }),
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    const box = await findTextBox('Text of Meeting notes');
    expect(textIn(box)).toBe('# Meeting notes\n\nMy draft text');
    expect(screen.getByText('Draft.')).toBeDefined();
    expect(saveState()).toMatch(/^Saved as a draft at/);
    expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
    // The committed text is not on screen until it is asked for.
    expect(screen.queryByText('Text of Meeting notes')).toBeNull();
    expect(api.callsTo('/api/reads/read-item')).toHaveLength(0);
    expect(api.callsTo(SAVE)).toHaveLength(0);

    const toggle = screen.getByRole('button', { name: 'Read the committed text' });
    act(() => toggle.focus());
    fireEvent.click(toggle);
    await screen.findByText('Text of Meeting notes');
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
    expect(
      screen.getByText('You have a draft of this item. Below is the committed text.'),
    ).toBeDefined();
    // One switch, whatever it says: it keeps keyboard focus.
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit your draft' }));
    expect(document.activeElement).toBe(toggle);
    fireEvent.click(toggle);
    expect(textIn(await findTextBox('Text of Meeting notes'))).toBe(
      '# Meeting notes\n\nMy draft text',
    );
    expect(api.violations).toEqual([]);
  });

  it('continues the draft from its own base, whatever the workspace shows now', async () => {
    const api = writable({
      [ITEM]: (body) =>
        body.item_id === NOTE ? ok(withDraft(OLD)) : ok(itemDocument(otherSummary)),
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    const box = await findTextBox('Text of Meeting notes');
    vi.useFakeTimers();
    typeInto(box, ' and more');
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE).map((sent) => sent.base_revision)).toEqual([OLD]);
    // The draft's own properties go with it, not the committed ones.
    expect(api.callsTo(SAVE).at(0)?.properties).toMatchObject({ owner: 'Grace' });
  });

  it('says so in a tab that cites another revision, and marks that tab', async () => {
    writable({
      [ITEM]: (body) => (body.item_id === NOTE ? ok(withDraft()) : ok(itemDocument(otherSummary))),
    });
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await screen.findByText('Text of Meeting notes');
    expect(await screen.findByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
    expect(screen.getByText(/^You have a draft of this item\. Editing starts from/)).toBeDefined();
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
  });
});

describe('typed text is never lost silently', () => {
  it('saves at once when the person goes to another item, and shows the text on return', async () => {
    const api = writable();
    const { box, router } = await startEditing();
    typeInto(box, ' kept');
    expect(api.callsTo(SAVE)).toHaveLength(0);
    const tree = screen.getByRole('tree', { name: 'Folders' });
    fireEvent.click(within(tree).getByRole('treeitem', { name: /^Plan/ }));
    await eventually(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`));
    // No wait for the autosave delay: leaving the editor is the moment.
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)?.body).toBe(`${COMMITTED} kept`);
    await eventually(() =>
      expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined(),
    );
    fireEvent.click(screen.getByRole('tab', { name: 'Meeting notes (draft)' }));
    await eventually(() => textBox('Text of Meeting notes'));
    expect(textIn(textBox('Text of Meeting notes'))).toBe(`${COMMITTED} kept`);
  });

  it('keeps text whose save failed through a closed tab, and lists it as not saved', async () => {
    const api = writable({ [SAVE]: () => fail(503, 'unavailable', 'busy') });
    const { box } = await startEditing();
    typeInto(box, ' kept');
    fireEvent.click(screen.getByRole('button', { name: 'Close tab' }));
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    await eventually(() => expect(screen.queryByRole('tab')).toBeNull());
    const drafts = screen.getByRole('region', { name: /^Drafts/ });
    const row = await eventuallyGet(() =>
      within(drafts).getByRole('button', { name: 'Meeting notes' }),
    );
    expect(row.closest('li')?.textContent).toContain('not saved');
    expect(within(drafts).getByRole('heading').textContent).toBe('Drafts (1)');
    fireEvent.click(row);
    await eventually(() => textBox('Text of Meeting notes'));
    expect(textIn(textBox('Text of Meeting notes'))).toBe(`${COMMITTED} kept`);
    expect(saveState()).toBe('Not saved. Your text is kept here.');
    expect(screen.getByRole('button', { name: 'Try again' })).toBeDefined();
  });

  it('asks before the page is left with unsaved text, and saves at once', async () => {
    const api = writable();
    const { box } = await startEditing();
    const leaving = () => {
      const event = new Event('beforeunload', { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };
    // Nothing typed: the page leaves without a question.
    expect(leaving()).toBe(false);
    typeInto(box, ' kept');
    expect(leaving()).toBe(true);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    // Saved: nothing is at risk any more.
    expect(leaving()).toBe(false);
  });

  it('asks while a save is still in flight, even with the text back to what it was', async () => {
    writable({ [SAVE]: () => new Promise<never>(() => {}) });
    const { box } = await startEditing();
    typeInto(box, ' kept');
    await pass(AUTOSAVE_DELAY_MS);
    expect(saveState()).toBe('Saving…');
    // The request in flight would leave the service holding text the editor no longer shows.
    replaceText(box, COMMITTED);
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it('saves at once when the page is hidden', async () => {
    const api = writable();
    const { box } = await startEditing();
    typeInto(box, ' kept');
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    vi.restoreAllMocks();
  });
});

// An editor reads the text once, when it opens: two over one session would each write the other's
// words away. So there is one editor per item on a page.
describe('the same item in two panes', () => {
  const boxes = () => screen.queryAllByRole('textbox', { name: 'Text of Meeting notes' });
  const pane = (which: 1 | 2, focused: 1 | 2) =>
    screen.getByRole('region', {
      name: `Documents, pane ${which} of 2${which === focused ? ', focused' : ''}`,
    });

  /** The note edited in pane 1, then split to the right: pane 2 has focus. */
  async function editedAndSplit() {
    const api = writable();
    const { box } = await startEditing();
    typeInto(box, ' first words');
    fireEvent.click(screen.getByRole('button', { name: 'Split right' }));
    await eventually(() => pane(2, 2));
    return { api, box };
  }

  it('shows the committed text in the second pane, says why, and keeps one editor', async () => {
    const { api, box } = await editedAndSplit();
    await eventually(() =>
      expect(within(pane(2, 2)).getByText('Text of Meeting notes')).toBeDefined(),
    );
    expect(boxes()).toEqual([box]);
    expect(
      within(pane(2, 2)).getByText(
        'This item is being edited in another pane. Below is the committed text.',
      ),
    ).toBeDefined();
    expect(within(pane(2, 2)).getByRole('button', { name: 'Edit here' })).toBeDefined();
    // The first pane's words are still the draft, and are saved.
    typeInto(box, ' and more');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(api.callsTo(SAVE).at(-1)?.body).toBe(`${COMMITTED} first words and more`);
    expect(document.querySelectorAll('.save-state')).toHaveLength(1);
  });

  it('reads the committed text for the second pane when the first opened on the draft and never read it', async () => {
    const api = writable({
      [ITEM]: (body) => (body.item_id === NOTE ? ok(withDraft()) : ok(itemDocument(otherSummary))),
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    // The pane opened on the draft: the committed text was not read.
    expect(api.callsTo('/api/reads/read-item')).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: 'Split right' }));
    await waitFor(() =>
      expect(within(pane(2, 2)).getByText('Text of Meeting notes')).toBeDefined(),
    );
    expect(boxes()).toHaveLength(1);
    expect(within(pane(2, 2)).getByRole('button', { name: 'Edit here' })).toBeDefined();
  });

  it('"Edit here" moves the editor, with its text, its save state and keyboard focus', async () => {
    const { api, box } = await editedAndSplit();
    const here = within(pane(2, 2)).getByRole('button', { name: 'Edit here' });
    act(() => here.focus());
    fireEvent.click(here);
    await eventually(() => expect(boxes()).toHaveLength(1));
    const moved = boxes().at(0);
    if (moved === undefined) throw new Error('no editor');
    expect(moved).not.toBe(box);
    expect(pane(2, 2).contains(moved)).toBe(true);
    // The text typed in the other pane came with it, saved or not.
    expect(textIn(moved)).toBe(`${COMMITTED} first words`);
    await eventually(() => expect(document.activeElement).toBe(moved));
    // The pane that had the editor reads the committed text and offers to take it back.
    expect(within(pane(1, 2)).getByRole('button', { name: 'Edit here' })).toBeDefined();
    expect(within(pane(1, 2)).getByText(/being edited in another pane/)).toBeDefined();
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(api.callsTo(SAVE).at(-1)?.body).toBe(`${COMMITTED} first words`);
    typeInto(moved, ' then these');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() =>
      expect(api.callsTo(SAVE).at(-1)?.body).toBe(`${COMMITTED} first words then these`),
    );
  });

  it('gives the editor up when its pane goes back to reading: no pane is said to edit while none does', async () => {
    const { box } = await editedAndSplit();
    await eventually(() =>
      expect(within(pane(2, 2)).getByRole('button', { name: 'Edit here' })).toBeDefined(),
    );
    const first = () => screen.getByRole('region', { name: /^Documents, pane 1 of 2/ });
    const second = () => screen.getByRole('region', { name: /^Documents, pane 2 of 2/ });
    fireEvent.click(within(first()).getByRole('button', { name: 'Read the committed text' }));
    // The pane that had said "being edited in another pane" wanted the editor: it has it now.
    await eventually(() => expect(boxes()).toHaveLength(1));
    const moved = boxes().at(0);
    if (moved === undefined) throw new Error('no editor');
    expect(moved).not.toBe(box);
    expect(second().contains(moved)).toBe(true);
    expect(textIn(moved)).toBe(`${COMMITTED} first words`);
    expect(within(second()).queryByText(/being edited in another pane/)).toBeNull();
    // The one line that says so is in the pane that reads, and it is true.
    expect(within(first()).getByText(/being edited in another pane/)).toBeDefined();
    expect(screen.getAllByText(/being edited in another pane/)).toHaveLength(1);
  });

  it('lets the other pane edit once the editing pane is closed', async () => {
    const { box } = await editedAndSplit();
    // Focus the first pane (the one editing) and close its tab: the pane goes.
    act(() =>
      within(pane(1, 2))
        .getByRole('tab', { name: /Meeting notes/ })
        .focus(),
    );
    await eventually(() => pane(1, 1));
    fireEvent.click(within(pane(1, 1)).getByRole('button', { name: 'Close tab' }));
    await eventually(() => expect(screen.getByRole('region', { name: 'Documents' })).toBeDefined());
    await eventually(() => expect(boxes()).toHaveLength(1));
    const left = boxes().at(0);
    if (left === undefined) throw new Error('no editor');
    expect(left).not.toBe(box);
    expect(textIn(left)).toBe(`${COMMITTED} first words`);
    expect(screen.queryByText(/being edited in another pane/)).toBeNull();
  });
});

describe('the drafts of a workspace', () => {
  const twoPages: Handler = (body) => {
    const page = body.page;
    const cursor =
      page !== null && typeof page === 'object' && 'cursor' in page ? page.cursor : null;
    return cursor === 'next'
      ? ok({ items: [draftOf(OTHER, HEAD, NOW)], next_cursor: null })
      : ok({ items: [draftOf(NOTE, HEAD, LATER)], next_cursor: 'next' });
  };

  it('lists every draft, page after page, and opens the item of a row', async () => {
    const api = writable({ [DRAFTS]: twoPages });
    const { router } = renderAt(`/w/${WS}`);
    const drafts = await screen.findByRole('region', { name: 'Drafts (2)' });
    await within(drafts).findByRole('button', { name: 'Meeting notes' });
    await within(drafts).findByRole('button', { name: 'Plan' });
    expect(
      within(drafts)
        .getAllByRole('listitem')
        .map((row) => row.textContent?.trim()),
    ).toEqual([
      expect.stringMatching(/^Meeting notes saved /),
      expect.stringMatching(/^Plan saved /),
    ]);
    expect(api.callsTo(DRAFTS).map((body) => body.page)).toEqual([
      { limit: 200, cursor: null },
      { limit: 200, cursor: 'next' },
    ]);
    fireEvent.click(within(drafts).getByRole('button', { name: 'Plan' }));
    await screen.findByRole('tab', { name: 'Plan (draft)', selected: true });
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`);
    expect(api.violations).toEqual([]);
  });

  it('marks drafted items in the tree', async () => {
    writable({
      [DRAFTS]: () => ok({ items: [draftOf(NOTE, HEAD, LATER)], next_cursor: null }),
    });
    renderAt(`/w/${WS}/i/${OTHER}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    expect(
      await within(tree).findByRole('treeitem', {
        name: 'Meeting notes, type meeting-note, draft',
      }),
    ).toBeDefined();
    expect(within(tree).getByRole('treeitem', { name: 'Plan, type plan' })).toBeDefined();
  });

  it('marks text that is typed and not saved yet, in the tree and on the tab', async () => {
    writable();
    const { box } = await startEditing();
    typeInto(box, ' new');
    const tree = screen.getByRole('tree', { name: 'Folders' });
    expect(
      within(tree).getByRole('treeitem', { name: 'Meeting notes, type meeting-note, unsaved' }),
    ).toBeDefined();
    expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
    expect(screen.getByRole('region', { name: 'Drafts (1)' })).toBeDefined();
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() =>
      expect(
        within(tree).getByRole('treeitem', { name: 'Meeting notes, type meeting-note, draft' }),
      ).toBeDefined(),
    );
  });

  it('says there are none', async () => {
    writable();
    renderAt(`/w/${WS}`);
    const drafts = await screen.findByRole('region', { name: 'Drafts (0)' });
    expect(within(drafts).getByText('You have no drafts in this workspace.')).toBeDefined();
  });

  it('keeps the drafts listed when a further page fails, and its retry brings the rest', async () => {
    let failing = true;
    writable({
      [DRAFTS]: (body) => {
        const reply = twoPages(body);
        const page = body.page;
        const further =
          page !== null && typeof page === 'object' && 'cursor' in page && page.cursor === 'next';
        return further && failing ? fail(500, 'internal', 'the store is unwell') : reply;
      },
    });
    renderAt(`/w/${WS}`);
    const drafts = await screen.findByRole('region', { name: 'Drafts (1)' });
    const retry = await within(drafts).findByRole('button', { name: 'Try again' });
    expect(
      within(drafts).getByText(
        'Listing the rest of your drafts: Something went wrong in the service.',
      ),
    ).toBeDefined();
    expect(within(drafts).getByRole('button', { name: 'Meeting notes' })).toBeDefined();
    failing = false;
    act(() => retry.focus());
    fireEvent.click(retry);
    await screen.findByRole('region', { name: 'Drafts (2)' });
    expect(within(drafts).queryByRole('button', { name: /Try again|Trying again/ })).toBeNull();
    expect(document.activeElement).toBe(
      within(drafts).getByRole('button', { name: 'Meeting notes' }),
    );
  });

  // `retry.ts` named one limit: an invalidation that replaces a request in flight. A first save,
  // a discard and a Snapshot list the drafts again, so here it is reachable.
  it.each([
    ['answers', 'the first draft'],
    ['fails', 'the retry button'],
  ] as const)(
    'follows the listing that replaces a further page’s retry in flight: when it %s, focus is on %s',
    async (outcome, _focus) => {
      let further: 'fail' | 'hold' = 'fail';
      let first: 'ok' | 'fail' = 'ok';
      const held: Array<() => void> = [];
      writable({
        [DRAFTS]: (body) => {
          const page = body.page;
          const cursor =
            page !== null && typeof page === 'object' && 'cursor' in page ? page.cursor : null;
          if (cursor !== 'next') {
            return first === 'ok'
              ? ok({ items: [draftOf(NOTE, HEAD, LATER)], next_cursor: 'next' })
              : fail(500, 'internal', 'the store is unwell again');
          }
          if (further === 'fail') return fail(500, 'internal', 'the store is unwell');
          return new Promise<Reply>((resolve) =>
            held.push(() => resolve(ok({ items: [draftOf(OTHER, HEAD, NOW)], next_cursor: null }))),
          );
        },
      });
      const { queryClient } = renderAt(`/w/${WS}`);
      const drafts = await screen.findByRole('region', { name: 'Drafts (1)' });
      const retry = await within(drafts).findByRole('button', { name: 'Try again' });
      further = 'hold';
      first = outcome === 'answers' ? 'ok' : 'fail';
      act(() => retry.focus());
      fireEvent.click(retry);
      await within(drafts).findByRole('button', { name: 'Trying again…' });
      expect(held).toHaveLength(1);

      // What a first save of another item does: the drafts are listed again, from the first page,
      // in place of the request in flight.
      act(() =>
        rememberDraft(queryClient, WS, SOURCE, {
          body: '',
          properties: {},
          draft: draftOf(SOURCE, HEAD, NOW),
        }),
      );
      // The retry is not over yet: it follows the listing that replaced its request.
      expect(within(drafts).getByRole('button', { name: 'Trying again…' })).toBe(retry);
      expect(document.activeElement).toBe(retry);

      if (outcome === 'answers') {
        const row = within(drafts).getByRole('button', { name: 'Meeting notes' });
        await waitFor(() => expect(document.activeElement).toBe(row));
        expect(within(drafts).queryByRole('button', { name: /Try again|Trying again/ })).toBeNull();
        // The further page is asked for again by the panel, and arrives.
        await waitFor(() => expect(held).toHaveLength(2));
        for (const answer of held.splice(0)) answer();
        await screen.findByRole('region', { name: 'Drafts (2)' });
        expect(document.activeElement).toBe(row);
      } else {
        await within(drafts).findByText(
          'Listing the rest of your drafts: Something went wrong in the service.',
        );
        await waitFor(() =>
          expect(document.activeElement).toBe(
            within(drafts).getByRole('button', { name: 'Try again' }),
          ),
        );
        // What was listed stays listed.
        expect(within(drafts).getByRole('button', { name: 'Meeting notes' })).toBeDefined();
      }
      expect(document.activeElement).not.toBe(document.body);
    },
  );

  it('shows a failed listing in plain words, never as "no drafts"', async () => {
    const { [DRAFTS]: _unbuilt, ...handlers } = explorerHandlers();
    mockApi(handlers);
    renderAt(`/w/${WS}`);
    const drafts = await screen.findByRole('region', { name: 'Drafts' });
    expect(
      await within(drafts).findByText('Listing your drafts: Not available in this build.'),
    ).toBeDefined();
    expect(within(drafts).queryByText('You have no drafts in this workspace.')).toBeNull();
  });
});

describe('discarding a draft', () => {
  it('asks first, then removes the draft and goes back to the committed text', async () => {
    let held = true;
    const api = writable({
      [ITEM]: (body) =>
        body.item_id === NOTE && held
          ? ok(withDraft())
          : ok(itemDocument(body.item_id === NOTE ? noteSummary : otherSummary)),
      [DRAFTS]: () => ok({ items: held ? [draftOf(NOTE, HEAD, LATER)] : [], next_cursor: null }),
      [DISCARD]: (body) => {
        held = false;
        return ok(draftOf(String(body.item_id), HEAD, LATER));
      },
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await screen.findByRole('region', { name: 'Drafts (1)' });
    const discard = screen.getByRole('button', { name: 'Discard draft' });
    act(() => discard.focus());
    fireEvent.click(discard);
    const question = screen.getByRole('group', { name: 'Discard this draft?' });
    const keep = within(question).getByRole('button', { name: 'Keep the draft' });
    expect(document.activeElement).toBe(keep);
    expect(api.callsTo(DISCARD)).toHaveLength(0);

    fireEvent.click(keep);
    expect(screen.queryByRole('group', { name: 'Discard this draft?' })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Discard draft' }));
    expect(api.callsTo(DISCARD)).toHaveLength(0);
    expect(textIn(textBox('Text of Meeting notes'))).toBe('# Meeting notes\n\nMy draft text');

    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await screen.findByText('Text of Meeting notes');
    expect(api.callsTo(DISCARD)).toHaveLength(1);
    expect(api.callsTo(DISCARD).at(0)).toMatchObject({ workspace_id: WS, item_id: NOTE });
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
    // Focus is on the switch, which offers to edit again.
    expect(document.activeElement).toBe(await screen.findByRole('button', { name: 'Edit' }));
    expect(await screen.findByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    expect(await screen.findByRole('region', { name: 'Drafts (0)' })).toBeDefined();
    expect(api.violations).toEqual([]);
  });

  it('puts focus on the switch after a discard pressed with a pointer that does not focus what it presses', async () => {
    writable({ [DISCARD]: (body) => ok(draftOf(String(body.item_id), HEAD, LATER)) });
    const { box } = await startEditing();
    typeInto(box, ' and more');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    // Focus is nowhere in the editor when the button is pressed (a pointer in WebKit).
    await eventually(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Keep the draft' })),
    );
    act(() => screen.getByRole('button', { name: 'Keep the draft' }).blur());
    expect(document.activeElement).toBe(document.body);
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await eventually(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit' })),
    );
  });

  it('keeps the draft when the discard fails, and repeats it under the same key', async () => {
    let failing = true;
    const api = writable({
      [ITEM]: (body) => (body.item_id === NOTE ? ok(withDraft()) : ok(itemDocument(otherSummary))),
      [DISCARD]: (body) =>
        failing ? fail(503, 'unavailable', 'busy') : ok(draftOf(String(body.item_id), HEAD, LATER)),
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    expect(
      await screen.findByText('Discarding this draft: The service is temporarily unavailable.'),
    ).toBeDefined();
    expect(textIn(textBox('Text of Meeting notes'))).toBe('# Meeting notes\n\nMy draft text');
    failing = false;
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await screen.findByText('Text of Meeting notes');
    const [first, second] = api.callsTo(DISCARD);
    expect(api.callsTo(DISCARD)).toHaveLength(2);
    expect(second).toEqual(first);
  });

  it('is not undone by a read of the item that began before it and answers after it', async () => {
    let held = true;
    const read: { hold: boolean; answer?: () => void } = { hold: false };
    const api = writable({
      [ITEM]: (body) => {
        if (body.item_id !== NOTE) return ok(itemDocument(otherSummary));
        const reply = held ? ok(withDraft()) : ok(itemDocument(noteSummary));
        if (!read.hold) return reply;
        read.hold = false;
        return new Promise<Reply>((resolve) => {
          read.answer = () => resolve(reply);
        });
      },
      [DRAFTS]: () => ok({ items: held ? [draftOf(NOTE, HEAD, LATER)] : [], next_cursor: null }),
      [DISCARD]: (body) => {
        held = false;
        return ok(draftOf(String(body.item_id), HEAD, LATER));
      },
    });
    const { queryClient } = renderAt(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    const key = itemOptions(WS, NOTE, HEAD).queryKey;
    // A read of the item begins while the draft is still there, and has no answer yet.
    read.hold = true;
    act(() => {
      void queryClient.invalidateQueries({ queryKey: key });
    });
    await waitFor(() => expect(read.answer).toBeDefined());
    const reads = api.callsTo(ITEM).length;
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await waitFor(() =>
      expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull(),
    );
    // The old answer arrives now, with the draft in it.
    act(() => read.answer?.());
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
    expect(queryClient.getQueryData(key)?.draft ?? null).toBeNull();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    expect(await screen.findByRole('button', { name: 'Edit' })).toBeDefined();
    // The item was read again after the discard: that answer has the discard in it.
    expect(api.callsTo(ITEM).length).toBeGreaterThan(reads);
  });

  it('takes back a first read of the item that is in flight when the cache is told of the discard', async () => {
    // A draft is not bound to a revision: a read of the item at another one carries it too. This
    // one has no answer yet, and nothing before it to fall back on.
    let held = true;
    const read: { answer?: () => void } = {};
    const api = writable({
      [ITEM]: () => {
        const reply = ok(held ? withDraft() : itemDocument(noteSummary));
        if (read.answer !== undefined) return reply;
        return new Promise<Reply>((resolve) => {
          read.answer = () => resolve(reply);
        });
      },
    });
    const client = createQueryClient();
    const options = itemOptions(WS, NOTE, OLD);
    const observer = new QueryObserver(client, options);
    const stop = observer.subscribe(() => {});
    await waitFor(() => expect(read.answer).toBeDefined());
    // The draft is discarded; the cache is told.
    held = false;
    rememberDraft(client, WS, NOTE, null);
    // The old answer arrives now, with the draft in it.
    read.answer?.();
    await waitFor(() => expect(client.getQueryData(options.queryKey)).toBeDefined());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(client.getQueryData(options.queryKey)?.draft ?? null).toBeNull();
    // What is shown came from a read asked after the discard.
    expect(api.callsTo(ITEM)).toHaveLength(2);
    stop();
    client.clear();
  });

  // A discard's key belongs to the item's draft, not to the editor that asked.
  it('repeats a discard under the same key after the editor was left and opened again', async () => {
    let failing = true;
    const api = writable({
      [ITEM]: (body) => (body.item_id === NOTE ? ok(withDraft()) : ok(itemDocument(otherSummary))),
      [DISCARD]: (body) =>
        failing ? fail(503, 'unavailable', 'busy') : ok(draftOf(String(body.item_id), HEAD, LATER)),
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await screen.findByText('Discarding this draft: The service is temporarily unavailable.');
    // The person reads the committed text and comes back: the editor is a new one.
    fireEvent.click(screen.getByRole('button', { name: 'Read the committed text' }));
    await waitFor(() =>
      expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull(),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Edit your draft' }));
    await findTextBox('Text of Meeting notes');
    expect(screen.queryByText(/^Discarding this draft:/)).toBeNull();
    failing = false;
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await waitFor(() => expect(api.callsTo(DISCARD)).toHaveLength(2));
    const [first, second] = api.callsTo(DISCARD);
    expect(second).toEqual(first);
  });

  it('asks a discard under a new key once the draft was saved again', async () => {
    let failing = true;
    const api = writable({
      [ITEM]: (body) => (body.item_id === NOTE ? ok(withDraft()) : ok(itemDocument(otherSummary))),
      [DISCARD]: (body) =>
        failing ? fail(503, 'unavailable', 'busy') : ok(draftOf(String(body.item_id), HEAD, LATER)),
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    const box = await findTextBox('Text of Meeting notes');
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await screen.findByText('Discarding this draft: The service is temporarily unavailable.');
    // The first discard may have been applied. The text saved now is another draft.
    vi.useFakeTimers();
    typeInto(box, ' after all');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    failing = false;
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await eventually(() => expect(api.callsTo(DISCARD)).toHaveLength(2));
    const [first, second] = api.callsTo(DISCARD);
    expect({ ...second, idempotency_key: null }).toEqual({ ...first, idempotency_key: null });
    expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
  });

  it('waits for a first save still in flight, then discards the draft that save made', async () => {
    let held = false;
    const saves: Array<() => void> = [];
    const api = writable({
      [SAVE]: (body) =>
        new Promise<Reply>((resolve) => {
          saves.push(() => {
            held = true;
            resolve(saved(body));
          });
        }),
      [DISCARD]: (body) => {
        held = false;
        return ok(draftOf(String(body.item_id), HEAD, LATER));
      },
    });
    const { box, queryClient } = await startEditing();
    typeInto(box, ' unwanted');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toBe('Saving…'));
    // The save has no answer yet: whether the service holds a draft is not known.
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await pass(50);
    expect(api.callsTo(DISCARD)).toHaveLength(0);
    expect(textIn(textBox('Text of Meeting notes'))).toBe(`${COMMITTED} unwanted`);
    // It lands: the service holds the draft, and the discard asks for it to go.
    act(() => saves.shift()?.());
    await eventually(() => expect(api.callsTo(DISCARD)).toHaveLength(1));
    await eventually(() =>
      expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull(),
    );
    expect(held).toBe(false);
    expect(
      queryClient.getQueryData(itemOptions(WS, NOTE, HEAD).queryKey)?.draft ?? null,
    ).toBeNull();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
  });

  it('asks nothing of the service for text no save of which was ever sent', async () => {
    const api = writable();
    const { box } = await startEditing();
    expect(
      screen.getByRole('button', { name: 'Discard draft' }).getAttribute('aria-disabled'),
    ).toBe('true');
    typeInto(box, ' unwanted');
    // Discarded before the autosave: the service was never asked to hold any of it.
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await eventually(() => expect(screen.getByText('Text of Meeting notes')).toBeDefined());
    await pass(AUTOSAVE_DELAY_MS * 3);
    expect(api.callsTo(DISCARD)).toHaveLength(0);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    // Editing again starts from the committed text, not from what was discarded.
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    await eventually(() => textBox('Text of Meeting notes'));
    expect(textIn(textBox('Text of Meeting notes'))).toBe(COMMITTED);
  });

  // A first save that failed or timed out may have been applied: the service may hold the draft.
  describe('after a first save whose outcome is unknown', () => {
    const COMMIT = '/api/history/commit-items';

    /** A service that holds what it is sent, and whose first save answers as `first` says. */
    function service(first: 'never answers' | 'refuses and holds nothing') {
      const state: { draft: string | null; committed: string } = {
        draft: null,
        committed: COMMITTED,
      };
      const held = () => (state.draft === null ? null : draftOf(NOTE, HEAD, LATER));
      const api = writable({
        [ITEM]: (body) => {
          if (body.item_id !== NOTE) return ok(itemDocument(otherSummary));
          const document = { ...itemDocument(noteSummary), body: state.committed };
          const draft = held();
          return ok(
            state.draft === null || draft === null
              ? document
              : {
                  ...document,
                  draft: { body: state.draft, properties: document.properties, draft },
                },
          );
        },
        [DRAFTS]: () => {
          const draft = held();
          return ok({ items: draft === null ? [] : [draft], next_cursor: null });
        },
        [SAVE]: (body) => {
          if (first === 'refuses and holds nothing') return fail(503, 'unavailable', 'busy');
          // Applied, and its answer never arrives.
          state.draft = String(body.body);
          return new Promise<never>(() => {});
        },
        [DISCARD]: () => {
          const was = held();
          state.draft = null;
          return was === null ? fail(404, 'not_found', 'you hold no draft of this item') : ok(was);
        },
        [COMMIT]: () => {
          state.committed = state.draft ?? state.committed;
          state.draft = null;
          return ok({ revision: 'c'.repeat(40), receipt_id: NOTE, warnings: [] });
        },
      });
      return { api, state };
    }

    async function discardAfterTheSave() {
      const { box } = await startEditing();
      typeInto(box, ' secret words');
      await pass(AUTOSAVE_DELAY_MS);
      await pass(REQUEST_TIMEOUT_MS);
      await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
      const discard = screen.getByRole('button', { name: 'Discard draft' });
      act(() => discard.focus());
      fireEvent.click(discard);
      fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
      await eventually(() =>
        expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull(),
      );
    }

    it('asks the service to discard, so the draft is not listed and no Snapshot commits it', async () => {
      const { api, state } = service('never answers');
      await discardAfterTheSave();
      expect(api.callsTo(DISCARD)).toHaveLength(1);
      expect(api.callsTo(DISCARD).at(0)).toMatchObject({ workspace_id: WS, item_id: NOTE });
      expect(state.draft).toBeNull();
      // The pane and the panel agree: no draft, anywhere.
      const drafts = await eventuallyGet(() => screen.getByRole('region', { name: 'Drafts (0)' }));
      expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit' }));
      // A Snapshot has nothing to take, and takes nothing.
      fireEvent.change(within(drafts).getByRole('textbox', { name: 'Snapshot name' }), {
        target: { value: 'Tidy' },
      });
      fireEvent.click(within(drafts).getByRole('button', { name: 'Snapshot 0 drafts' }));
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(COMMIT)).toHaveLength(0);
      expect(state.committed).toBe(COMMITTED);
      expect(api.violations).toEqual([]);
    });

    it('takes the answer that the service holds no such draft as the discard done', async () => {
      const { api } = service('refuses and holds nothing');
      await discardAfterTheSave();
      expect(api.callsTo(DISCARD)).toHaveLength(1);
      // "Not found" here is what was asked for: nothing is shown as a failure.
      expect(screen.queryByText(/^Discarding this draft:/)).toBeNull();
      expect(screen.queryByText(/could not be found/)).toBeNull();
      await eventually(() => expect(screen.getByText('Text of Meeting notes')).toBeDefined());
      expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
      expect(screen.getByRole('region', { name: 'Drafts (0)' })).toBeDefined();
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit' }));
      // Editing again starts from the committed text.
      fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
      await eventually(() => textBox('Text of Meeting notes'));
      expect(textIn(textBox('Text of Meeting notes'))).toBe(COMMITTED);
    });

    it('keeps the text when the discard itself fails, and says so', async () => {
      const { api } = service('refuses and holds nothing');
      const { box } = await startEditing();
      typeInto(box, ' secret words');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(saveState()).toBe('Not saved. Your text is kept here.'));
      // The service is not there for the discard either.
      vi.stubGlobal(
        'fetch',
        vi.fn(() => Promise.reject(new TypeError('Failed to fetch'))),
      );
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
      await eventually(() =>
        expect(
          screen.getByText('Discarding this draft: The workspace service could not be reached.'),
        ).toBeDefined(),
      );
      expect(textIn(textBox('Text of Meeting notes'))).toBe(`${COMMITTED} secret words`);
      expect(api.callsTo(DISCARD)).toHaveLength(0);
    });
  });
});

/** Under the test's clock: the button with this name, once it is there. */
async function screenButton(name: string): Promise<HTMLElement> {
  return eventuallyGet(() => screen.getByRole('button', { name }));
}

async function eventuallyGet(get: () => HTMLElement): Promise<HTMLElement> {
  await eventually(() => {
    get();
  });
  return get();
}
