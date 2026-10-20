/**
 * Connectors (ruling C5), on a workspace's Administration page: issue one for this workspace (read
 * by default, proposing when allowed, never review or approve), list those that read it (revoked
 * ones on request) and revoke one through the destructive dialog (ruling C3).
 *
 * The secret is shown exactly once, right after `create_connector` answers, and kept nowhere but
 * in the component that shows it: not in the query cache or the mutation cache, not in the URL,
 * not in storage, and gone after a reload. A replay answered `already_issued` says so in words. A
 * person who is not the installation's local owner is refused by the service (`forbidden`, before
 * any grant is looked up: `crates/core/src/access.rs` `check_local_owner_route`); the section then
 * reads the hosted words, not an error to retry.
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Workspace } from '../../src/api/generated/types.gen';
import { LOCAL_OWNER_WORDS } from '../../src/features/explorer/rights';
import {
  adminWorkspace,
  CREATE_CONNECTOR,
  LIST_CONNECTORS,
  REVOKE_CONNECTOR,
  SESSION,
  sessionOn,
} from './support/admin';
import { connectorService, desktop, LOCAL_OWNER_REFUSAL, SECRET } from './support/admin-connectors';
import { fail, mockApi, ok } from './support/api';
import { explorerHandlers, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  localStorage.clear();
  sessionStorage.clear();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const ARCHIVED_AT = '2026-10-18T09:00:00.000Z';
const PATHS = { create: CREATE_CONNECTOR, list: LIST_CONNECTORS, revoke: REVOKE_CONNECTOR };

const section = () => screen.findByRole('region', { name: 'Connectors' });
/** The connectors listed: the rows of the list, not the items of the secret's instructions. */
function rows(region: HTMLElement): HTMLElement[] {
  const list = within(region).queryByRole('list', { name: 'Connectors that read this workspace' });
  return list === null ? [] : within(list).queryAllByRole('listitem');
}

/** The Administration page of this workspace, served by a connector service. */
function openConnectors(
  workspace: Workspace = adminWorkspace,
  route: 'local_owner' | 'browser_session' = 'local_owner',
  service = connectorService(PATHS),
) {
  const api = mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(workspace),
      [SESSION]: () => ok(sessionOn(route)),
      ...service.handlers,
    }),
  );
  const view = renderAt(`/w/${WS}/admin`);
  return { api, service, ...view };
}

/** Name the connector and press Issue. */
async function issue(region: HTMLElement, name: string, propose = false) {
  fireEvent.change(within(region).getByRole('textbox', { name: 'Name of the connector' }), {
    target: { value: name },
  });
  if (propose)
    fireEvent.click(within(region).getByRole('checkbox', { name: 'Also allow proposing' }));
  fireEvent.click(within(region).getByRole('button', { name: 'Issue a connector' }));
}

/** Everything the app keeps: the query and mutation caches, storage and the address. */
function kept(view: ReturnType<typeof openConnectors>): string {
  return JSON.stringify([
    view.queryClient
      .getQueryCache()
      .getAll()
      .map((query) => query.state),
    view.queryClient
      .getMutationCache()
      .getAll()
      .map((mutation) => mutation.state),
    { ...localStorage },
    { ...sessionStorage },
    view.router.state.location.href,
    window.location.href,
  ]);
}

