/**
 * The purges (rulings C3, C4 and C7). "Purge this workspace…", on its Administration page, and
 * "Purge this item…", among the item's own actions, ask through the destructive dialog: it says in
 * SPEC section 8's words what a purge removes and what it cannot erase, is bound to the
 * `base_revision` it was opened at, repeats a failed request under its key, and says a refusal
 * in words with what it allows. A workspace purge answers 202 with its record, and the page goes
 * to that record (`/purges/<id>`), read with `get_purge` alone: its state, its target, who asked
 * and when, its last failure and every count of its report, asked again while it has not
 * finished. An item purge is not built on main: it reads "Not available in this build.", in the
 * dialog, and nothing is removed.
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
import type { Purge, Workspace } from '../../src/api/generated/types.gen';
import { PURGE_POLL_MS } from '../../src/features/admin/purges/purges';
import { adoptHead } from '../../src/features/explorer/head';
import { humanSessionWords } from '../../src/features/explorer/rights';
import { formatInstant } from '../../src/lib/instant';
import { adminWorkspace, GET_PURGE, PURGE_ITEM, PURGE_WORKSPACE } from './support/admin';
import {
  acceptedPurge,
  archivedAdminWorkspace,
  COMPLETED_AT,
  NEWER,
  PURGE_ID,
  PURGE_JOB,
  purgeOf,
  REQUESTED_AT,
} from './support/admin-lifecycle';
import { fail, type Handler, mockApi, notImplemented, ok, type Reply } from './support/api';
import { explorerHandlers, HEAD, NOTE, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

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

/**
 * Wait until `check` holds after the clock itself started something, moving it on a millisecond at
 * a time: a timer set while the clock is being moved is due a millisecond later.
 */
async function later(check: () => void) {
  for (let turn = 0; turn < 200; turn += 1) {
    try {
      check();
      return;
    } catch {
      await pass(1);
    }
  }
  check();
}

/** Answers in order; the last one again once they are used up. */
function inTurn(...answers: Reply[]): Handler {
  let left = answers;
  return () => {
    const [next, ...rest] = left;
    if (rest.length > 0) left = rest;
    return next ?? fail(500, 'internal', 'no answer');
  };
}

async function openAdmin(who: Workspace, handlers: Record<string, Handler> = {}) {
  const api = mockApi(explorerHandlers({ [OPEN]: () => ok(who), ...handlers }));
  const app = renderAt(`/w/${WS}/admin`);
  await screen.findByRole('region', { name: 'This workspace' });
  return { api, ...app };
}

const opener = () => screen.getByRole('button', { name: 'Purge this workspace…' });
const workspaceDialog = () => screen.findByRole('alertdialog', { name: 'Purge this workspace?' });
const confirmWorkspace = () => screen.getByRole('button', { name: 'Purge the workspace' });

