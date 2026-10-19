/**
 * A proposed View on its proposal's page (M4b ruling B2; SPEC section 13: "proposed
 * View/approval/render"): each change that would store a View (a create of one, or an edit of an
 * item that is a View at the proposal's base) is read with `parseViewFile` and drawn with
 * `present_view`, beside what the proposal changes and before Approve. A file the parser refuses
 * is said in words there, and Approve stays offered: the service checks it again at acceptance.
 * A refusal of `present_view` is the workspace's failure notice, with its retry where one helps.
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Permission, Workspace } from '../../src/api/generated/types.gen';
import { fail, type Handler, mockApi, ok } from './support/api';
import { DIFF, diffOf } from './support/history';
import { COMMENTS, PROPOSAL, PROPOSAL_ID, proposalOf } from './support/proposals';
import {
  addedViewChange,
  createViewChange,
  ITEM,
  PRESENT_VIEW,
  PROPOSED_PATH,
  presentationOf,
  VIEW,
  viewFileOf,
  viewHandlers,
  viewOf,
} from './support/views-explorer';
import { explorerHandlers, HEAD, NOTE, NOW, renderAt, WS, workspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const as = (...permissions: Permission[]): Workspace => ({ ...workspace, permissions });
const approver = as('read', 'write', 'approve');

/** The proposal creates the View of `viewOf()`, unless the test gives other changes. */
function setup(
  overrides: Record<string, Handler> = {},
  changes = [createViewChange()],
  who: Workspace = approver,
) {
  const base = explorerHandlers();
  return mockApi({
    ...base,
    ...viewHandlers(base),
    [OPEN]: () => ok(who),
    [PROPOSAL]: () => ok(proposalOf({ title: 'Add the revenue View', changes })),
    [COMMENTS]: () => ok({ items: [], next_cursor: null }),
    [DIFF]: (body) => ok(diffOf(body, [addedViewChange(viewFileOf().body)])),
    [PRESENT_VIEW]: () => ok(presentationOf()),
    ...overrides,
  });
}

async function openProposal() {
  renderAt(`/w/${WS}/proposals/${PROPOSAL_ID}`);
  const page = await screen.findByRole('region', { name: 'Add the revenue View' });
  const changes = await within(page).findByRole('region', { name: 'What it changes' });
  const proposed = await within(page).findByRole('region', {
    name: `Proposed View: ${PROPOSED_PATH}`,
  });
  return { page, changes, proposed };
}

