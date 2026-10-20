/**
 * A workspace's backups and restore (ruling C6), in its Administration.
 *
 * "Back up this workspace" asks `backup_workspace`, which answers 202 with the job it queued: the
 * job is told to the cache (the Jobs page lists it) and followed in the section, where its row,
 * once finished, offers the archive by the artifact's own rule: `admin` in a person's own session,
 * never on a service or an agent's route. A restore takes an upload slot for the chosen archive
 * (`create_upload`), sends its bytes through the upload transport, finishes it (`complete_upload`)
 * and only then asks, in the destructive dialog (ruling C3), whether to restore this workspace
 * from it (`restore_workspace`); its key is kept through a retry. The finished restore's
 * `RestoreReport` is said in words. Both are said to be a human session's alone, and a refusal
 * the service answers is said in words with what it allows. A backup runs in an archived
 * workspace; a restore does not, and says why.
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
import type { AccessRoute, Job, Workspace } from '../../src/api/generated/types.gen';
import { humanSessionWords } from '../../src/features/explorer/rights';
import {
  adminWorkspace,
  BACKUP_WORKSPACE,
  RESTORE_WORKSPACE,
  SESSION,
  sessionOn,
} from './support/admin';
import {
  ARCHIVE_SHA,
  ARTIFACT,
  BACKUP_JOB,
  backupJobOf,
  forbidden,
  keyedJobs,
  lost,
  notBlank,
  RESTORE_JOB,
  report,
  restoreJobOf,
} from './support/admin-backups';
import { fail, type Handler, type Json, mockApi, ok } from './support/api';
import {
  COMPLETE,
  CREATE,
  fileOf,
  GET_UPLOAD,
  JOB,
  JOBS,
  SHA_ABC,
  type UploadService,
  uploadService,
} from './support/imports';
import { mockTransfers } from './support/transfers';
import { explorerHandlers, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const ARCHIVED_AT = '2026-01-02T08:00:00.000Z';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const BACKUP_PATH = `/api/workspaces/${WS}/artifacts/${ARTIFACT}`;

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

interface Options {
  who?: Workspace;
  route?: AccessRoute;
  /** What `get_job` answers for each job, by id; the job as accepted when it names none. */
  jobs?: Record<string, Job>;
  uploads?: UploadService;
  overrides?: Record<string, Handler>;
}

/** The workspace's Administration, with the backup and restore services answering. */
async function openBackups({
  who = adminWorkspace,
  route = 'browser_session',
  jobs = {},
  uploads = uploadService(),
  overrides = {},
}: Options = {}) {
  const backups = keyedJobs((_body, index) =>
    backupJobOf('queued', { id: `${'bcdef'.charAt(index).repeat(8)}-7777-4777-8777-777777777777` }),
  );
  const restores = keyedJobs(() => restoreJobOf('queued'));
  const accepted = () => [...backups.byKey.values(), ...restores.byKey.values()];
  const api = mockApi(
    explorerHandlers({
      [OPEN]: () => ok(who),
      [SESSION]: () => ok(sessionOn(route)),
      [BACKUP_WORKSPACE]: (body) => backups.answer(body),
      [RESTORE_WORKSPACE]: (body) => restores.answer(body),
      [JOB]: (body) => {
        const id = String(body.job_id);
        const known = jobs[id] ?? accepted().find((job) => job.id === id);
        return known === undefined ? fail(404, 'not_found', 'no such job') : ok(known);
      },
      [JOBS]: () => ok({ items: accepted().reverse(), next_cursor: null }),
      ...uploads.handlers,
      ...overrides,
    }),
  );
  const sent = mockTransfers(uploads.transfer);
  const app = renderAt(`/w/${WS}/admin`);
  const section = await screen.findByRole('region', { name: 'Backups' });
  return { api, sent, backups, restores, uploads, section, ...app };
}

const backupButton = (section: HTMLElement) =>
  within(section).getByRole('button', {
    name: /^(Back up this workspace|Starting the backup…)$/,
  });
const restorePart = () => screen.getByRole('region', { name: 'Restore from a backup' });
const rowOf = (id: string) => document.querySelector<HTMLElement>(`li[data-job="${id}"]`);