/** What an element's `aria-describedby` names, as one text. */
function descriptionOf(element: HTMLElement): string {
  return (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ')
    .trim();
}

describe('purging a workspace (C3)', { timeout: 30_000 }, () => {
  it('says in SPEC section 8’s words what it removes and what it cannot erase, and who may ask', async () => {
    const { api } = await openAdmin(adminWorkspace);
    expect(descriptionOf(opener())).toContain(humanSessionWords('Purging a workspace'));
    opener().focus();
    fireEvent.click(opener());
    const dialog = await workspaceDialog();
    const text = dialog.textContent ?? '';
    expect(text).toContain(
      'A purge removes the original bytes, derivatives, index entries, historical content and installation-managed backups of this workspace',
    );
    expect(text).toContain(
      'marks the revisions, citations and Views that referred to them as invalidated.',
    );
    expect(text).toContain(
      'A purge never erases exports, clones or backups outside the application’s control',
    );
    expect(text).toContain('It leaves a record of what was removed, by identity, with no content.');
    expect(text).toContain('It cannot be undone.');
    // Exactly what is purged, at the revision the page shows.
    expect(text).toContain('The workspace “Workshop notes”');
    expect(text).toContain(`as of revision ${HEAD.slice(0, 10)}`);
    expect(text).toContain(humanSessionWords('Purging a workspace'));
    // No further challenge: Cancel has focus, and the action is one press away.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' })),
    );
    expect(within(dialog).queryByRole('textbox')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener()));
    expect(api.callsTo(PURGE_WORKSPACE)).toEqual([]);
  });

  it('is bound to the revision it was opened at, and then shows the purge’s record', async () => {
    const { api, queryClient, router } = await openAdmin(adminWorkspace, {
      [PURGE_WORKSPACE]: () => acceptedPurge(purgeOf('requested')),
      [GET_PURGE]: () => ok(purgeOf('running')),
    });
    fireEvent.click(opener());
    await workspaceDialog();
    // The head the shell shows moves on while the dialog is open: it stays bound to the one it saw.
    act(() => {
      adoptHead(queryClient, WS, NEWER);
    });
    expect((await workspaceDialog()).textContent).toContain(`as of revision ${HEAD.slice(0, 10)}`);
    fireEvent.click(confirmWorkspace());
    await waitFor(() => expect(router.state.location.pathname).toBe(`/purges/${PURGE_ID}`));
    expect(api.callsTo(PURGE_WORKSPACE)).toEqual([
      { workspace_id: WS, base_revision: HEAD, idempotency_key: expect.stringMatching(UUID) },
    ]);
    const heading = await screen.findByRole('heading', { level: 1, name: 'Purge 66666666' });
    await waitFor(() => expect(document.activeElement).toBe(heading));
    // The record as the purge was answered, then as the service has it now.
    await screen.findByText('Running: removing what it covers.');
    expect(api.callsTo(GET_PURGE)).toEqual([{ purge_id: PURGE_ID }]);
    expect(api.violations).toEqual([]);
  });

  it('repeats a failed purge under the same key, from the dialog', async () => {
    const { api, router } = await openAdmin(adminWorkspace, {
      [PURGE_WORKSPACE]: inTurn(
        fail(503, 'unavailable', 'busy'),
        acceptedPurge(purgeOf('requested')),
      ),
      [GET_PURGE]: () => ok(purgeOf('requested')),
    });
    fireEvent.click(opener());
    await workspaceDialog();
    fireEvent.click(confirmWorkspace());
    const retry = await screen.findByRole('button', { name: 'Try again' });
    expect(screen.getByRole('alertdialog').textContent).toContain(
      'Purging this workspace: The service is temporarily unavailable.',
    );
    await act(async () => {
      fireEvent.click(retry);
    });
    await waitFor(() => expect(router.state.location.pathname).toBe(`/purges/${PURGE_ID}`));
    const keys = api.callsTo(PURGE_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
  });

  it('says a refusal from the service in words, with what it allows; nothing is removed', async () => {
    const { api, router } = await openAdmin(adminWorkspace, {
      [PURGE_WORKSPACE]: () =>
        fail(403, 'forbidden', 'purging needs the tenant administrator in a human session'),
    });
    fireEvent.click(opener());
    await workspaceDialog();
    fireEvent.click(confirmWorkspace());
    const dialog = await workspaceDialog();
    const alert = await within(dialog).findByRole('alert');
    expect(alert.textContent).toContain('Purging this workspace: The service refused this purge.');
    expect(alert.textContent).toContain(
      'A purge is asked for only by an administrator of this installation, in their own browser session',
    );
    expect(alert.textContent).toContain('Ask an administrator of this installation');
    expect(alert.textContent).toContain('Nothing was removed.');
    expect(alert.textContent).toContain(
      'purging needs the tenant administrator in a human session',
    );
    // Not a refusal to repeat: no retry is offered, and the page stays where it is.
    expect(within(dialog).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(router.state.location.pathname).toBe(`/w/${WS}/admin`);
    expect(api.callsTo(PURGE_WORKSPACE)).toHaveLength(1);
  });

  it('stays offered in an archived workspace, and runs', async () => {
    const { api, router } = await openAdmin(archivedAdminWorkspace, {
      [PURGE_WORKSPACE]: () => acceptedPurge(purgeOf('requested')),
      [GET_PURGE]: () => ok(purgeOf('requested')),
    });
    expect(opener().getAttribute('aria-disabled')).toBe('false');
    fireEvent.click(opener());
    await workspaceDialog();
    fireEvent.click(confirmWorkspace());
    await waitFor(() => expect(router.state.location.pathname).toBe(`/purges/${PURGE_ID}`));
    expect(api.callsTo(PURGE_WORKSPACE)).toHaveLength(1);
  });

  it('is not offered to a person without admin here', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace) }));
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'Administration' });
    expect(screen.queryByRole('button', { name: 'Purge this workspace…' })).toBeNull();
  });
});

