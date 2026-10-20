/**
 * The installation's Administration (rulings C1, C6 and C9): creating a workspace, a backup of the
 * installation and the installation's jobs.
 *
 * "New workspace" is reached from the workspace list, offered there by the same rule as the way
 * to the installation's Administration (admin on a listed workspace, or the local owner), and
 * leads to the form on that page with its name field focused. `create_workspace` is sent under an
 * idempotency key kept while the same values are asked again and new once the service answered;
 * the new workspace then opens. The contract names no tenant grant a client can read, so the page
 * offers each action, the service decides, and a `forbidden` is said in words.
 *
 * "Back up the installation" asks `backup_installation`, a job of the installation: it is listed
 * under "Installation jobs" (`list_tenant_jobs`, paged by its cursor), whose finished row offers
 * the archive at the installation's own path, by the artifact's rule (a person's own session).
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
import type { AccessRoute, Job } from '../../src/api/generated/types.gen';
import { humanSessionWords } from '../../src/features/explorer/rights';
import {
  adminWorkspace,
  BACKUP_INSTALLATION,
  CREATE_WORKSPACE,
  LIST_TENANT_EVENTS,
  LIST_TENANT_JOBS,
  SESSION,
  sessionOn,
  WORKSPACES,
  workspacesPage,
} from './support/admin';
import {
  ARCHIVE_SHA,
  ARTIFACT,
  CREATED,
  forbidden,
  INSTALLATION_JOB,
  installationJobOf,
  keyedJobs,
  keyedWorkspaces,
  lost,
} from './support/admin-backups';
import { type Handler, type Json, mockApi, ok } from './support/api';
import { explorerHandlers, renderAt, workspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** A tenant job of index `n`, for a listing of several pages. */
function listedJob(n: number): Job {
  return installationJobOf('succeeded', {
    id: `${String(n).padStart(8, '0')}-7777-4777-8777-777777777777`,
    artifact: null,
  });
}

interface Options {
  route?: AccessRoute;
  /** The pages `list_tenant_jobs` answers, by cursor (`''` for the first). */
  pages?: Record<string, { items: Job[]; next_cursor: string | null }>;
  overrides?: Record<string, Handler>;
}

/** The installation's Administration, with its services answering. */
async function openInstallation({ route = 'local_owner', pages, overrides = {} }: Options = {}) {
  const backups = keyedJobs(() => installationJobOf('queued'));
  const created = keyedWorkspaces();
  const api = mockApi(
    explorerHandlers({
      [SESSION]: () => ok(sessionOn(route)),
      [WORKSPACES]: () => ok(workspacesPage([workspace])),
      [CREATE_WORKSPACE]: (body) => created.answer(body),
      [OPEN]: (body) => {
        const made = [...created.byKey.values()].find((one) => one.id === body.workspace_id);
        return ok(made ?? workspace);
      },
      [BACKUP_INSTALLATION]: (body) => backups.answer(body),
      [LIST_TENANT_JOBS]: (body) => {
        const page = body.page as { cursor?: string | null };
        if (pages !== undefined) {
          return ok(pages[page.cursor ?? ''] ?? { items: [], next_cursor: null });
        }
        // Once a backup was accepted, the service has finished it.
        const listed = backups.byKey.size > 0 ? [installationJobOf('succeeded')] : [];
        return ok({ items: listed, next_cursor: null });
      },
      [LIST_TENANT_EVENTS]: () => ok({ events: [], next_cursor: null }),
      ...overrides,
    }),
  );
  const app = renderAt('/admin');
  await screen.findByRole('heading', { level: 1, name: 'Administration of this installation' });
  return { api, backups, created, ...app };
}

const region = (name: string) => screen.getByRole('region', { name });
const createButton = () =>
  within(region('New workspace')).getByRole('button', {
    name: /^(Create the workspace|Creating the workspace…)$/,
  });

/** Fill in the form and press Create. */
function create(name: string, description: string) {
  const form = region('New workspace');
  fireEvent.change(within(form).getByRole('textbox', { name: 'Name' }), {
    target: { value: name },
  });
  fireEvent.change(within(form).getByRole('textbox', { name: 'Description' }), {
    target: { value: description },
  });
  fireEvent.click(createButton());
}

