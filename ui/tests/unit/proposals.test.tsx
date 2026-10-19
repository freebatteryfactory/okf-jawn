/**
 * Proposals: the list, one proposal with what it changes, its discussion, and both kept current
 * by the workspace's events (SPEC section 8: "Proposals carry base and proposed revisions and
 * allowed content changes"; ruling R10: the status is shown exactly as the service answers it).
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
import { COMMENTS_PAGE, PROPOSALS_PAGE } from '../../src/features/proposals/proposals';
import { formatInstant } from '../../src/lib/instant';
import { fail, type Handler, mockApi, ok } from './support/api';
import { changeOf, DIFF, diffOf } from './support/history';
import { eventOf, mockEvents } from './support/imports';
import {
  acceptedProposal,
  CANDIDATE,
  COMMENT,
  COMMENTS,
  commentOf,
  listOf,
  PROPOSAL,
  PROPOSAL_ID,
  PROPOSALS,
  PROPOSED_AT,
  proposalOf,
} from './support/proposals';
import {
  explorerHandlers,
  HEAD,
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
      [PROPOSALS]: () => ok(listOf([proposalOf(), acceptedProposal])),
      [PROPOSAL]: () => ok(proposalOf()),
      [COMMENTS]: () => ok({ items: [], next_cursor: null }),
      [DIFF]: budgetDiff,
      ...overrides,
    }),
  );
}

const openList = async () => {
  renderAt(`/w/${WS}/proposals`);
  return screen.findByRole('region', { name: 'Proposals' });
};

const openProposal = async () => {
  renderAt(`/w/${WS}/proposals/${PROPOSAL_ID}`);
  return screen.findByRole('region', { name: 'Publish the budget' });
};

describe('the list of proposals', { timeout: 30_000 }, () => {
  it('is in the workspace’s bar and lists each proposal with its status, who made it, how and when', async () => {
    const api = setup();
    renderAt(`/w/${WS}`);
    fireEvent.click(await screen.findByRole('link', { name: 'Proposals' }));
    const page = await screen.findByRole('region', { name: 'Proposals' });
    expect(screen.getByRole('heading', { level: 2, name: 'Proposals' })).toBeDefined();
    const link = await within(page).findByRole('link', { name: 'Publish the budget' });
    expect(link.getAttribute('href')).toBe(`/w/${WS}/proposals/${PROPOSAL_ID}`);
    const row = link.closest('li');
    expect(row?.textContent).toContain('Open: waiting for someone who may approve it');
    expect(row?.textContent).toContain('proposed by claude-agent');
    expect(row?.textContent).toContain('an agent connected through MCP');
    expect(row?.textContent).toContain(formatInstant(PROPOSED_AT) ?? '');
    expect(
      within(page).getByRole('link', { name: 'Add the minutes' }).closest('li')?.textContent,
    ).toContain('Accepted');
    expect(api.callsTo(PROPOSALS)).toEqual([{ workspace_id: WS, page: { limit: PROPOSALS_PAGE } }]);
    expect(api.violations).toEqual([]);
  });

  it('asks for the proposals of one status when the person chooses it', async () => {
    const api = setup();
    const page = await openList();
    await within(page).findByRole('link', { name: 'Publish the budget' });
    fireEvent.change(within(page).getByRole('combobox', { name: 'Show' }), {
      target: { value: 'open' },
    });
    await waitFor(() => expect(api.callsTo(PROPOSALS)).toHaveLength(2));
    expect(api.callsTo(PROPOSALS).at(1)).toEqual({
      workspace_id: WS,
      status: 'open',
      page: { limit: PROPOSALS_PAGE },
    });
  });

  it('says there are none when the list is empty, and why it could not be read when it fails', async () => {
    setup({ [PROPOSALS]: () => ok(listOf([])) });
    const page = await openList();
    await within(page).findByText('No proposals.');
    cleanup();
    vi.unstubAllGlobals();
    setup({ [PROPOSALS]: () => fail(501, 'not_implemented', 'not built') });
    const failed = await openList();
    const alert = await within(failed).findByRole('alert');
    expect(alert.textContent).toContain('Listing the proposals: Not available in this build.');
    expect(within(failed).queryByText('No proposals.')).toBeNull();
  });

  it('says where proposals are made, and why a person who may not propose cannot', async () => {
    setup({}, workspace);
    const page = await openList();
    await within(page).findByRole('link', { name: 'Publish the budget' });
    expect(page.textContent).toContain(
      'Proposing a change needs permission to propose changes in this workspace, which you do not have.',
    );
    cleanup();
    vi.unstubAllGlobals();
    setup({}, { ...workspace, permissions: ['read', 'propose'] });
    const allowedPage = await openList();
    expect(allowedPage.textContent).toContain('open the item and choose “Propose a change…”');
  });
});

describe('one proposal', { timeout: 30_000 }, () => {
  it('shows what it proposes in words and what it changes against the version it was made on', async () => {
    const api = setup();
    const page = await openProposal();
    expect(page.textContent).toContain('The board approved it on Monday.');
    expect(page.textContent).toContain('Open: waiting for someone who may approve it');
    expect(page.textContent).toContain(`made on revision ${HEAD.slice(0, 10)}`);
    expect(page.textContent).toContain(`proposed content is revision ${CANDIDATE.slice(0, 10)}`);
    expect(within(page).getByRole('list', { name: 'What it proposes' }).textContent).toContain(
      'Change the text and properties of an item',
    );
    const changes = await within(page).findByRole('region', { name: 'What it changes' });
    expect(changes.querySelector('ins')?.textContent).toBe('Added: + The budget is published.\n');
    expect(api.callsTo(PROPOSAL)).toEqual([{ workspace_id: WS, proposal_id: PROPOSAL_ID }]);
    expect(api.callsTo(DIFF)).toEqual([{ workspace_id: WS, from: HEAD, to: CANDIDATE }]);
  });

  it('shows a status that is not open as the service answers it, and offers no Approve or Decline', async () => {
    setup(
      {
        [PROPOSAL]: () => ok(proposalOf({ status: 'conflict' })),
      },
      { ...writableWorkspace, permissions: ['read', 'write', 'approve'] },
    );
    const page = await openProposal();
    await within(page).findByText(/^In conflict/);
    expect(within(page).queryByRole('button', { name: 'Approve…' })).toBeNull();
    expect(within(page).queryByRole('button', { name: 'Decline…' })).toBeNull();
  });
});

describe('the discussion', { timeout: 30_000 }, () => {
  it('lists the comments oldest first, page by page', async () => {
    const api = setup({
      [COMMENTS]: (body) => {
        const page = body.page as { cursor?: string | null };
        return page.cursor === 'more'
          ? ok({ items: [commentOf('c3', 'Third')], next_cursor: null })
          : ok({
              items: [commentOf('c1', 'First'), commentOf('c2', 'Second')],
              next_cursor: 'more',
            });
      },
    });
    const page = await openProposal();
    const list = await within(page).findByRole('list', { name: 'Discussion' });
    await within(list).findByText('First');
    expect(
      within(list)
        .getAllByRole('listitem')
        .map((row) => row.querySelector('p')?.textContent),
    ).toEqual(['First', 'Second']);
    fireEvent.click(within(page).getByRole('button', { name: 'Show more comments' }));
    await within(list).findByText('Third');
    expect(api.callsTo(COMMENTS)).toEqual([
      { workspace_id: WS, proposal_id: PROPOSAL_ID, page: { limit: COMMENTS_PAGE } },
      {
        workspace_id: WS,
        proposal_id: PROPOSAL_ID,
        page: { limit: COMMENTS_PAGE, cursor: 'more' },
      },
    ]);
  });

  it('adds a comment, repeating a failed one under the same key, and lists it', async () => {
    let tries = 0;
    let saved = false;
    const api = setup({
      [COMMENTS]: () =>
        ok({ items: saved ? [commentOf('c1', 'Looks right to me.')] : [], next_cursor: null }),
      [COMMENT]: () => {
        tries += 1;
        if (tries === 1) return fail(503, 'unavailable', 'busy');
        saved = true;
        return ok(commentOf('c1', 'Looks right to me.'));
      },
    });
    const page = await openProposal();
    const box = await within(page).findByRole('textbox', { name: 'Your comment' });
    fireEvent.change(box, { target: { value: 'Looks right to me.' } });
    fireEvent.click(within(page).getByRole('button', { name: 'Add the comment' }));
    fireEvent.click(await within(page).findByRole('button', { name: 'Try again' }));
    const list = await within(page).findByRole('list', { name: 'Discussion' });
    await within(list).findByText('Looks right to me.');
    const sent = api.callsTo(COMMENT);
    expect(sent).toHaveLength(2);
    expect(sent.at(0)).toMatchObject({
      workspace_id: WS,
      proposal_id: PROPOSAL_ID,
      text: 'Looks right to me.',
    });
    expect(sent.at(1)?.idempotency_key).toBe(sent.at(0)?.idempotency_key);
    // The box is emptied for the next comment.
    expect((box as HTMLTextAreaElement).value).toBe('');
  });

  it('sends no empty comment', async () => {
    const api = setup();
    const page = await openProposal();
    await within(page).findByRole('textbox', { name: 'Your comment' });
    fireEvent.click(within(page).getByRole('button', { name: 'Add the comment' }));
    await within(page).findByText('Write the comment first.');
    expect(api.callsTo(COMMENT)).toEqual([]);
  });

  it('is offered to read only, with the reason, to a person who may not change the workspace', async () => {
    setup({}, workspace);
    const page = await openProposal();
    await within(page).findByText('No comments yet.');
    expect(within(page).queryByRole('textbox', { name: 'Your comment' })).toBeNull();
    expect(page.textContent).toContain(
      'Commenting needs permission to change this workspace, which you do not have.',
    );
  });
});

describe('kept current by the workspace’s events', { timeout: 30_000 }, () => {
  it('asks again for the proposal and its comments when it changes, and for the list on any change', async () => {
    const api = setup();
    const events = mockEvents();
    const page = await openProposal();
    await within(page).findByText('No comments yet.');
    await waitFor(() => expect(events.streams).toHaveLength(1));
    // Once the stream is open, what is shown is asked again.
    await waitFor(() => expect(api.callsTo(PROPOSAL).length).toBeGreaterThanOrEqual(2));
    const proposals = api.callsTo(PROPOSAL).length;
    const comments = api.callsTo(COMMENTS).length;
    act(() =>
      events.streams
        .at(0)
        ?.send(
          '1',
          eventOf('proposal_updated', '1', { proposal_id: 'aaaaaaaa-0000-4000-8000-000000000000' }),
        ),
    );
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(api.callsTo(PROPOSAL)).toHaveLength(proposals);
    act(() =>
      events.streams
        .at(0)
        ?.send('2', eventOf('proposal_updated', '2', { proposal_id: PROPOSAL_ID })),
    );
    await waitFor(() => expect(api.callsTo(PROPOSAL)).toHaveLength(proposals + 1));
    await waitFor(() => expect(api.callsTo(COMMENTS)).toHaveLength(comments + 1));
  });

  it('shows the status the service answers after an event, and keeps the proposal when that read fails', async () => {
    let status: 'open' | 'declined' = 'open';
    let failing = false;
    setup({
      [PROPOSAL]: () => (failing ? fail(503, 'unavailable', 'busy') : ok(proposalOf({ status }))),
    });
    const events = mockEvents();
    const page = await openProposal();
    await within(page).findByText('Open: waiting for someone who may approve it');
    await waitFor(() => expect(events.streams).toHaveLength(1));
    status = 'declined';
    act(() =>
      events.streams
        .at(0)
        ?.send('1', eventOf('proposal_updated', '1', { proposal_id: PROPOSAL_ID })),
    );
    await within(page).findByText('Declined');
    failing = true;
    act(() =>
      events.streams
        .at(0)
        ?.send('2', eventOf('proposal_updated', '2', { proposal_id: PROPOSAL_ID })),
    );
    const alert = await within(page).findByRole('alert');
    expect(alert.textContent).toContain('Reading this proposal again');
    expect(within(page).getByText('Declined')).toBeDefined();
    expect(within(page).getByText('The board approved it on Monday.')).toBeDefined();
  });
});