/** What an element's `aria-describedby` names, as one text. */
function descriptionOf(element: HTMLElement): string {
  return (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ')
    .trim();
}

/**
 * Choose an archive as a person does: press the button, which opens the browser's picker, and
 * pick. The picker itself is the browser's; here it is the input the button opened.
 */
function chooseArchive(file: File) {
  const opened: HTMLInputElement[] = [];
  const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function open(
    this: HTMLInputElement,
  ) {
    opened.push(this);
  });
  fireEvent.click(within(restorePart()).getByRole('button', { name: 'Choose a backup archive…' }));
  click.mockRestore();
  const picker = opened.at(0);
  if (picker === undefined) throw new Error('"Choose a backup archive…" opened no picker');
  Object.defineProperty(picker, 'files', { configurable: true, value: [file] });
  fireEvent.change(picker);
  return picker;
}

/** Choose `abc.zip` (the bytes "abc") and upload it; resolves once the service confirmed it. */
async function uploadArchive() {
  chooseArchive(fileOf('workshop-backup.zip', 'abc'));
  await within(restorePart()).findByText('Chosen: workshop-backup.zip, 3 bytes.');
  fireEvent.click(within(restorePart()).getByRole('button', { name: 'Upload the archive' }));
  await within(restorePart()).findByText(
    /^Uploaded, and the service checked its length and SHA-256/,
  );
}

const dialog = () =>
  screen.getByRole('alertdialog', { name: 'Restore this workspace from this archive?' });

/** The dialog has closed: the page behind it is in the accessibility tree again. */
const closed = () => waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());

