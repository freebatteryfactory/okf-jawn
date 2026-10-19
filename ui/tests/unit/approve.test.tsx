/**
 * Approve, Decline and Propose: who may, the confirmation an acceptance asks through and what it
 * shows (ruling R5: "Accept and Verify each ask through a confirmation dialog that shows exactly
 * what will be accepted"), the head the workspace then shows, a refusal because the head moved,
 * an archived workspace, and a proposal made from an item's page (SPEC section 8: "An authorized
 * person can accept or decline the exact proposal shown").
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
import type { Permission, ProposalStatus, Workspace } from '../../src/api/generated/types.gen';
import { formatInstant } from '../../src/lib/instant';
import { fail, type Handler, mockApi, ok } from './support/api';
import { changeOf, DIFF, diffOf } from './support/history';
import {
  ACCEPT,
  CANDIDATE,
  CHANGES_DIGEST,
  COMMENTS,
  DECLINE,
  listOf,
  PROPOSAL,
  PROPOSAL_ID,
  PROPOSALS,
  PROPOSE,
  proposalOf,
} from './support/proposals';
import { CONFIRM, CONFIRMATION_ID, confirmationFor } from './support/reviews';
import {
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  RECEIPT,
  renderAt,
  WS,
  workspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const ACCEPTED_AT = '4'.repeat(40);

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const as = (...permissions: Permission[]): Workspace => ({ ...workspace, permissions });
const approver = as('read', 'write', 'approve');

const budgetDiff: Handler = (body) =>
  ok(
    diffOf(body, [
      changeOf('notes/meeting.md', 'The budget is draft.', 'The budget is published.'),
    ]),
  );

/** Answers for one proposal whose status the test moves, as the service would. */
function setup(overrides: Record<string, Handler> = {}, who: Workspace = approver) {
  const state: { status: ProposalStatus } = { status: 'open' };
  const api = mockApi(
    explorerHandlers({
      [OPEN]: () => ok(who),
      [PROPOSALS]: () => ok(listOf([proposalOf({ status: state.status })])),
      [PROPOSAL]: () => ok(proposalOf({ status: state.status })),
      [COMMENTS]: () => ok({ items: [], next_cursor: null }),
      [DIFF]: budgetDiff,
      [CONFIRM]: (body) => ok(confirmationFor(body)),
      [ACCEPT]: () => {
        state.status = 'accepted';
        return ok({ revision: ACCEPTED_AT, receipt_id: RECEIPT, warnings: [] });
      },
      [DECLINE]: () => {
        state.status = 'declined';
        return ok(proposalOf({ status: 'declined' }));
      },
      ...overrides,
    }),
  );
  return { api, state };
}

async function openProposal() {
  renderAt(`/w/${WS}/proposals/${PROPOSAL_ID}`);
  const page = await screen.findByRole('region', { name: 'Publish the budget' });
  await within(page).findByRole('region', { name: 'What it changes' });
  return page;
}

