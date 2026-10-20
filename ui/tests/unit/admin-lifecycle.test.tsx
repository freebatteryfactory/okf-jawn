/**
 * This workspace's details and lifecycle, on its Administration page (rulings C1, C2 and C9):
 * its name and description, changed through `update_workspace` (`write`, on the head the page
 * shows, a conflict said with a way to reload it); "Rebuild the search index" (`rebuild_index`,
 * `admin`, a job the Jobs page lists); Archive and Unarchive (`admin`; the shell then shows the
 * workspace as the service now has it). Each is offered by the person's grant; in an archived
 * workspace the actions the service runs there stay offered, and every other stays visible, says
 * why it cannot run and sends nothing.
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
import { getJobOptions } from '../../src/api/generated/@tanstack/react-query.gen';
import type { Workspace } from '../../src/api/generated/types.gen';
import { archivedWords } from '../../src/features/explorer/rights';
import { formatInstant } from '../../src/lib/instant';
import {
  ARCHIVE_WORKSPACE,
  adminWorkspace,
  REBUILD_INDEX,
  UNARCHIVE_WORKSPACE,
  UPDATE_WORKSPACE,
} from './support/admin';
import {
  ARCHIVED_AT,
  acceptedRebuild,
  adminWithoutWrite,
  archivedAdminWorkspace,
  mutationAt,
  NEWER,
  REBUILD_JOB,
  rebuildJob,
} from './support/admin-lifecycle';
import { fail, type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import { explorerHandlers, HEAD, renderAt, WS } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/**
 * The service's workspace, as `open_workspace` answers it each time it is asked: what the test
 * last made of it.
 */
function workspaceService(first: Workspace) {
  let current = first;
  return {
    set: (next: Workspace) => {
      current = next;
    },
    get: () => current,
    open: (): Reply => ok(current),
  };
}

async function openAdmin(
  who: Workspace,
  handlers: Record<string, Handler> = {},
): Promise<ReturnType<typeof renderAt> & { api: ReturnType<typeof mockApi> }> {
  const service = workspaceService(who);
  const api = mockApi(explorerHandlers({ [OPEN]: service.open, ...handlers }));
  const app = renderAt(`/w/${WS}/admin`);
  await screen.findByRole('region', { name: 'Administration' });
  return { api, ...app };
}

const lifecycle = () => screen.getByRole('region', { name: 'This workspace' });
const nameBox = () => within(lifecycle()).getByRole('textbox', { name: 'Name' });
const descriptionBox = () => within(lifecycle()).getByRole('textbox', { name: 'Description' });
const saveButton = () =>
  within(lifecycle()).getByRole('button', { name: /^(Save the name and description|Saving…)$/ });
const rebuildButton = () =>
  within(lifecycle()).getByRole('button', {
    name: /^(Rebuild the search index|Starting the rebuild…)$/,
  });
const archiveButton = () =>
  within(lifecycle()).getByRole('button', {
    name: /^(Archive this workspace|Unarchive this workspace|Archiving…|Unarchiving…)$/,
  });
const purgeButton = () =>
  within(lifecycle()).getByRole('button', { name: 'Purge this workspace…' });