describe('backing up a workspace (C6)', { timeout: 30_000 }, () => {
  it('is offered to an administrator, with the human-session rule in words', async () => {
    const { section, api } = await openBackups();
    expect(within(section).getByRole('heading', { level: 3, name: 'Backups' })).toBeTruthy();
    expect(backupButton(section).getAttribute('aria-disabled')).toBe('false');
    expect(section.textContent).toContain(humanSessionWords('Backing up this workspace'));
    expect(section.textContent).toContain('drafts included');
    expect(api.callsTo(BACKUP_WORKSPACE)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('is not there for a person without admin here', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace) }));
    renderAt(`/w/${WS}/admin`);
    await screen.findByText(
      'Administering this workspace needs administrator rights here, which you do not have.',
    );
    expect(screen.queryByRole('region', { name: 'Backups' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Back up this workspace' })).toBeNull();
  });

  it('starts a job that the Jobs page lists, and follows it here until its archive is offered', async () => {
    const { section, api, backups, router } = await openBackups({
      jobs: { [BACKUP_JOB]: backupJobOf('succeeded') },
    });
    fireEvent.click(backupButton(section));
    await within(section).findByText(
      'The backup was accepted as Backup of the workspace, job bbbbbbbb: “waiting to start” when the service answered. Its row below shows how it stands now; once finished, it offers the download.',
    );
    expect(api.callsTo(BACKUP_WORKSPACE)).toEqual([
      { workspace_id: WS, idempotency_key: expect.stringMatching(UUID) },
    ]);
    expect(backups.byKey.size).toBe(1);
    // Asked again at once, the service has finished it: the row offers the archive.
    const link = await within(section).findByRole('link', {
      name: 'Download the backup of the workspace',
    });
    expect(link.getAttribute('href')).toBe(BACKUP_PATH);
    expect(link.hasAttribute('download')).toBe(true);
    expect(rowOf(BACKUP_JOB)?.textContent).toContain(`SHA-256: ${ARCHIVE_SHA}`);
    expect(section.textContent).toContain('It left nothing out of the archive.');
    // Every job of the workspace is on its Jobs page, this one among them.
    const all = within(section).getByRole('link', { name: 'All jobs of this workspace' });
    expect(all.getAttribute('href')).toBe(`/w/${WS}/jobs`);
    await act(() => router.navigate({ to: '/w/$workspaceId/jobs', params: { workspaceId: WS } }));
    await screen.findByRole('heading', { name: 'Jobs' });
    await waitFor(() =>
      expect(rowOf(BACKUP_JOB)?.textContent).toContain('Backup of the workspace'),
    );
    expect(api.violations).toEqual([]);
  });

  it('repeats a request whose answer was lost under its key; a press after an answer is a new backup', async () => {
    const { section, api, backups } = await openBackups();
    backups.script.push(() => lost());
    fireEvent.click(backupButton(section));
    await within(section).findByText(
      'Backing up this workspace: The service is temporarily unavailable.',
    );
    expect(section.textContent).not.toContain('accepted');
    fireEvent.click(within(section).getByRole('button', { name: 'Try again' }));
    await within(section).findByText(
      /^The backup was accepted as Backup of the workspace, job bbbbbbbb/,
    );
    fireEvent.click(backupButton(section));
    await within(section).findByText(
      /^The backup was accepted as Backup of the workspace, job cccccccc/,
    );
    const keys = api.callsTo(BACKUP_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(3);
    expect(keys[1]).toBe(keys[0]);
    expect(keys[2]).not.toBe(keys[0]);
    expect(backups.byKey.size).toBe(2);
  });

  it('says a refusal the service answers in words, with what it allows', async () => {
    const { section, backups } = await openBackups();
    backups.script.push(() => forbidden());
    fireEvent.click(backupButton(section));
    await within(section).findByText(
      'Backing up this workspace: The service did not allow this backup.',
    );
    expect(section.textContent).toContain(
      'A backup is made only for an administrator of this workspace, in their own browser session: not for an agent, a connector or a service.',
    );
    expect(within(section).queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('stays offered, and runs, in an archived workspace', async () => {
    const { section, api } = await openBackups({
      who: { ...adminWorkspace, archived_at: ARCHIVED_AT },
    });
    expect(backupButton(section).getAttribute('aria-disabled')).toBe('false');
    fireEvent.click(backupButton(section));
    await within(section).findByText(/^The backup was accepted/);
    expect(api.callsTo(BACKUP_WORKSPACE)).toHaveLength(1);
  });

  it.each([
    ['service', 'a service identity'],
    ['mcp_delegation', 'an agent connected through MCP'],
  ] as const)('offers no download on a %s route, and says whose it is', async (route, words) => {
    const { section } = await openBackups({
      route,
      jobs: { [BACKUP_JOB]: backupJobOf('succeeded') },
    });
    fireEvent.click(backupButton(section));
    await within(section).findByText(
      `Only a signed-in browser or the local owner of this installation may download a backup of the workspace; this session is ${words}.`,
    );
    expect(within(section).queryByRole('link', { name: /^Download/ })).toBeNull();
  });

  it('offers the download to the local owner', async () => {
    const { section } = await openBackups({
      route: 'local_owner',
      jobs: { [BACKUP_JOB]: backupJobOf('succeeded', { backup: { rows_left_out: 2 } }) },
    });
    fireEvent.click(backupButton(section));
    expect(
      await within(section).findByRole('link', { name: 'Download the backup of the workspace' }),
    ).toBeTruthy();
    expect(section.textContent).toContain(
      'It left 2 records out of the archive: they named a managed backup or a written history.',
    );
  });
});

describe('restoring a workspace from a backup (C6, C3)', { timeout: 30_000 }, () => {
  it('says what a restore needs, the human-session rule among it', async () => {
    await openBackups();
    const part = restorePart();
    expect(within(part).getByRole('heading', { level: 4 })).toBeTruthy();
    expect(part.textContent).toContain('This workspace must be blank');
    expect(part.textContent).toContain(humanSessionWords('Restoring a workspace'));
    // Nothing is offered to upload before an archive is chosen.
    expect(within(part).queryByRole('button', { name: 'Upload the archive' })).toBeNull();
  });

  it('takes an upload slot, sends the bytes and finishes them, then asks in the dialog', async () => {
    const { api, sent, uploads } = await openBackups();
    await uploadArchive();
    expect(api.callsTo(CREATE)).toEqual([
      {
        workspace_id: WS,
        filename: 'workshop-backup.zip',
        relative_path: '',
        size: '3',
        sha256: SHA_ABC,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    const slot = uploads.slots.at(0);
    expect(sent.transfers.map((transfer) => [transfer.method, transfer.path])).toEqual([
      ['PUT', `/api/workspaces/${WS}/uploads/${slot?.id}/content`],
    ]);
    expect(await uploads.held(slot)).toBe('abc');
    expect(api.callsTo(COMPLETE)).toEqual([
      {
        workspace_id: WS,
        upload_id: slot?.id,
        sha256: SHA_ABC,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    expect(restorePart().textContent).toContain(SHA_ABC);
    // Nothing is restored before the dialog is answered.
    expect(api.callsTo(RESTORE_WORKSPACE)).toEqual([]);

    const opener = within(restorePart()).getByRole('button', {
      name: 'Restore this workspace from it…',
    });
    fireEvent.click(opener);
    const asked = dialog();
    expect(asked.textContent).toContain('workshop-backup.zip');
    expect(asked.textContent).toContain('3 bytes');
    expect(asked.textContent).toContain(SHA_ABC);
    expect(asked.textContent).toContain(adminWorkspace.name);
    expect(asked.textContent).toContain('under the same key');
    await waitFor(() =>
      expect(document.activeElement).toBe(within(asked).getByRole('button', { name: 'Cancel' })),
    );
    fireEvent.click(within(asked).getByRole('button', { name: 'Restore the workspace' }));
    await closed();
    await within(restorePart()).findByText(
      'The restore was accepted as Restore of the workspace, job cccccccc.',
    );
    // How it stands, as the service now reports it, in a status that says each change.
    expect(
      within(restorePart())
        .getByText('Restore of the workspace, job cccccccc: waiting to start.')
        .getAttribute('role'),
    ).toBe('status');
    expect(api.callsTo(RESTORE_WORKSPACE)).toEqual([
      {
        workspace_id: WS,
        upload_id: slot?.id,
        sha256: SHA_ABC,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(opener));
    expect(api.violations).toEqual([]);
    expect(sent.violations).toEqual([]);
  });

  it('closes without sending anything when cancelled, and focus goes back', async () => {
    const { api } = await openBackups();
    await uploadArchive();
    const opener = within(restorePart()).getByRole('button', {
      name: 'Restore this workspace from it…',
    });
    fireEvent.click(opener);
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
    expect(api.callsTo(RESTORE_WORKSPACE)).toEqual([]);
  });

  it('repeats a restore whose answer was lost under the same key, from the dialog and after reopening it', async () => {
    const { api, restores } = await openBackups();
    await uploadArchive();
    restores.script.push(
      () => lost(),
      () => lost(),
    );
    const opener = within(restorePart()).getByRole('button', {
      name: 'Restore this workspace from it…',
    });
    fireEvent.click(opener);
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Restore the workspace' }));
    await within(dialog()).findByText(
      'Restoring this workspace: The service is temporarily unavailable.',
    );
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(api.callsTo(RESTORE_WORKSPACE)).toHaveLength(2));
    await within(dialog()).findByText(
      'Restoring this workspace: The service is temporarily unavailable.',
    );
    // Closed and opened again on the same archive: still the same request.
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    fireEvent.click(opener);
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Restore the workspace' }));
    await closed();
    await within(restorePart()).findByText(/^The restore was accepted as Restore of the workspace/);
    const keys = api.callsTo(RESTORE_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(3);
    expect(new Set(keys).size).toBe(1);
    expect(restores.byKey.size).toBe(1);
  });

  it('says in words that the workspace is not blank, when the service refuses so', async () => {
    const { restores } = await openBackups();
    await uploadArchive();
    restores.script.push(() => notBlank());
    fireEvent.click(
      within(restorePart()).getByRole('button', { name: 'Restore this workspace from it…' }),
    );
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Restore the workspace' }));
    await within(dialog()).findByText('Restoring this workspace: This workspace is not blank.');
    expect(dialog().textContent).toContain(
      'A restore fills only a workspace that holds nothing yet',
    );
    expect(dialog().textContent).toContain('it holds items');
    expect(within(dialog()).queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('says a forbidden in words, with what it allows', async () => {
    const { restores } = await openBackups();
    await uploadArchive();
    restores.script.push(() => forbidden());
    fireEvent.click(
      within(restorePart()).getByRole('button', { name: 'Restore this workspace from it…' }),
    );
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Restore the workspace' }));
    await within(dialog()).findByText(
      'Restoring this workspace: The service did not allow this restore.',
    );
    expect(dialog().textContent).toContain(
      'A restore is done only for an administrator of this workspace, in their own browser session: not for an agent, a connector or a service.',
    );
  });

  it('says the finished restore’s report in words', async () => {
    const { section } = await openBackups({
      jobs: {
        [RESTORE_JOB]: restoreJobOf('succeeded', { restore: report, revision: 'f'.repeat(40) }),
      },
    });
    await uploadArchive();
    fireEvent.click(
      within(restorePart()).getByRole('button', { name: 'Restore this workspace from it…' }),
    );
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Restore the workspace' }));
    await closed();
    await within(restorePart()).findByText('Restore of the workspace, job cccccccc: finished.');
    const part = restorePart();
    expect(part.textContent).toContain('It restored 12 items.');
    expect(part.textContent).toContain('3 drafts were given back to their editors.');
    expect(part.textContent).toContain(
      '1 draft stays in the archive, unassigned: its editor is not a known identity here.',
    );
    expect(part.textContent).toContain(
      '2 records were not restored: the archive’s own backup had left them out.',
    );
    expect(part.textContent).toContain('press Refresh');
    expect(section.textContent).not.toContain('undefined');
  });

  it('says why a restore that failed did, and where it can be tried again', async () => {
    await openBackups({
      jobs: {
        [RESTORE_JOB]: restoreJobOf('failed', {
          error: { code: 'invalid_input', message: 'the archive is not a workspace backup' },
        }),
      },
    });
    await uploadArchive();
    fireEvent.click(
      within(restorePart()).getByRole('button', { name: 'Restore this workspace from it…' }),
    );
    fireEvent.click(within(dialog()).getByRole('button', { name: 'Restore the workspace' }));
    await closed();
    await within(restorePart()).findByText('Restore of the workspace, job cccccccc: failed.');
    expect(restorePart().textContent).toContain('the archive is not a workspace backup');
    expect(restorePart().textContent).toContain(
      'It can be tried again from its row on the Jobs page.',
    );
  });

  it('tries an upload that failed again on the same slot under the same keys', async () => {
    const uploads = uploadService();
    // The connection breaks while the bytes go.
    uploads.script.put.push((transfer) => {
      transfer.breakConnection();
      return undefined;
    });
    const { api } = await openBackups({ uploads });
    chooseArchive(fileOf('workshop-backup.zip', 'abc'));
    fireEvent.click(
      await within(restorePart()).findByRole('button', { name: 'Upload the archive' }),
    );
    await within(restorePart()).findByText(
      'Uploading the archive: The workspace service could not be reached.',
    );
    expect(restorePart().textContent).not.toContain('Uploaded, and');
    fireEvent.click(within(restorePart()).getByRole('button', { name: 'Try again' }));
    await within(restorePart()).findByText(/^Uploaded, and the service checked/);
    const slots = api.callsTo(CREATE).map((body) => body.idempotency_key);
    expect(slots).toHaveLength(2);
    expect(slots[1]).toBe(slots[0]);
    expect(uploads.slots).toHaveLength(1);
    // The slot is read as it stands before the rest is sent.
    expect(api.callsTo(GET_UPLOAD)).toHaveLength(1);
    expect(await uploads.held(uploads.slots.at(0))).toBe('abc');
  });

  it('stays visible in an archived workspace, says why it cannot run, and sends nothing', async () => {
    const { api } = await openBackups({ who: { ...adminWorkspace, archived_at: ARCHIVED_AT } });
    const choose = within(restorePart()).getByRole('button', { name: 'Choose a backup archive…' });
    expect(choose.getAttribute('aria-disabled')).toBe('true');
    expect(descriptionOf(choose)).toContain('archived');
    expect(descriptionOf(choose)).toContain('A restore cannot run in an archived workspace');
    const click = vi.spyOn(HTMLInputElement.prototype, 'click');
    fireEvent.click(choose);
    expect(click).not.toHaveBeenCalled();
    expect(api.callsTo(CREATE)).toEqual([]);
    expect(api.callsTo(RESTORE_WORKSPACE)).toEqual([]);
  });

  it('keeps the slot request out of a second store: a new archive is a new slot', async () => {
    const { api } = await openBackups();
    await uploadArchive();
    chooseArchive(fileOf('other-backup.zip', 'abcd'));
    await within(restorePart()).findByText('Chosen: other-backup.zip, 4 bytes.');
    expect(
      within(restorePart()).queryByRole('button', { name: 'Restore this workspace from it…' }),
    ).toBeNull();
    fireEvent.click(within(restorePart()).getByRole('button', { name: 'Upload the archive' }));
    await within(restorePart()).findByText(/^Uploaded, and the service checked/);
    const keys = api.callsTo(CREATE).map((body: Json) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).not.toBe(keys[0]);
  });
});