describe('“New workspace” from the workspace list (C9)', { timeout: 30_000 }, () => {
  it('is offered to the local owner and leads to the form, its name field focused', async () => {
    const api = mockApi({
      [WORKSPACES]: () => ok(workspacesPage([])),
      [SESSION]: () => ok(sessionOn('local_owner')),
      [LIST_TENANT_JOBS]: () => ok({ items: [], next_cursor: null }),
    });
    const { router } = renderAt('/');
    const link = await screen.findByRole('link', { name: 'New workspace' });
    expect(link.getAttribute('href')).toBe('/admin#new-workspace');
    link.focus();
    fireEvent.click(link);
    const name = await screen.findByRole('textbox', { name: 'Name' });
    expect(router.state.location.pathname).toBe('/admin');
    expect(router.state.location.hash).toBe('new-workspace');
    await waitFor(() => expect(document.activeElement).toBe(name));
    expect(api.violations).toEqual([]);
  });

  it('is offered to a person who holds admin on a listed workspace', async () => {
    mockApi({
      [WORKSPACES]: () => ok(workspacesPage([workspace, adminWorkspace])),
      [SESSION]: () => ok(sessionOn('browser_session')),
    });
    renderAt('/');
    expect(await screen.findByRole('link', { name: 'New workspace' })).toBeTruthy();
  });

  it('is not offered to anyone else', async () => {
    const api = mockApi({
      [WORKSPACES]: () => ok(workspacesPage([workspace])),
      [SESSION]: () => ok(sessionOn('browser_session')),
    });
    renderAt('/');
    expect(await screen.findByRole('link', { name: workspace.name })).toBeTruthy();
    await waitFor(() => expect(api.callsTo(SESSION)).toHaveLength(1));
    await act(() => Promise.resolve());
    expect(screen.queryByRole('link', { name: 'New workspace' })).toBeNull();
  });
});

describe('creating a workspace (C9)', { timeout: 30_000 }, () => {
  it('says what it makes and who decides', async () => {
    await openInstallation();
    const form = region('New workspace');
    expect(within(form).getByRole('heading', { level: 2, name: 'New workspace' })).toBeTruthy();
    expect(form.textContent).toContain('no example content');
    expect(form.textContent).toContain('needs administrator rights for this installation');
  });

  it('creates it under a key and opens it', async () => {
    const { api, router } = await openInstallation();
    create('Field notes', 'Notes from the field');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${CREATED}`));
    expect(api.callsTo(CREATE_WORKSPACE)).toEqual([
      {
        name: 'Field notes',
        description: 'Notes from the field',
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    expect(await screen.findByRole('heading', { level: 1, name: 'Field notes' })).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('asks for a name before anything is sent', async () => {
    const { api } = await openInstallation();
    create('  ', 'No name');
    const name = within(region('New workspace')).getByRole('textbox', { name: 'Name' });
    expect(
      await within(region('New workspace')).findByText('Give the workspace a name.'),
    ).toBeTruthy();
    expect(name.getAttribute('aria-invalid')).toBe('true');
    expect(document.activeElement).toBe(name);
    expect(api.callsTo(CREATE_WORKSPACE)).toEqual([]);
  });

  it('repeats a request whose answer was lost under its key; other values are a new request', async () => {
    const { api, created, router } = await openInstallation();
    created.script.push(
      () => lost(),
      () => lost(),
    );
    create('Field notes', '');
    const form = region('New workspace');
    await within(form).findByText(
      'Creating the workspace: The service is temporarily unavailable.',
    );
    fireEvent.click(within(form).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(api.callsTo(CREATE_WORKSPACE)).toHaveLength(2));
    await within(form).findByText(
      'Creating the workspace: The service is temporarily unavailable.',
    );
    // Another name is another workspace: a new key.
    create('Field notes 2026', '');
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${CREATED}`));
    const keys = api.callsTo(CREATE_WORKSPACE).map((body: Json) => body.idempotency_key);
    expect(keys).toHaveLength(3);
    expect(keys[1]).toBe(keys[0]);
    expect(keys[2]).not.toBe(keys[0]);
  });

  it('says a forbidden in words, with what it allows', async () => {
    const { created, router } = await openInstallation({ route: 'browser_session' });
    created.script.push(() => forbidden());
    create('Field notes', '');
    const form = region('New workspace');
    await within(form).findByText(
      'Creating the workspace: Creating a workspace needs administrator rights for this installation, which the service says you do not have.',
    );
    expect(form.textContent).toContain(
      'Ask an administrator of this installation to create it for you, or to give you those rights.',
    );
    expect(within(form).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(router.state.location.pathname).toBe('/admin');
  });
});

