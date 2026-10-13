/**
 * Editing an item's text as a draft: the editor in place of the reader, autosave through
 * `save_draft` with the base revision, the save state always said, typed text never lost, the
 * person's draft shown when an item is reopened, the drafts listed and marked, and discarding.
 */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { REQUEST_TIMEOUT_MS } from '../../src/app/api-client';
import { itemOptions } from '../../src/features/documents/DocumentView';
import { draftsOptions, rememberDraft } from '../../src/features/drafts/drafts-list';
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

  it('lets a tab that cites the shown version be edited', async () => {
    writable();
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    expect(await screen.findByRole('button', { name: 'Edit' })).toBeDefined();
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

  it('keeps the base the service recorded for later saves, never the head on screen', async () => {
    // The service records its own base at the first save: here an older one than the pane reads.
    const api = writable({ [SAVE]: (body) => saved(body, OLD) });
    const { box } = await startEditing();
    typeInto(box, ' one');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    typeInto(box, ' two');
    await pass(AUTOSAVE_DELAY_MS);
    expect(api.callsTo(SAVE).map((sent) => sent.base_revision)).toEqual([HEAD, OLD]);
    expect(screen.getByText('Draft.').parentElement?.textContent).toContain(OLD.slice(0, 7));
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

  it('asks nothing of the service for text that was never saved', async () => {
    const api = writable({ [SAVE]: () => fail(503, 'unavailable', 'busy') });
    const { box } = await startEditing();
    expect(
      screen.getByRole('button', { name: 'Discard draft' }).getAttribute('aria-disabled'),
    ).toBe('true');
    typeInto(box, ' unwanted');
    await pass(AUTOSAVE_DELAY_MS);
    await screenButton('Try again');
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await eventually(() => expect(screen.getByText('Text of Meeting notes')).toBeDefined());
    expect(api.callsTo(DISCARD)).toHaveLength(0);
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    // Editing again starts from the committed text, not from what was discarded.
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    await eventually(() => textBox('Text of Meeting notes'));
    expect(textIn(textBox('Text of Meeting notes'))).toBe(COMMITTED);
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