/** What an element's `aria-describedby` names, as one text. */
function descriptionOf(element: HTMLElement): string {
  return (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ')
    .trim();
}

describe('this workspace’s name and description (C9)', { timeout: 30_000 }, () => {
  it('shows them, and saves them on the head the page shows, said "as of"; the shell then shows them', async () => {
    const service = workspaceService(adminWorkspace);
    const api = mockApi(
      explorerHandlers({
        [OPEN]: service.open,
        [UPDATE_WORKSPACE]: (body: Json) => {
          service.set({
            ...service.get(),
            name: String(body.name),
            description: String(body.description),
          });
          return ok(mutationAt(HEAD));
        },
      }),
    );
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'This workspace' });
    expect(nameBox()).toHaveProperty('value', 'Workshop notes');
    expect(descriptionBox()).toHaveProperty('value', 'Notes from the workshop');
    expect(descriptionOf(saveButton())).toContain(
      `as of revision ${HEAD.slice(0, 10)}, the version this page shows`,
    );
    fireEvent.change(nameBox(), { target: { value: 'Workshop 2026' } });
    fireEvent.change(descriptionBox(), { target: { value: 'What the October workshop found' } });
    const opened = api.callsTo(OPEN).length;
    saveButton().focus();
    fireEvent.click(saveButton());
    await within(lifecycle()).findByText('The name and description were saved.');
    expect(api.callsTo(UPDATE_WORKSPACE)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        name: 'Workshop 2026',
        description: 'What the October workshop found',
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    // The shell reads the workspace again, and shows it as the service now has it.
    await waitFor(() => expect(api.callsTo(OPEN).length).toBeGreaterThan(opened));
    expect(await screen.findByRole('heading', { level: 1, name: 'Workshop 2026' })).toBeTruthy();
    expect(nameBox()).toHaveProperty('value', 'Workshop 2026');
    expect(document.activeElement).toBe(saveButton());
    expect(api.violations).toEqual([]);
  });

  it('says a conflict in words, with a way to reload the workspace, and asks again on what it then shows', async () => {
    const service = workspaceService(adminWorkspace);
    const api = mockApi(
      explorerHandlers({
        [OPEN]: service.open,
        [UPDATE_WORKSPACE]: (body: Json) =>
          body.base_revision === HEAD
            ? {
                status: 409,
                body: {
                  code: 'conflict',
                  message: 'the workspace head moved',
                  field: '/base_revision',
                  detail: { kind: 'stale_base', head: NEWER },
                },
              }
            : ok(mutationAt(NEWER)),
      }),
    );
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'This workspace' });
    fireEvent.change(nameBox(), { target: { value: 'Workshop 2026' } });
    fireEvent.click(saveButton());
    const alert = await within(lifecycle()).findByRole('alert');
    expect(alert.textContent).toContain('Saving the name and description: nothing was changed.');
    expect(alert.textContent).toContain(
      `The workspace changed since this page showed it: it now holds revision ${NEWER.slice(0, 10)}.`,
    );
    // Someone else made the newer version; reloading shows the workspace as it is now.
    service.set({ ...adminWorkspace, head: NEWER, description: 'Described elsewhere' });
    const reload = within(lifecycle()).getByRole('button', { name: 'Reload the workspace' });
    reload.focus();
    fireEvent.click(reload);
    await within(lifecycle()).findByText(
      'The workspace was refreshed. Nothing was changed yet: check this form, then press its button again.',
    );
    expect(within(lifecycle()).queryByRole('alert')).toBeNull();
    // The notice left with keyboard focus in it: focus is on Save, never on the page body.
    await waitFor(() => expect(document.activeElement).toBe(saveButton()));
    // What the person typed stays; what they did not touch is the workspace's as it is now.
    expect(nameBox()).toHaveProperty('value', 'Workshop 2026');
    expect(descriptionBox()).toHaveProperty('value', 'Described elsewhere');
    expect(descriptionOf(saveButton())).toContain(`as of revision ${NEWER.slice(0, 10)}`);
    fireEvent.click(saveButton());
    await within(lifecycle()).findByText('The name and description were saved.');
    const [first, second] = api.callsTo(UPDATE_WORKSPACE);
    expect(second?.base_revision).toBe(NEWER);
    expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
    expect(api.violations).toEqual([]);
  });

  it('repeats a failure that may pass under the same key', async () => {
    let answers: Reply[] = [fail(503, 'unavailable', 'busy'), ok(mutationAt(HEAD))];
    const { api } = await openAdmin(adminWorkspace, {
      [UPDATE_WORKSPACE]: () => {
        const [next, ...rest] = answers;
        answers = rest;
        return next ?? ok(mutationAt(HEAD));
      },
    });
    fireEvent.change(nameBox(), { target: { value: 'Workshop 2026' } });
    fireEvent.click(saveButton());
    const retry = await within(lifecycle()).findByRole('button', { name: 'Try again' });
    await act(async () => {
      fireEvent.click(retry);
    });
    await within(lifecycle()).findByText('The name and description were saved.');
    const keys = api.callsTo(UPDATE_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
  });

  it('asks for a name before sending anything', async () => {
    const { api } = await openAdmin(adminWorkspace, {
      [UPDATE_WORKSPACE]: () => ok(mutationAt(HEAD)),
    });
    fireEvent.change(nameBox(), { target: { value: '   ' } });
    fireEvent.click(saveButton());
    expect(await within(lifecycle()).findByText('Give the workspace a name.')).toBeTruthy();
    expect(nameBox().getAttribute('aria-invalid')).toBe('true');
    expect(api.callsTo(UPDATE_WORKSPACE)).toEqual([]);
  });

  it('is not offered to a person who may not write here: the page says why', async () => {
    const { api } = await openAdmin(adminWithoutWrite);
    await screen.findByRole('region', { name: 'This workspace' });
    expect(within(lifecycle()).queryByRole('textbox')).toBeNull();
    expect(lifecycle().textContent).toContain('Workshop notes');
    expect(lifecycle().textContent).toContain(
      'Changing this workspace’s name and description needs permission to change this workspace, which you do not have.',
    );
    expect(api.callsTo(UPDATE_WORKSPACE)).toEqual([]);
  });

  it('in an archived workspace stays visible, says why it cannot run, and sends nothing', async () => {
    const { api } = await openAdmin(archivedAdminWorkspace, {
      [UPDATE_WORKSPACE]: () => ok(mutationAt(HEAD)),
    });
    await screen.findByRole('region', { name: 'This workspace' });
    expect(saveButton().getAttribute('aria-disabled')).toBe('true');
    expect(descriptionOf(saveButton())).toContain(archivedWords(ARCHIVED_AT));
    fireEvent.click(saveButton());
    expect(api.callsTo(UPDATE_WORKSPACE)).toEqual([]);
  });
});