/** Whether `a` comes before `b` in the page. */
const precedes = (a: Node, b: Node) =>
  (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;

describe('a proposed View', { timeout: 30_000 }, () => {
  it('is drawn with present_view beside what the proposal changes, before Approve, which stays offered', async () => {
    const api = setup();
    const { page, changes, proposed } = await openProposal();
    const table = await within(proposed).findByRole('table', { name: 'revenue' });
    expect(within(table).getAllByRole('row')).toHaveLength(4);
    // The proposed file, read here: its title, description and bindings, each in this workspace.
    expect(api.callsTo(PRESENT_VIEW)).toEqual([{ workspace_id: WS, view: viewOf() }]);
    const approve = within(page).getByRole('button', { name: 'Approve…' });
    expect(precedes(changes, proposed)).toBe(true);
    expect(precedes(proposed, approve)).toBe(true);
    await waitFor(() => expect(approve.getAttribute('aria-disabled')).toBe('false'));
    expect(api.violations).toEqual([]);
  });

  it('is drawn for an edit of an item that is a View at the base, and not for an edit of a Note', async () => {
    const revised = viewOf({ title: 'Revenue by quarter, revised' });
    const edited = viewFileOf(revised);
    const api = setup({}, [
      { kind: 'edit', item_id: VIEW, body: edited.body, properties: edited.properties },
      changeAsNote(),
    ]);
    // The View item's path at the base names its region.
    renderAt(`/w/${WS}/proposals/${PROPOSAL_ID}`);
    const page = await screen.findByRole('region', { name: 'Add the revenue View' });
    const proposed = await within(page).findByRole('region', {
      name: 'Proposed View: views/revenue.md',
    });
    await within(proposed).findByRole('table', { name: 'revenue' });
    expect(api.callsTo(PRESENT_VIEW)).toEqual([{ workspace_id: WS, view: revised }]);
    // Each edited item's kind is read at the proposal's base, as the service reads it.
    expect(api.callsTo(ITEM)).toContainEqual({
      workspace_id: WS,
      item_id: VIEW,
      at: { kind: 'revision', revision: HEAD },
    });
    expect(api.callsTo(ITEM)).toContainEqual({
      workspace_id: WS,
      item_id: NOTE,
      at: { kind: 'revision', revision: HEAD },
    });
    expect(within(page).getAllByRole('region', { name: /^Proposed View/ })).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('that does not read as a View is said in words beside the diff, and Approve stays offered', async () => {
    const api = setup({}, [
      createViewChange({ properties: { title: 'Revenue' }, body: 'Revenue grew in 2025.\n' }),
    ]);
    const { page, proposed } = await openProposal();
    const said = within(proposed).getByRole('note');
    expect(said.textContent).toContain(
      'This proposed file cannot be shown as a View: its text has no View block.',
    );
    expect(said.textContent).toContain('ends with one ```json okf-jawn-view block');
    expect(said.textContent).toContain(
      'The service checks the View again when the proposal is accepted',
    );
    expect(api.callsTo(PRESENT_VIEW)).toEqual([]);
    const approve = within(page).getByRole('button', { name: 'Approve…' });
    await waitFor(() => expect(approve.getAttribute('aria-disabled')).toBe('false'));
  });

  it('names where in its block the parser found the fault', async () => {
    const { body, properties } = viewFileOf();
    const named = body.replace('"item_id"', `"workspace_id": "${WS}",\n        "item_id"`);
    setup({}, [createViewChange({ properties, body: named })]);
    const { proposed } = await openProposal();
    const said = within(proposed).getByRole('note');
    expect(said.textContent).toContain('a part of it is not what a View holds');
    expect(said.textContent).toContain('at /bindings/0/source/workspace_id');
  });

  it('that present_view refuses is the failure notice, tried again on request', async () => {
    let answers = 0;
    const api = setup({
      [PRESENT_VIEW]: () => {
        answers += 1;
        return answers === 1 ? fail(500, 'internal', 'the renderer stopped') : ok(presentationOf());
      },
    });
    const { page, proposed } = await openProposal();
    const alert = await within(proposed).findByRole('alert');
    expect(alert.textContent).toContain(
      'Showing the proposed View: Something went wrong in the service.',
    );
    expect(alert.textContent).toContain('the renderer stopped');
    // Approve stays offered whatever the presentation answered.
    expect(within(page).getByRole('button', { name: 'Approve…' })).toBeDefined();
    fireEvent.click(within(proposed).getByRole('button', { name: 'Try again' }));
    await within(proposed).findByRole('table', { name: 'revenue' });
    expect(api.callsTo(PRESENT_VIEW)).toHaveLength(2);
    expect(within(proposed).queryByRole('alert')).toBeNull();
  });

  it('says a build without present_view in plain words, with nothing to try again', async () => {
    setup({ [PRESENT_VIEW]: () => fail(501, 'not_implemented', 'present_view is not built') });
    const { page, proposed } = await openProposal();
    const alert = await within(proposed).findByRole('alert');
    expect(alert.textContent).toContain('Showing the proposed View: Not available in this build.');
    expect(within(proposed).queryByRole('button', { name: 'Try again' })).toBeNull();
    const approve = within(page).getByRole('button', { name: 'Approve…' });
    await waitFor(() => expect(approve.getAttribute('aria-disabled')).toBe('false'));
  });

  it('is still drawn in an archived workspace, where Approve says why it cannot run', async () => {
    const api = setup({}, [createViewChange()], { ...approver, archived_at: NOW });
    const { page, proposed } = await openProposal();
    await within(proposed).findByRole('table', { name: 'revenue' });
    expect(api.callsTo(PRESENT_VIEW)).toHaveLength(1);
    const approve = within(page).getByRole('button', { name: 'Approve…' });
    expect(approve.getAttribute('aria-disabled')).toBe('true');
    expect(page.textContent).toContain('It can still be read.');
  });
});

/** An edit of the Note beside the View's. */
function changeAsNote() {
  return {
    kind: 'edit' as const,
    item_id: NOTE,
    body: '# Meeting notes\n\nThe budget is published.\n',
    properties: { type: 'meeting-note' },
  };
}