describe('connectors of this workspace (C5)', { timeout: 30_000 }, () => {
  it('states the local-owner rule, and lists the connectors that read this workspace', async () => {
    const { api } = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    expect(region.textContent).toContain(
      'Only the installation’s local owner, in their own browser on this computer, issues, lists and revokes connectors.',
    );
    expect(await within(region).findByText('You are the local owner here.')).toBeTruthy();
    const [first, second] = rows(region);
    expect(first?.textContent).toContain('Desktop app');
    expect(first?.textContent).toContain('May read; it can never verify or approve anything.');
    expect(second?.textContent).toContain('Code editor');
    expect(second?.textContent).toContain(
      'May read and propose changes; it can never verify or approve anything.',
    );
    expect(second?.textContent).toContain('It also reads 1 other workspace.');
    // The one of another workspace, and the revoked one, are not listed.
    expect(region.textContent).not.toContain('Notes tool');
    expect(region.textContent).not.toContain('Old laptop');
    expect(api.callsTo(LIST_CONNECTORS)).toEqual([{ include_revoked: false }]);
    expect(api.violations).toEqual([]);
  });

  it('issues one for this workspace and shows its secret once, kept nowhere else', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    expect(region.textContent).toContain(
      'A connector may read this workspace. With proposing allowed it may also propose changes, for a person to review; it can never verify or approve anything.',
    );
    await issue(region, 'Research agent', true);

    const secretBox = await within(region).findByRole('group', {
      name: 'The secret of “Research agent”',
    });
    expect(within(secretBox).getByRole('textbox', { name: 'Secret' })).toHaveProperty(
      'value',
      SECRET,
    );
    expect(secretBox.textContent).toContain(
      'It is shown only this once: it will not be shown again, here or anywhere.',
    );
    expect(secretBox.textContent).toContain('okf-jawn mcp --stdio');
    expect(secretBox.textContent).toContain('OKF_JAWN_TOKEN');
    expect(secretBox.textContent).toContain('Authorization: Bearer');
    // Keyboard focus moves to the secret.
    expect(document.activeElement).toBe(secretBox);

    expect(view.service.issues).toEqual([
      {
        label: 'Research agent',
        allow_propose: true,
        workspace_ids: [WS],
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    // The list is asked again and lists the new connector, without its secret.
    await waitFor(() => expect(rows(region)).toHaveLength(3));
    expect(rows(region).at(2)?.textContent).toContain('Research agent');
    expect(kept(view)).not.toContain(SECRET);

    fireEvent.click(within(secretBox).getByRole('button', { name: 'Hide the secret' }));
    expect(within(region).queryByRole('group', { name: /The secret of/ })).toBeNull();
    expect(document.body.textContent).not.toContain(SECRET);
    expect(document.activeElement).toBe(
      within(region).getByRole('button', { name: 'Issue a connector' }),
    );
    expect(view.api.violations).toEqual([]);
  });

  it('is gone after a reload: the page from scratch lists the connector, never its secret', async () => {
    const first = openConnectors();
    let region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    await issue(region, 'Research agent');
    expect(await within(region).findByRole('textbox', { name: 'Secret' })).toBeTruthy();

    // A reload: the page rendered from scratch, over the same service.
    cleanup();
    const again = openConnectors(adminWorkspace, 'local_owner', first.service);
    region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(3));
    expect(rows(region).at(2)?.textContent).toContain('Research agent');
    expect(within(region).queryByRole('textbox', { name: 'Secret' })).toBeNull();
    expect(document.body.textContent).not.toContain(SECRET);
    expect(kept(again)).not.toContain(SECRET);
    expect(again.api.violations).toEqual([]);
  });

  it('retries a failed issue under the same key, and shows the secret the service then answers', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    view.service.issueSteps.push(() => fail(500, 'internal', 'The credential store failed'));
    await issue(region, 'Research agent');
    const retry = await within(region).findByRole('button', { name: 'Try again' });
    expect(region.textContent).toContain(
      'Issuing a connector: Something went wrong in the service.',
    );
    retry.focus();
    fireEvent.click(retry);
    expect(await within(region).findByRole('textbox', { name: 'Secret' })).toHaveProperty(
      'value',
      SECRET,
    );
    const [firstKey, secondKey] = view.service.issues.map((body) => body.idempotency_key);
    expect(view.service.issues).toHaveLength(2);
    expect(secondKey).toBe(firstKey);
    expect(view.api.violations).toEqual([]);
  });

  it('says a replay answered already_issued in words: the secret is not shown again', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    // The first answer was lost after the service had issued it; the retry is a replay.
    view.service.issueSteps.push((usual) => {
      usual();
      return fail(500, 'internal', 'The answer was lost');
    });
    await issue(region, 'Research agent');
    fireEvent.click(await within(region).findByRole('button', { name: 'Try again' }));
    await waitFor(() =>
      expect(region.textContent).toContain(
        'Issuing a connector: This connector was already issued, and its secret is not shown again.',
      ),
    );
    expect(region.textContent).toContain(
      'The service issued connector aaaaaaaa under this request before, and shows a secret only once. If it was lost, revoke that connector and issue a new one.',
    );
    expect(within(region).queryByRole('textbox', { name: 'Secret' })).toBeNull();
    expect(within(region).queryByRole('button', { name: 'Try again' })).toBeNull();
    // The connector the replay names is listed.
    await waitFor(() => expect(rows(region)).toHaveLength(3));

    // Issuing again is a new request, under a new key.
    fireEvent.click(within(region).getByRole('button', { name: 'Issue a connector' }));
    expect(await within(region).findByRole('textbox', { name: 'Secret' })).toBeTruthy();
    const keys = view.service.issues.map((body) => body.idempotency_key);
    expect(keys).toHaveLength(3);
    expect(keys.at(1)).toBe(keys.at(0));
    expect(keys.at(2)).not.toBe(keys.at(0));
    expect(view.api.violations).toEqual([]);
  });

  it('reads the hosted words, not an error to retry, when the service refuses a hosted session', async () => {
    const service = connectorService(PATHS);
    service.handlers[LIST_CONNECTORS] = () => fail(403, 'forbidden', LOCAL_OWNER_REFUSAL);
    const { api } = openConnectors(adminWorkspace, 'browser_session', service);
    const region = await section();
    await waitFor(() => expect(region.textContent).toBe(`Connectors${LOCAL_OWNER_WORDS}`));
    expect(within(region).queryByRole('alert')).toBeNull();
    expect(within(region).queryByRole('button')).toBeNull();
    expect(api.callsTo(LIST_CONNECTORS)).toHaveLength(1);
    expect(api.callsTo(CREATE_CONNECTOR)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('says, before the service answers, that the session is not the local owner’s', async () => {
    const service = connectorService(PATHS);
    service.handlers[LIST_CONNECTORS] = () => new Promise(() => undefined);
    openConnectors(adminWorkspace, 'browser_session', service);
    const region = await section();
    expect(
      await within(region).findByText('You are not signed in as this installation’s local owner.'),
    ).toBeTruthy();
  });

  it('lists revoked connectors too when asked, saying when each was revoked', async () => {
    const { api } = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    fireEvent.click(within(region).getByRole('checkbox', { name: 'Show revoked connectors too' }));
    await waitFor(() => expect(rows(region)).toHaveLength(3));
    expect(api.callsTo(LIST_CONNECTORS).at(-1)).toEqual({ include_revoked: true });
    const old = rows(region).find((row) => row.textContent?.includes('Old laptop'));
    expect(old?.textContent).toContain('Revoked on');
    expect(old?.querySelector('time[datetime="2026-10-19T09:30:00.000Z"]')).toBeTruthy();
    expect(within(old ?? region).queryByRole('button')).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('revokes one through the dialog bound to it, its key reused on a retry', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    const opener = within(region).getByRole('button', { name: 'Revoke “Desktop app”…' });
    opener.focus();
    fireEvent.click(opener);
    const dialog = await screen.findByRole('alertdialog', {
      name: 'Revoke the connector “Desktop app”?',
    });
    expect(dialog.textContent).toContain(desktop.connector_id);
    expect(dialog.textContent).toContain('This cannot be undone');
    view.service.revokeSteps.push(() => fail(500, 'internal', 'The credential store failed'));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Revoke it' }));
    const retry = await within(dialog).findByRole('button', { name: 'Try again' });
    expect(dialog.textContent).toContain('Revoking the connector: Something went wrong');
    fireEvent.click(retry);
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(view.service.revocations).toEqual([
      { connector_id: desktop.connector_id, idempotency_key: expect.stringMatching(UUID) },
      {
        connector_id: desktop.connector_id,
        idempotency_key: view.service.revocations.at(0)?.idempotency_key,
      },
    ]);
    expect(
      await within(region).findByText(
        '“Desktop app” was revoked: its MCP client is refused from now on.',
      ),
    ).toBeTruthy();
    await waitFor(() => expect(rows(region)).toHaveLength(1));
    expect(rows(region).at(0)?.textContent).toContain('Code editor');
    // The button that opened the dialog is gone; focus is on the section's heading, not the body.
    expect(document.activeElement).toBe(
      within(region).getByRole('heading', { level: 3, name: 'Connectors' }),
    );
    expect(view.api.violations).toEqual([]);
  });

  it('says, once revoked ones are shown, that the connector revoked is revoked', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    fireEvent.click(within(region).getByRole('checkbox', { name: 'Show revoked connectors too' }));
    await waitFor(() => expect(rows(region)).toHaveLength(3));
    fireEvent.click(within(region).getByRole('button', { name: 'Revoke “Code editor”…' }));
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Revoke it' }));
    await waitFor(() =>
      expect(
        rows(region).find((row) => row.textContent?.includes('Code editor'))?.textContent,
      ).toContain('Revoked on'),
    );
    expect(view.service.revocations).toHaveLength(1);
    expect(view.api.violations).toEqual([]);
  });

  it('in an archived workspace, keeps issuing and listing offered, and says why revoking cannot run', async () => {
    const view = openConnectors({ ...adminWorkspace, archived_at: ARCHIVED_AT });
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    const issueButton = within(region).getByRole('button', { name: 'Issue a connector' });
    expect(issueButton.getAttribute('aria-disabled')).not.toBe('true');
    const revoke = within(region).getByRole('button', { name: 'Revoke “Desktop app”…' });
    expect(revoke.getAttribute('aria-disabled')).toBe('true');
    const why = revoke.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(why)?.textContent).toContain(
      'nothing in it can be changed until it is unarchived',
    );
    fireEvent.click(revoke);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    // Issuing runs while archived.
    await issue(region, 'Research agent');
    expect(await within(region).findByRole('textbox', { name: 'Secret' })).toBeTruthy();
    expect(view.service.revocations).toEqual([]);
    expect(view.api.violations).toEqual([]);
  });

  it('offers proposing only to a person who may propose here, and says why otherwise', async () => {
    const view = openConnectors({ ...adminWorkspace, permissions: ['read', 'admin'] });
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    expect(within(region).queryByRole('checkbox', { name: 'Also allow proposing' })).toBeNull();
    expect(region.textContent).toContain(
      'Letting a connector propose changes needs permission to propose changes in this workspace, which you do not have.',
    );
    await issue(region, 'Reader');
    expect(await within(region).findByRole('textbox', { name: 'Secret' })).toBeTruthy();
    expect(view.service.issues.at(0)?.allow_propose).toBe(false);
  });

  it('says a refusal of this workspace’s grant in its own words', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    view.service.issueSteps.push(() => ({
      status: 403,
      body: {
        code: 'forbidden',
        message: 'The owner may not read this workspace',
        field: '/workspace_ids/0',
      },
    }));
    await issue(region, 'Research agent');
    const alert = await within(region).findByRole('alert');
    expect(alert.textContent).toContain(
      'Issuing a connector: The service refused to give a connector this workspace.',
    );
    expect(alert.textContent).toContain(
      'A connector may be given only what you may do here yourself: reading, and proposing when you may propose.',
    );
    expect(region.textContent).not.toContain(LOCAL_OWNER_WORDS);
    expect(view.api.violations).toEqual([]);
  });

  it('asks for a name before issuing, and sends nothing without one', async () => {
    const view = openConnectors();
    const region = await section();
    await waitFor(() => expect(rows(region)).toHaveLength(2));
    fireEvent.click(within(region).getByRole('button', { name: 'Issue a connector' }));
    const name = within(region).getByRole('textbox', { name: 'Name of the connector' });
    expect(name.getAttribute('aria-invalid')).toBe('true');
    expect(region.textContent).toContain('Give the connector a name, such as the app it is for.');
    expect(document.activeElement).toBe(name);
    expect(view.service.issues).toEqual([]);
  });

  it('is not asked for a person who holds no admin in the workspace', async () => {
    const service = connectorService(PATHS);
    const api = mockApi(explorerHandlers({ ...service.handlers }));
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'Administration' });
    expect(screen.queryByRole('region', { name: 'Connectors' })).toBeNull();
    expect(api.callsTo(LIST_CONNECTORS)).toEqual([]);
  });
});