describe('rebuilding the search index (C9)', { timeout: 30_000 }, () => {
  it('starts a rebuild, a job the cache holds so the Jobs page lists it', async () => {
    const { api, queryClient } = await openAdmin(adminWorkspace, {
      [REBUILD_INDEX]: () => acceptedRebuild(),
    });
    await screen.findByRole('region', { name: 'This workspace' });
    expect(descriptionOf(rebuildButton())).toContain('reviews and receipts are kept');
    fireEvent.click(rebuildButton());
    await within(lifecycle()).findByText(
      'The rebuild was accepted as Rebuild of the search index, job 88888888: “waiting to start” when the service answered. The Jobs page lists it.',
    );
    expect(api.callsTo(REBUILD_INDEX)).toEqual([
      { workspace_id: WS, idempotency_key: expect.stringMatching(UUID) },
    ]);
    const held = queryClient.getQueryData(
      getJobOptions({ body: { workspace_id: WS, job_id: REBUILD_JOB } }).queryKey,
    );
    expect(held).toEqual(rebuildJob);
    expect(within(lifecycle()).getByRole('link', { name: 'Open the Jobs page' })).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('repeats a failed request under its key, and a new rebuild once one was answered', async () => {
    let answers: Reply[] = [fail(503, 'unavailable', 'busy')];
    const { api } = await openAdmin(adminWorkspace, {
      [REBUILD_INDEX]: () => {
        const [next, ...rest] = answers;
        answers = rest;
        return next ?? acceptedRebuild();
      },
    });
    await screen.findByRole('region', { name: 'This workspace' });
    fireEvent.click(rebuildButton());
    const retry = await within(lifecycle()).findByRole('button', { name: 'Try again' });
    await act(async () => {
      fireEvent.click(retry);
    });
    await within(lifecycle()).findByText(/The rebuild was accepted/);
    fireEvent.click(rebuildButton());
    await waitFor(() => expect(api.callsTo(REBUILD_INDEX)).toHaveLength(3));
    const [first, second, third] = api.callsTo(REBUILD_INDEX).map((body) => body.idempotency_key);
    expect(second).toBe(first);
    expect(third).not.toBe(first);
  });
});

describe('archiving and unarchiving (C9)', { timeout: 30_000 }, () => {
  it('archives on the head the page shows; the shell then shows it archived, and the same button unarchives', async () => {
    const service = workspaceService(adminWorkspace);
    const api = mockApi(
      explorerHandlers({
        [OPEN]: service.open,
        [ARCHIVE_WORKSPACE]: () => {
          service.set({ ...service.get(), archived_at: ARCHIVED_AT });
          return ok(mutationAt(HEAD));
        },
      }),
    );
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'This workspace' });
    const button = archiveButton();
    expect(button.textContent).toBe('Archive this workspace');
    expect(descriptionOf(button)).toContain('Archiving is reversible and keeps the history.');
    button.focus();
    fireEvent.click(button);
    await within(lifecycle()).findByText(
      'Archived. Nothing in it can be changed until it is unarchived; it can still be read, exported and backed up.',
    );
    expect(api.callsTo(ARCHIVE_WORKSPACE)).toEqual([
      { workspace_id: WS, base_revision: HEAD, idempotency_key: expect.stringMatching(UUID) },
    ]);
    // The shell read the workspace again: it is archived, and says so where actions are offered.
    await waitFor(() => expect(archiveButton().textContent).toBe('Unarchive this workspace'));
    expect(archiveButton()).toBe(button);
    expect(document.activeElement).toBe(button);
    expect(descriptionOf(button)).toContain(
      `This workspace was archived on ${formatInstant(ARCHIVED_AT)}.`,
    );
    // Unarchiving is one of what the service runs in an archived workspace; a change is not.
    expect(button.getAttribute('aria-disabled')).toBe('false');
    expect(saveButton().getAttribute('aria-disabled')).toBe('true');
    expect(descriptionOf(saveButton())).toContain(archivedWords(ARCHIVED_AT));
    expect(api.violations).toEqual([]);
  });

  it('unarchives an archived workspace; the shell then shows it can be changed again', async () => {
    const service = workspaceService(archivedAdminWorkspace);
    const api = mockApi(
      explorerHandlers({
        [OPEN]: service.open,
        [UNARCHIVE_WORKSPACE]: () => {
          service.set({ ...service.get(), archived_at: null });
          return ok(mutationAt(HEAD));
        },
      }),
    );
    renderAt(`/w/${WS}/admin`);
    await screen.findByRole('region', { name: 'This workspace' });
    expect(archiveButton().textContent).toBe('Unarchive this workspace');
    expect(archiveButton().getAttribute('aria-disabled')).toBe('false');
    fireEvent.click(archiveButton());
    await within(lifecycle()).findByText('Unarchived. It can be changed again.');
    expect(api.callsTo(UNARCHIVE_WORKSPACE)).toEqual([
      { workspace_id: WS, idempotency_key: expect.stringMatching(UUID) },
    ]);
    await waitFor(() => expect(archiveButton().textContent).toBe('Archive this workspace'));
    expect(saveButton().getAttribute('aria-disabled')).toBe('false');
    expect(api.violations).toEqual([]);
  });

  it('says the service’s refusal of an archive in words: nothing was changed', async () => {
    const { api } = await openAdmin(adminWorkspace, {
      [ARCHIVE_WORKSPACE]: () =>
        fail(409, 'conflict', 'an import of the workspace is queued or running'),
    });
    await screen.findByRole('region', { name: 'This workspace' });
    fireEvent.click(archiveButton());
    const alert = await within(lifecycle()).findByRole('alert');
    expect(alert.textContent).toContain('Archiving this workspace: nothing was changed.');
    // The head had not moved: what the service holds now is the reason.
    await waitFor(() =>
      expect(alert.textContent).toContain(
        'A workspace is not archived while an import, a conversion or a restore of it is waiting or running.',
      ),
    );
    expect(archiveButton().textContent).toBe('Archive this workspace');
    expect(api.violations).toEqual([]);
  });
});