describe('Approve', { timeout: 30_000 }, () => {
  it('asks in a dialog that shows the proposal’s revision, its change set and what it changes, then confirms and accepts that revision on the head shown', async () => {
    const { api } = setup();
    const page = await openProposal();
    const approve = within(page).getByRole('button', { name: 'Approve…' });
    act(() => approve.focus());
    fireEvent.click(approve);
    const dialog = await screen.findByRole('alertdialog', { name: 'Accept “Publish the budget”?' });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    expect(dialog.textContent).toContain(`revision ${CANDIDATE.slice(0, 10)}`);
    expect(dialog.textContent).toContain(CHANGES_DIGEST.slice(0, 10));
    expect(dialog.textContent).toContain(
      `revision ${HEAD.slice(0, 10)}, the one this workspace shows`,
    );
    expect(dialog.textContent).toContain('Change the text and properties of an item');
    const changes = within(dialog).getByRole('region', { name: 'What it changes' });
    expect(changes.querySelector('ins')?.textContent).toBe('Added: + The budget is published.\n');
    expect(api.callsTo(CONFIRM)).toEqual([]);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Accept this proposal' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(api.callsTo(CONFIRM)).toHaveLength(1);
    expect(api.callsTo(CONFIRM).at(0)).toMatchObject({
      workspace_id: WS,
      action: 'accept_proposal',
      target: { kind: 'proposal', proposal_id: PROPOSAL_ID },
      revision: CANDIDATE,
      content_digest: CHANGES_DIGEST,
    });
    expect(api.callsTo(ACCEPT)).toHaveLength(1);
    expect(api.callsTo(ACCEPT).at(0)).toMatchObject({
      workspace_id: WS,
      proposal_id: PROPOSAL_ID,
      expected_head: HEAD,
      proposal_revision: CANDIDATE,
      confirmation_id: CONFIRMATION_ID,
    });
    // The status is the service's, read again; the head the workspace shows is the new version.
    await within(page).findByText('Accepted');
    await screen.findByText(ACCEPTED_AT.slice(0, 10), { selector: '.head-line code' });
    const said = await within(page).findByText(/^Accepted: its changes are in a new version/);
    // The actions left with the proposal no longer open; focus is on what was done.
    await waitFor(() => expect(document.activeElement).toBe(said));
    expect(within(page).queryByRole('button', { name: 'Approve…' })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  // R5 and SPEC section 8 require the exact proposed content to be inspectable before Accept.
  // These fixed limits are the existing display budgets, not expected answers from the parser.
  it('lets a person read a proposed long line past its first 2000 characters before confirming', async () => {
    const prefix = 'x'.repeat(2_000);
    const suffix = 'Transfer approval authority to the contractor.';
    const proposed = `${prefix}${suffix}`;
    const { api } = setup({
      [PROPOSAL]: () =>
        ok(
          proposalOf({
            changes: [
              {
                kind: 'edit',
                item_id: NOTE,
                body: `# Meeting notes\n\n${proposed}\n`,
                properties: { type: 'meeting-note' },
              },
            ],
          }),
        ),
      [DIFF]: (body) =>
        ok(diffOf(body, [changeOf('notes/meeting.md', 'The budget is draft.', proposed)])),
    });
    const page = await openProposal();
    fireEvent.click(await within(page).findByRole('button', { name: 'Approve…' }));
    const dialog = await screen.findByRole('alertdialog');
    const changes = within(dialog).getByRole('region', { name: 'What it changes' });
    expect(changes.textContent).toContain(prefix);
    expect(changes.textContent).not.toContain(suffix);
    expect(api.violations).toEqual([]);
    fireEvent.click(within(changes).getByRole('button', { name: 'Continue this line' }));
    expect(changes.textContent).toContain(suffix);
    fireEvent.click(within(changes).getByRole('button', { name: 'Earlier in this line' }));
    expect(changes.textContent).toContain(prefix);
    expect(changes.textContent).not.toContain(suffix);
    fireEvent.click(within(changes).getByRole('button', { name: 'Continue this line' }));
    expect(changes.textContent).toContain(suffix);
    expect(api.callsTo(CONFIRM)).toEqual([]);
    expect(api.callsTo(ACCEPT)).toEqual([]);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Accept this proposal' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(api.callsTo(CONFIRM).at(0)).toMatchObject({
      revision: CANDIDATE,
      content_digest: CHANGES_DIGEST,
    });
    expect(api.callsTo(ACCEPT)).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('lets a person read a later proposed file beyond the shared 2000-line window before confirming', async () => {
    // One removed line and 1999 added lines fill the first window. The next file's only line
    // must remain reachable, and must never be described as having no changed text.
    const rows = Array.from({ length: 1_999 }, (_, index) => `Reviewed budget item ${index + 1}.`);
    const important = 'All future spending may be authorized by the contractor.';
    const { api } = setup({
      [PROPOSAL]: () =>
        ok(
          proposalOf({
            changes: [
              {
                kind: 'edit',
                item_id: NOTE,
                body: `${rows.join('\n')}\n`,
                properties: { type: 'meeting-note' },
              },
              {
                kind: 'create',
                path: 'notes/authority.md',
                type_name: 'meeting-note',
                body: `${important}\n`,
                properties: { type: 'meeting-note' },
              },
            ],
          }),
        ),
      [DIFF]: (body) =>
        ok(
          diffOf(body, [
            {
              ...changeOf('notes/meeting.md', '', ''),
              patch: [
                'diff --git a/notes/meeting.md b/notes/meeting.md',
                '--- a/notes/meeting.md',
                '+++ b/notes/meeting.md',
                '@@ -1 +1,1999 @@',
                '-The budget is draft.',
                ...rows.map((line) => `+${line}`),
                '',
              ].join('\n'),
            },
            {
              ...changeOf('notes/authority.md', '', ''),
              kind: 'added',
              old_path: null,
              patch: [
                'diff --git a/notes/authority.md b/notes/authority.md',
                'new file mode 100644',
                '--- /dev/null',
                '+++ b/notes/authority.md',
                '@@ -0,0 +1 @@',
                `+${important}`,
                '',
              ].join('\n'),
            },
          ]),
        ),
    });
    const page = await openProposal();
    fireEvent.click(await within(page).findByRole('button', { name: 'Approve…' }));
    const dialog = await screen.findByRole('alertdialog');
    const changes = within(dialog).getByRole('region', { name: 'What it changes' });
    expect(changes.querySelectorAll('ins, del')).toHaveLength(2_000);
    expect(changes.textContent).not.toContain(important);
    expect(api.violations).toEqual([]);
    expect(changes.textContent).not.toContain('No line of its text changed.');
    fireEvent.click(within(changes).getByRole('button', { name: 'Next changed lines' }));
    expect(changes.textContent).toContain(important);
    expect(changes.querySelectorAll('ins, del')).toHaveLength(1);
    fireEvent.click(within(changes).getByRole('button', { name: 'Previous changed lines' }));
    expect(changes.querySelectorAll('ins, del')).toHaveLength(2_000);
    expect(changes.textContent).not.toContain(important);
    fireEvent.click(within(changes).getByRole('button', { name: 'Next changed lines' }));
    expect(changes.textContent).toContain(important);
    expect(api.callsTo(CONFIRM)).toEqual([]);
    expect(api.callsTo(ACCEPT)).toEqual([]);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Accept this proposal' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(api.callsTo(CONFIRM).at(0)).toMatchObject({
      revision: CANDIDATE,
      content_digest: CHANGES_DIGEST,
    });
    expect(api.callsTo(ACCEPT)).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('repeats both steps under their own keys after a failure, never asking again', async () => {
    let tries = 0;
    const { api, state } = setup({
      [ACCEPT]: () => {
        tries += 1;
        if (tries === 1) return fail(503, 'unavailable', 'busy');
        state.status = 'accepted';
        return ok({ revision: ACCEPTED_AT, receipt_id: RECEIPT, warnings: [] });
      },
    });
    const page = await openProposal();
    fireEvent.click(within(page).getByRole('button', { name: 'Approve…' }));
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Accept this proposal' }));
    fireEvent.click(await within(dialog).findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    const confirmKeys = new Set(api.callsTo(CONFIRM).map((body) => body.idempotency_key));
    const acceptKeys = api.callsTo(ACCEPT).map((body) => body.idempotency_key);
    expect(confirmKeys.size).toBe(1);
    expect(acceptKeys).toHaveLength(2);
    expect(acceptKeys.at(1)).toBe(acceptKeys.at(0));
  });

  it('says the workspace changed when the head moved, names that head and offers to show it', async () => {
    const MOVED = '5'.repeat(40);
    setup({
      [ACCEPT]: () => ({
        status: 409,
        body: {
          code: 'conflict',
          message: 'the head moved',
          field: '/expected_head',
          detail: { kind: 'stale_base', head: MOVED },
        },
      }),
    });
    const page = await openProposal();
    fireEvent.click(within(page).getByRole('button', { name: 'Approve…' }));
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Accept this proposal' }));
    const alert = await within(dialog).findByRole('alert');
    expect(alert.textContent).toContain(
      'Accepting: The workspace changed since the version this was made on.',
    );
    expect(
      within(dialog).getByRole('button', { name: `Show revision ${MOVED.slice(0, 10)}` }),
    ).toBeDefined();
  });

  it('is not offered to a person who may not approve, and the page says why', async () => {
    setup({}, as('read', 'write'));
    const page = await openProposal();
    expect(within(page).queryByRole('button', { name: 'Approve…' })).toBeNull();
    expect(within(page).queryByRole('button', { name: 'Decline…' })).toBeNull();
    expect(page.textContent).toContain(
      'Approving or declining a proposal needs permission to approve proposals in this workspace, which you do not have.',
    );
  });

  it('stays offered in an archived workspace, says why it cannot run, and opens nothing', async () => {
    const archivedAt = '2026-01-01T08:00:00.000Z';
    const { api } = setup({}, { ...approver, archived_at: archivedAt });
    const page = await openProposal();
    for (const name of ['Approve…', 'Decline…']) {
      const button = within(page).getByRole('button', { name });
      expect(button.getAttribute('aria-disabled')).toBe('true');
      expect(
        document.getElementById(button.getAttribute('aria-describedby') ?? '')?.textContent,
      ).toBe(
        `This workspace was archived on ${formatInstant(archivedAt)}: nothing in it can be changed until it is unarchived. It can still be read.`,
      );
      fireEvent.click(button);
    }
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(within(page).queryByRole('textbox', { name: 'Why it is declined' })).toBeNull();
    expect(api.callsTo(CONFIRM)).toEqual([]);
  });
});

describe('Decline', { timeout: 30_000 }, () => {
  it('asks for the reason in place, sends nothing without one, then declines with it', async () => {
    const { api } = setup();
    const page = await openProposal();
    fireEvent.click(within(page).getByRole('button', { name: 'Decline…' }));
    const reason = within(page).getByRole('textbox', { name: 'Why it is declined' });
    expect(document.activeElement).toBe(reason);
    fireEvent.click(within(page).getByRole('button', { name: 'Decline' }));
    await within(page).findByText('Say why it is declined first.');
    expect(api.callsTo(DECLINE)).toEqual([]);
    fireEvent.change(reason, { target: { value: 'The board has not met yet.' } });
    fireEvent.click(within(page).getByRole('button', { name: 'Decline' }));
    await within(page).findByText('Declined');
    expect(api.callsTo(DECLINE)).toHaveLength(1);
    expect(api.callsTo(DECLINE).at(0)).toMatchObject({
      workspace_id: WS,
      proposal_id: PROPOSAL_ID,
      reason: 'The board has not met yet.',
    });
    // Focus left with the form and the actions and is on what was done, never the page body.
    const said = await within(page).findByText(
      'Declined: your reason is recorded in the discussion.',
    );
    await waitFor(() => expect(document.activeElement).toBe(said));
  });

  it('gives focus back to Decline when the form is left by Escape', async () => {
    setup();
    const page = await openProposal();
    const opener = within(page).getByRole('button', { name: 'Decline…' });
    fireEvent.click(opener);
    fireEvent.keyDown(within(page).getByRole('textbox', { name: 'Why it is declined' }), {
      key: 'Escape',
    });
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });
});

describe('Propose from an item', { timeout: 30_000 }, () => {
  it('opens a proposal of the item’s new text on the head shown, echoing its properties, and links to it', async () => {
    const { api } = setup({ [PROPOSE]: () => ok(proposalOf()) }, as('read', 'propose'));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    fireEvent.click(screen.getByRole('button', { name: 'Propose a change…' }));
    const title = screen.getByRole('textbox', { name: 'Title' });
    expect(document.activeElement).toBe(title);
    const text = screen.getByRole('textbox', { name: 'The text you propose' });
    expect((text as HTMLTextAreaElement).value).toBe('# Meeting notes');
    fireEvent.change(title, { target: { value: 'Publish the budget' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Why' }), {
      target: { value: 'The board approved it on Monday.' },
    });
    fireEvent.change(text, { target: { value: '# Meeting notes\n\nThe budget is published.\n' } });
    fireEvent.click(screen.getByRole('button', { name: 'Propose' }));
    const open = await screen.findByRole('link', { name: 'Open the proposal' });
    expect(open.getAttribute('href')).toBe(`/w/${WS}/proposals/${PROPOSAL_ID}`);
    expect(api.callsTo(PROPOSE)).toHaveLength(1);
    expect(api.callsTo(PROPOSE).at(0)).toMatchObject({
      workspace_id: WS,
      base_revision: HEAD,
      title: 'Publish the budget',
      description: 'The board approved it on Monday.',
      changes: [
        {
          kind: 'edit',
          item_id: NOTE,
          body: '# Meeting notes\n\nThe budget is published.\n',
          properties: itemDocument(noteSummary, { owner: 'Ada' }).properties,
        },
      ],
    });
    expect(api.violations).toEqual([]);
  });

  it('sends no proposal without a title', async () => {
    const { api } = setup({ [PROPOSE]: () => ok(proposalOf()) }, as('read', 'propose'));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    fireEvent.click(screen.getByRole('button', { name: 'Propose a change…' }));
    fireEvent.click(screen.getByRole('button', { name: 'Propose' }));
    await screen.findByText('Give the proposal a title first.');
    expect(api.callsTo(PROPOSE)).toEqual([]);
  });

  it('is not offered on the item to a person who may not propose', async () => {
    setup({}, as('read', 'write'));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    expect(screen.queryByRole('button', { name: 'Propose a change…' })).toBeNull();
  });
});
