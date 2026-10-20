/**
 * Security events (ruling C8): a readable list of `list_events` (a workspace's, on its
 * Administration page) and `list_tenant_events` (the installation's, on its own), each kind in
 * plain words, with when, who, what it touched, what was refused (an operation, or a byte transport
 * named in words) and how many occurrences an aggregate stands for. Paged by the service's cursor
 * (`page.cursor`, from `next_cursor`): the next page is asked only when the person asks, and at
 * the end newer events are looked for after the last one listed (`after`). The other kinds of
 * event are counted in words, not listed. Each failure is said with its retry.
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Event, EventKind } from '../../src/api/generated/types.gen';
import {
  adminWorkspace,
  LIST_EVENTS,
  LIST_TENANT_EVENTS,
  SESSION,
  sessionOn,
} from './support/admin';
import { fail, fromNewerService, type Json, mockApi, ok, type Reply } from './support/api';
import { explorerHandlers, HEAD, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const AT = '2026-10-19T08:00:00.000Z';
const CONNECTOR = '77777777-7777-4777-8777-777777777777';
const JOB = '88888888-8888-4888-8888-888888888888';
const ITEM = '22222222-2222-4222-8222-222222222222';

function event(id: string, kind: EventKind, extra: Partial<Event> = {}): Event {
  return { id, kind, at: AT, ...extra };
}

const owner = { subject: 'local-owner', route: 'local_owner' as const };

/** The installation's log: one event of each security kind, and a job's progress. */
const tenantLog: Event[] = [
  event('1', 'signed_in', { actor: owner }),
  event('2', 'connector_issued', { actor: owner, connector_id: CONNECTOR }),
  event('3', 'sign_in_refused', { count: 12 }),
  event('4', 'job_updated', { job_id: JOB }),
  event('5', 'permission_denied', {
    actor: { subject: 'agent-7', route: 'mcp_delegation', client_id: 'desktop-app' },
    operation: 'list_tenant_events',
  }),
  event('6', 'permission_denied', {
    actor: { subject: 'user_02', route: 'browser_session' },
    transport: 'download_tenant_artifact',
    count: 3,
  }),
  event('7', 'connector_revoked', { actor: owner, connector_id: CONNECTOR }),
];

const page = (events: Event[], nextCursor: string | null = null): Reply =>
  ok({ events, next_cursor: nextCursor });

const installation = () =>
  screen.findByRole('region', { name: 'Security events of the installation' });
const workspaceEvents = () => screen.findByRole('region', { name: 'Security events' });
const rows = (region: HTMLElement) => within(region).getAllByRole('listitem');

function adminAt(workspace = adminWorkspace, events: (body: Json) => Reply | Promise<Reply>) {
  const api = mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(workspace),
      [LIST_EVENTS]: events,
    }),
  );
  const view = renderAt(`/w/${WS}/admin`);
  return { api, ...view };
}