describe('who is offered this workspace’s lifecycle (C1, C2)', { timeout: 30_000 }, () => {
  it('a person without admin here is offered none of it', async () => {
    const { api } = await openAdmin({
      ...adminWorkspace,
      permissions: ['read', 'write'],
    });
    expect(screen.queryByRole('region', { name: 'This workspace' })).toBeNull();
    for (const name of [
      'Rebuild the search index',
      'Archive this workspace',
      'Purge this workspace…',
      'Save the name and description',
    ]) {
      expect(screen.queryByRole('button', { name })).toBeNull();
    }
    expect(api.callsTo(REBUILD_INDEX)).toEqual([]);
  });

  it('an administrator is offered each of them', async () => {
    await openAdmin(adminWorkspace);
    await screen.findByRole('region', { name: 'This workspace' });
    for (const button of [saveButton(), rebuildButton(), archiveButton(), purgeButton()]) {
      expect(button.getAttribute('aria-disabled')).toBe('false');
    }
  });

  it('in an archived workspace, what the service runs there stays offered, and runs', async () => {
    const { api } = await openAdmin(archivedAdminWorkspace, {
      [REBUILD_INDEX]: () => acceptedRebuild(),
    });
    await screen.findByRole('region', { name: 'This workspace' });
    for (const button of [rebuildButton(), archiveButton(), purgeButton()]) {
      expect(button.getAttribute('aria-disabled')).toBe('false');
      expect(descriptionOf(button)).not.toContain(archivedWords(ARCHIVED_AT));
    }
    fireEvent.click(rebuildButton());
    await within(lifecycle()).findByText(/The rebuild was accepted/);
    expect(api.callsTo(REBUILD_INDEX)).toHaveLength(1);
  });
});