/** A purge's record page, answered by `get_purge`. */
async function openRecord(answer: Handler) {
  const api = mockApi({ [GET_PURGE]: answer });
  const app = renderAt(`/purges/${PURGE_ID}`);
  await screen.findByRole('heading', { level: 1, name: 'Purge 66666666' });
  return { api, ...app };
}

const record = () => screen.getByRole('region', { name: 'Purge 66666666' });

describe('a purge’s record (C4)', { timeout: 30_000 }, () => {
  it.each([
    ['requested', 'Requested: accepted, and nothing removed yet.'],
    ['running', 'Running: removing what it covers.'],
    [
      'completed',
      'Completed: every copy the application manages is removed or rewritten, and no affected backup can be restored.',
    ],
    [
      'failed',
      'Failed: it stopped before it finished, and does not run again by itself. Asking for the same purge again resumes it.',
    ],
  ] as const)('says the state %s in words', async (state, words) => {
    const { api } = await openRecord(() => ok(purgeOf(state)));
    const status = await screen.findByText(words);
    expect(status.closest('[role="status"]')).not.toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('says what it removes, who asked and when, and asks with no workspace', async () => {
    const { api } = await openRecord(() => ok(purgeOf('completed')));
    await screen.findByText(/Completed:/);
    const text = record().textContent ?? '';
    expect(text).toContain(`What it removes: the workspace ${WS}.`);
    expect(text).toContain(`Asked for by user_01 on ${formatInstant(REQUESTED_AT)}.`);
    expect(text).toContain(`Completed on ${formatInstant(COMPLETED_AT)}.`);
    expect(text).toContain(`Its work is the installation’s job ${PURGE_JOB.slice(0, 8)}.`);
    expect(text).toContain(
      'never erases exports, clones or backups outside the application’s control',
    );
    // Tenant-level: readable after the workspace is gone, so nothing of the workspace is asked.
    expect(api.calls.map((call) => call.path)).toEqual([GET_PURGE]);
    expect(api.violations).toEqual([]);
  });

  it('says the item an item purge removes, and its workspace', async () => {
    await openRecord(() =>
      ok(purgeOf('requested', { target: { kind: 'item', item_id: NOTE, workspace_id: WS } })),
    );
    await screen.findByText(/Requested:/);
    expect(record().textContent).toContain(
      `What it removes: the item ${NOTE} of the workspace ${WS}.`,
    );
  });

  it('says every count of a completed purge’s report in words, the large ones grouped', async () => {
    await openRecord(() => ok(purgeOf('completed')));
    const heading = await screen.findByRole('heading', { level: 2, name: 'What it removed' });
    const list = heading.nextElementSibling;
    expect(list?.tagName).toBe('UL');
    const items = Array.from(list?.querySelectorAll('li') ?? [], (li) => li.textContent);
    expect(items).toEqual([
      '9,876,543,210 stored objects deleted.',
      '1,234,567 stored objects kept, because a surviving item still references them.',
      '1,200 revisions that no longer exist or were rewritten.',
      '4 stored citations of what was purged (reviews, receipts, comments, View bindings) marked invalidated.',
      '5 stored citations of other items moved to a rewritten revision.',
      '11 saved Views with a binding marked invalidated.',
      '6 drafts deleted.',
      '1 proposal deleted, with its candidate references and comments.',
      '7 retained exports and View exports deleted.',
      '2 managed backups deleted.',
      '3 managed backups rewritten without what was purged.',
    ]);
  });

  it('says its last failure in words', async () => {
    await openRecord(() =>
      ok(
        purgeOf('failed', {
          error: { code: 'unavailable', message: 'the object store did not answer' },
        }),
      ),
    );
    await screen.findByText(/Failed:/);
    const text = record().textContent ?? '';
    expect(text).toContain('Its last failure: The service is temporarily unavailable.');
    // The service's own words, quoted (`<q>`).
    expect(text).toContain('The service said: the object store did not answer');
    // A record that has not completed has no counts yet, and says so.
    expect(text).toContain('Its counts are shown once it has completed.');
    expect(screen.queryByRole('heading', { name: 'What it removed' })).toBeNull();
  });

  it('is asked again while it has not finished, and no more once it has', async () => {
    vi.useFakeTimers();
    let current: Purge = purgeOf('running');
    const api = mockApi({ [GET_PURGE]: () => ok(current) });
    renderAt(`/purges/${PURGE_ID}`);
    await eventually(() => screen.getByText('Running: removing what it covers.'));
    expect(record().textContent).toContain(
      `This page asks again every ${PURGE_POLL_MS / 1000} seconds until the purge has finished.`,
    );
    expect(api.callsTo(GET_PURGE)).toHaveLength(1);
    await pass(PURGE_POLL_MS - 1);
    expect(api.callsTo(GET_PURGE)).toHaveLength(1);
    current = purgeOf('completed');
    await pass(1);
    await later(() => screen.getByText(/^Completed:/));
    expect(api.callsTo(GET_PURGE)).toHaveLength(2);
    // It has finished: nothing more is asked, however long the page stays.
    await pass(PURGE_POLL_MS * 10);
    expect(api.callsTo(GET_PURGE)).toHaveLength(2);
  });

  it('says a refusal in words, with what it allows', async () => {
    await openRecord(() => fail(403, 'forbidden', 'needs the tenant administrator'));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain(
      'Reading this purge: You do not have permission to read this purge.',
    );
    expect(alert.textContent).toContain(
      'A purge’s record is read only by an administrator of this installation.',
    );
  });

  it('asks again on a retry, and shows the record it then answers', async () => {
    const { api } = await openRecord(
      inTurn(fail(500, 'internal', 'broke'), ok(purgeOf('completed'))),
    );
    const retry = await screen.findByRole('button', { name: 'Try again' });
    await act(async () => {
      fireEvent.click(retry);
    });
    await screen.findByText(/^Completed:/);
    expect(api.callsTo(GET_PURGE)).toHaveLength(2);
  });

  it('asks nothing for an address that names no purge', async () => {
    const api = mockApi({});
    renderAt('/purges/not-a-purge');
    await screen.findByRole('heading', { level: 1 });
    expect((await screen.findByRole('alert')).textContent).toContain(
      'The address does not name a purge. Nothing was requested.',
    );
    expect(api.calls).toEqual([]);
  });
});

const itemOpener = () => screen.findByRole('button', { name: 'Purge this item…' });
const itemDialog = () => screen.findByRole('alertdialog', { name: 'Purge this item?' });

describe('purging an item (C7)', { timeout: 30_000 }, () => {
  it('is offered among the item’s own actions, bound to the item and the head; not available in this build', async () => {
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(adminWorkspace),
        [PURGE_ITEM]: () => notImplemented('purge_item'),
      }),
    );
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    const button = await itemOpener();
    button.focus();
    fireEvent.click(button);
    const dialog = await itemDialog();
    const text = dialog.textContent ?? '';
    expect(text).toContain(`The item “Meeting notes” (notes/meeting.md, ${NOTE})`);
    expect(text).toContain(`as of revision ${HEAD.slice(0, 10)}`);
    expect(text).toContain(
      'A purge removes the original bytes, derivatives, index entries and historical content of this item',
    );
    expect(text).toContain(
      'A purge never erases exports, clones or backups outside the application’s control',
    );
    expect(text).toContain(humanSessionWords('Purging an item'));
    fireEvent.click(screen.getByRole('button', { name: 'Purge the item' }));
    const alert = await within(dialog).findByRole('alert');
    expect(alert.textContent).toContain('Purging this item: Not available in this build.');
    expect(alert.textContent).toContain('Nothing was done.');
    expect(api.callsTo(PURGE_ITEM)).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    // Nothing more to try: no retry, the dialog stays, and the item is where it was.
    expect(within(dialog).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(button));
    expect(api.violations).toEqual([]);
  });

  it('stays offered in an archived workspace', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(archivedAdminWorkspace) }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    expect((await itemOpener()).getAttribute('aria-disabled')).toBe('false');
  });

  it('is not offered to a person without admin here', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace) }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('button', { name: 'Rename or move' });
    expect(screen.queryByRole('button', { name: 'Purge this item…' })).toBeNull();
  });
});