describe('a backup of the installation (C6)', { timeout: 30_000 }, () => {
  it('is a job of the installation, listed below, whose finished row offers the archive at its own path', async () => {
    const { api } = await openInstallation();
    const section = region('Backup of the installation');
    expect(section.textContent).toContain(humanSessionWords('Backing up the installation'));
    expect(section.textContent).toContain('no workspace content');
    await within(region('Installation jobs')).findByText('This installation has no jobs yet.');
    fireEvent.click(within(section).getByRole('button', { name: 'Back up the installation' }));
    await within(section).findByText(
      'The backup of the installation was accepted as Backup of the installation, job dddddddd: “waiting to start” when the service answered. Its row under “Installation jobs” shows how it stands now, and offers the download once it has finished.',
    );
    expect(api.callsTo(BACKUP_INSTALLATION)).toEqual([
      { idempotency_key: expect.stringMatching(UUID) },
    ]);
    const jobs = region('Installation jobs');
    const link = await within(jobs).findByRole('link', {
      name: 'Download the backup of the installation',
    });
    expect(link.getAttribute('href')).toBe(`/api/artifacts/${ARTIFACT}`);
    expect(link.hasAttribute('download')).toBe(true);
    const row = document.querySelector<HTMLElement>(`li[data-job="${INSTALLATION_JOB}"]`);
    expect(row?.textContent).toContain('Backup of the installation, job dddddddd');
    expect(row?.textContent).toContain('Finished.');
    expect(row?.textContent).toContain(`SHA-256: ${ARCHIVE_SHA}`);
    // A job of the installation is controlled by no workspace: nothing offers to cancel it here.
    expect(within(jobs).queryByRole('button', { name: 'Cancel' })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('repeats a request whose answer was lost under its key', async () => {
    const { api, backups } = await openInstallation();
    backups.script.push(() => lost());
    const section = region('Backup of the installation');
    fireEvent.click(within(section).getByRole('button', { name: 'Back up the installation' }));
    await within(section).findByText(
      'Backing up the installation: The service is temporarily unavailable.',
    );
    fireEvent.click(within(section).getByRole('button', { name: 'Try again' }));
    await within(section).findByText(/^The backup of the installation was accepted/);
    const keys = api.callsTo(BACKUP_INSTALLATION).map((body: Json) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
  });

  it('says a forbidden in words, with what it allows', async () => {
    const { backups } = await openInstallation({ route: 'browser_session' });
    backups.script.push(() => forbidden());
    const section = region('Backup of the installation');
    fireEvent.click(within(section).getByRole('button', { name: 'Back up the installation' }));
    await within(section).findByText(
      'Backing up the installation: The service did not allow this backup.',
    );
    expect(section.textContent).toContain(
      'A backup of the installation is made only for an administrator of this installation, in their own browser session: not for an agent, a connector or a service.',
    );
  });

  it('offers no download on a service route, and says whose it is', async () => {
    await openInstallation({
      route: 'service',
      pages: { '': { items: [installationJobOf('succeeded')], next_cursor: null } },
    });
    const jobs = region('Installation jobs');
    await within(jobs).findByText(
      'Only a signed-in browser or the local owner of this installation may download a backup of the installation; this session is a service identity.',
    );
    expect(within(jobs).queryByRole('link', { name: /^Download/ })).toBeNull();
  });
});

describe('the installation’s jobs (C1)', { timeout: 30_000 }, () => {
  it('are paged by the service’s cursor, and focus goes to the first job a page brought', async () => {
    const { api } = await openInstallation({
      pages: {
        '': { items: [listedJob(1), listedJob(2)], next_cursor: 'second-page' },
        'second-page': { items: [listedJob(3)], next_cursor: null },
      },
    });
    const jobs = region('Installation jobs');
    await within(jobs).findByText('Backup of the installation, job 00000001');
    const more = within(jobs).getByRole('button', { name: 'Show more installation jobs' });
    more.focus();
    fireEvent.click(more);
    await within(jobs).findByText('Backup of the installation, job 00000003');
    expect(api.callsTo(LIST_TENANT_JOBS).map((body) => body.page)).toEqual([
      { limit: 50, cursor: null },
      { limit: 50, cursor: 'second-page' },
    ]);
    expect(within(jobs).queryByRole('button', { name: 'Show more installation jobs' })).toBeNull();
    await waitFor(() =>
      expect(document.activeElement?.getAttribute('data-job')).toBe(listedJob(3).id),
    );
  });

  it('says a refused listing in words', async () => {
    await openInstallation({ overrides: { [LIST_TENANT_JOBS]: () => forbidden() } });
    await within(region('Installation jobs')).findByText(
      'Listing the installation’s jobs: Listing the installation’s jobs needs administrator rights for this installation, which the service says you do not have.',
    );
  });
});