describe('the installation’s security events (C8)', { timeout: 30_000 }, () => {
  it('lists each security kind in words: when, who, what it touched, what was refused, how many', async () => {
    const api = mockApi({
      [SESSION]: () => ok(sessionOn('local_owner')),
      [LIST_TENANT_EVENTS]: () => page(tenantLog),
    });
    renderAt('/admin');
    const region = await installation();
    await waitFor(() => expect(rows(region)).toHaveLength(6));
    const [signedIn, issued, refused, denied, transport, revoked] = rows(region);

    expect(signedIn?.textContent).toContain('A person signed in');
    expect(signedIn?.textContent).toContain(
      'local-owner, as the installation’s local owner, in the browser on this computer',
    );
    expect(signedIn?.querySelector('time')?.getAttribute('datetime')).toBe(AT);

    expect(issued?.textContent).toContain('A connector was issued');
    expect(issued?.textContent).toContain('connector 77777777');

    expect(refused?.textContent).toContain('A sign-in was refused');
    expect(refused?.textContent).toContain(
      '12 refused sign-ins in this minute, recorded as one event.',
    );
    expect(refused?.textContent).toContain(
      'Nobody known: the attempt was refused before anyone was identified.',
    );

    expect(denied?.textContent).toContain('A request was refused');
    expect(denied?.textContent).toContain('reading the installation’s security events');
    expect(denied?.textContent).toContain(
      'agent-7, through an MCP client (a connector or a delegated agent), with client desktop-app',
    );

    expect(transport?.textContent).toContain('downloading a backup of the installation');
    expect(transport?.textContent).toContain(
      '3 refused requests in this minute, recorded as one event.',
    );
    expect(transport?.textContent).toContain('user_02, in a browser session');

    expect(revoked?.textContent).toContain('A connector was revoked');

    // The job's progress is counted, in words, not listed.
    expect(region.textContent).toContain('1 other event: 1 job update.');
    // The first page was asked from the start of the log, and nothing more.
    expect(api.callsTo(LIST_TENANT_EVENTS)).toEqual([{ page: { limit: 50 } }]);
    expect(api.violations).toEqual([]);
  });

  it('says a refusal of the installation’s events in words, with what it allows and no retry', async () => {
    const api = mockApi({
      [SESSION]: () => ok(sessionOn('browser_session')),
      [LIST_TENANT_EVENTS]: () =>
        fail(403, 'forbidden', 'Reading installation events needs the tenant admin grant'),
    });
    renderAt('/admin');
    const region = await installation();
    const alert = await within(region).findByRole('alert');
    expect(alert.textContent).toContain(
      'Listing the installation’s security events: Only an administrator of this installation may read its security events.',
    );
    expect(alert.textContent).toContain(
      'Each workspace’s refused requests are listed on that workspace’s Administration page, for its administrators.',
    );
    expect(within(region).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(within(region).queryByRole('list')).toBeNull();
    expect(api.violations).toEqual([]);
  });
});

describe('a workspace’s security events (C8)', { timeout: 30_000 }, () => {
  it('lists the requests it refused, in words, and counts the other events of each page', async () => {
    const { api } = adminAt(adminWorkspace, () =>
      page([
        event('10', 'changed', { revision: HEAD, item_id: ITEM }),
        event('11', 'permission_denied', {
          actor: { subject: 'agent-7', route: 'mcp_delegation', client_id: 'desktop-app' },
          operation: 'purge_workspace',
          workspace_id: WS,
        }),
        event('12', 'changed', { revision: HEAD }),
        event('13', 'job_updated', { job_id: JOB }),
        event('14', 'permission_denied', {
          actor: { subject: 'user_02', route: 'browser_session' },
          transport: 'download_artifact',
          workspace_id: WS,
          count: 40,
        }),
      ]),
    );
    const region = await workspaceEvents();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    const [purge, download] = rows(region);
    expect(purge?.textContent).toContain('A request was refused');
    expect(purge?.textContent).toContain('purging a workspace');
    expect(download?.textContent).toContain('downloading an export or a backup of a workspace');
    expect(download?.textContent).toContain(
      '40 refused requests in this minute, recorded as one event.',
    );
    expect(region.textContent).toContain('3 other events: 2 content changes, 1 job update.');
    expect(api.callsTo(LIST_EVENTS)).toEqual([{ workspace_id: WS, page: { limit: 50 } }]);
    expect(api.violations).toEqual([]);
  });

  it('asks the next page by its cursor only when asked, then looks for newer events after the last', async () => {
    const answers: Reply[] = [
      page([event('20', 'permission_denied', { actor: owner, operation: 'get_item' })], '20'),
      page([event('21', 'permission_denied', { actor: owner, operation: 'read_item' })]),
      page([]),
    ];
    const { api } = adminAt(adminWorkspace, () => answers.shift() ?? page([]));
    const region = await workspaceEvents();
    await waitFor(() => expect(rows(region)).toHaveLength(1));
    const more = within(region).getByRole('button', { name: 'Show more events' });
    // Nothing more is asked until the person asks.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(api.callsTo(LIST_EVENTS)).toHaveLength(1);

    more.focus();
    fireEvent.click(more);
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    expect(api.callsTo(LIST_EVENTS).at(1)).toEqual({
      workspace_id: WS,
      page: { limit: 50, cursor: '20' },
    });
    // Focus moves to the first event the page brought.
    expect(document.activeElement).toBe(rows(region).at(1));
    expect(region.textContent).toContain(
      'That is every event recorded when the last page was read.',
    );

    const newer = within(region).getByRole('button', { name: 'Look for newer events' });
    newer.focus();
    fireEvent.click(newer);
    expect(await within(region).findByText('No newer events were recorded.')).toBeTruthy();
    expect(api.callsTo(LIST_EVENTS).at(2)).toEqual({
      workspace_id: WS,
      after: '21',
      page: { limit: 50 },
    });
    expect(document.activeElement).toBe(newer);
    expect(api.violations).toEqual([]);
  });

  it('says a failure with its retry, and lists the events once the retry is answered', async () => {
    // Not a transient failure: the query client does not ask again by itself.
    const answers: Reply[] = [fail(500, 'internal', 'The event log could not be read')];
    const { api } = adminAt(
      adminWorkspace,
      () =>
        answers.shift() ??
        page([event('30', 'permission_denied', { actor: owner, operation: 'get_item' })]),
    );
    const region = await workspaceEvents();
    const retry = await within(region).findByRole('button', { name: 'Try again' });
    expect(region.textContent).toContain('Listing this workspace’s security events');
    retry.focus();
    fireEvent.click(retry);
    await waitFor(() => expect(rows(region)).toHaveLength(1));
    expect(within(region).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(document.activeElement).not.toBe(document.body);
    expect(api.violations).toEqual([]);
  });

  it('keeps the events listed when a later page fails, and retries that page from its cursor', async () => {
    const answers: Reply[] = [
      page([event('40', 'permission_denied', { actor: owner, operation: 'get_item' })], '40'),
      fail(500, 'internal', 'The event log could not be read'),
      page([event('41', 'permission_denied', { actor: owner, operation: 'read_item' })]),
    ];
    const { api } = adminAt(adminWorkspace, () => answers.shift() ?? page([]));
    const region = await workspaceEvents();
    await waitFor(() => expect(rows(region)).toHaveLength(1));
    fireEvent.click(within(region).getByRole('button', { name: 'Show more events' }));
    const retry = await within(region).findByRole('button', { name: 'Try again' });
    expect(rows(region)).toHaveLength(1);
    fireEvent.click(retry);
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    const asked = api.callsTo(LIST_EVENTS);
    expect(asked.at(1)).toEqual({ workspace_id: WS, page: { limit: 50, cursor: '40' } });
    expect(asked.at(2)).toEqual({ workspace_id: WS, page: { limit: 50, cursor: '40' } });
    expect(api.violations).toEqual([]);
  });

  it('is listed in an archived workspace as in any other: reading its events changes nothing', async () => {
    const { api } = adminAt({ ...adminWorkspace, archived_at: '2026-10-18T09:00:00.000Z' }, () =>
      page([event('50', 'permission_denied', { actor: owner, operation: 'save_draft' })]),
    );
    const region = await workspaceEvents();
    await waitFor(() => expect(rows(region)).toHaveLength(1));
    expect(rows(region).at(0)?.textContent).toContain('saving a draft');
    expect(api.violations).toEqual([]);
  });

  it('is not asked for a person who holds no admin in the workspace', async () => {
    const api = mockApi(explorerHandlers({ [LIST_EVENTS]: () => page([]) }));
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'Administration' });
    expect(screen.queryByRole('region', { name: 'Security events' })).toBeNull();
    expect(api.callsTo(LIST_EVENTS)).toEqual([]);
  });

  it('says when no security event is among the events listed', async () => {
    const { api } = adminAt(adminWorkspace, () =>
      page([event('60', 'changed', { revision: HEAD }), event('61', fromNewerService('pinged'))]),
    );
    const region = await workspaceEvents();
    await waitFor(() =>
      expect(region.textContent).toContain('No security event is among the 2 events listed.'),
    );
    expect(region.textContent).toContain(
      '2 other events: 1 content change, 1 event of a kind this version does not know.',
    );
    // The newer service's kind is the one contract violation of this test.
    expect(api.violations).toHaveLength(1);
  });
});
