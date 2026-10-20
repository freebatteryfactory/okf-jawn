/**
 * Starting an export (B3): "Export this workspace" on the workspace's Jobs page (`export_workspace`:
 * the workspace as it is now, its history when asked) and "Export visual" for one View
 * (`export_view`, at the revision its tab reads). Both answer 202 with a job, which the Jobs page
 * then lists as it lists every job.
 *
 * The service here keeps the rule the real one keeps for a job-starting operation: a repeat under
 * an idempotency key it has seen answers the job that key started, and starts nothing. A test
 * scripts the awkward answers, one step each: a refusal, or an answer lost after the job was made.
 */
import { QueryClientProvider } from '@tanstack/react-query';
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { getJobOptions } from '../../src/api/generated/@tanstack/react-query.gen';
import type { Job, JobKind, Workspace } from '../../src/api/generated/types.gen';
import { createQueryClient } from '../../src/app/query-client';
import type { Rights } from '../../src/features/explorer/rights';
import { ExportViewButton } from '../../src/features/exports/ExportView';
import { fail, type Json, mockApi, ok, type Reply } from './support/api';
import { EXPORT_VIEW, EXPORT_WORKSPACE } from './support/exports';
import { LOG, OLDER, VERSIONS } from './support/history';
import { accepted, JOBS, jobOf } from './support/imports';
import {
  explorerHandlers,
  HEAD,
  NOTE,
  OLD,
  renderAt,
  WS,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const ARCHIVED_AT = '2026-01-02T08:00:00.000Z';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** One scripted answer: `made` is the job the request started (or found), `usual` its 202. */
type Step = (made: Job, usual: Reply) => Reply | Promise<Reply>;

/** Export jobs as a service keeps them: one per idempotency key, newest listed first. */
function exportService() {
  const byKey = new Map<string, Job>();
  const script: Step[] = [];
  const answer = (body: Json, kind: JobKind) => {
    const key = String(body.idempotency_key);
    let made = byKey.get(key);
    if (made === undefined) {
      const letter = 'abcdef'.charAt(byKey.size);
      made = jobOf('queued', { id: `${letter.repeat(8)}-7777-4777-8777-777777777777`, kind });
      byKey.set(key, made);
    }
    const usual = accepted(made);
    const step = script.shift();
    return step === undefined ? usual : step(made, usual);
  };
  return {
    byKey,
    script,
    jobs: () => [...byKey.values()].reverse(),
    handlers: {
      [EXPORT_WORKSPACE]: (body: Json) => answer(body, 'export_workspace'),
      [EXPORT_VIEW]: (body: Json) => answer(body, 'export_view'),
      [JOBS]: () => ok({ items: [...byKey.values()].reverse(), next_cursor: null }),
    },
  };
}

/** The Jobs page of `who`'s workspace, with the export service answering. */
async function openJobs(who: Workspace = writableWorkspace) {
  const service = exportService();
  const api = mockApi(explorerHandlers({ [OPEN]: () => ok(who), ...service.handlers }));
  renderAt(`/w/${WS}/jobs`);
  await screen.findByRole('heading', { name: 'Jobs' });
  return { api, service };
}

const section = () => screen.getByRole('group', { name: 'Export this workspace' });
const exportButton = () =>
  within(section()).getByRole('button', { name: /^(Export the workspace|Starting the export…)$/ });
const historyBox = () =>
  within(section()).getByRole('checkbox', { name: 'Include the workspace’s history' });
const rowOf = (id: string) => document.querySelector<HTMLElement>(`li[data-job="${id}"]`);
const FIRST = 'aaaaaaaa-7777-4777-8777-777777777777';
const SECOND = 'bbbbbbbb-7777-4777-8777-777777777777';

/** What an element's `aria-describedby` names, as one text. */
function descriptionOf(element: HTMLElement): string {
  return (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ')
    .trim();
}

describe('"Export this workspace" on the Jobs page', () => {
  it('offers the workspace as it is now, without its history unless asked', async () => {
    await openJobs();
    await screen.findByRole('group', { name: 'Export this workspace' });
    expect(section().textContent).toContain('as it is now');
    expect(section().textContent).toContain('newer than the version this page shows');
    expect(historyBox()).toHaveProperty('checked', false);
    expect(exportButton().textContent).toBe('Export the workspace');
    expect(exportButton().getAttribute('aria-disabled')).toBe('false');
  });

  it('asks for the latest version, resolved by the service, and lists the job it answered', async () => {
    const { api } = await openJobs();
    await screen.findByText(/This workspace has no jobs yet\./);
    fireEvent.click(exportButton());
    await within(section()).findByText(
      'The export was accepted as Export of the workspace, job aaaaaaaa: “waiting to start” when the service answered. Its row in the list below shows how it stands now.',
    );
    const [sent] = api.callsTo(EXPORT_WORKSPACE);
    expect(sent).toEqual({
      workspace_id: WS,
      at: { kind: 'latest' },
      include_history: false,
      idempotency_key: expect.stringMatching(UUID),
    });
    // The page lists the job as it lists every job, in the state the service reported.
    await screen.findByText('Export of the workspace, job aaaaaaaa');
    expect(rowOf(FIRST)?.textContent).toContain('Waiting to start.');
    expect(api.violations).toEqual([]);
  });

  it('includes the history when asked', async () => {
    const { api } = await openJobs();
    fireEvent.click(
      await screen.findByRole('checkbox', { name: 'Include the workspace’s history' }),
    );
    expect(historyBox()).toHaveProperty('checked', true);
    fireEvent.click(exportButton());
    await within(section()).findByText(/^The export was accepted/);
    expect(api.callsTo(EXPORT_WORKSPACE).map((body) => body.include_history)).toEqual([true]);
  });

  it('starts one job for each press, each under a new key', async () => {
    const { api, service } = await openJobs();
    fireEvent.click(await screen.findByRole('button', { name: 'Export the workspace' }));
    await within(section()).findByText(/job aaaaaaaa: “waiting to start”/);
    fireEvent.click(exportButton());
    await screen.findByText('Export of the workspace, job bbbbbbbb', {
      selector: `li[data-job="${SECOND}"] p`,
    });
    const keys = api.callsTo(EXPORT_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(new Set(keys).size).toBe(2);
    expect(service.jobs().map((job) => job.id)).toEqual([SECOND, FIRST]);
  });

  it('repeats a request whose answer was lost under its key, so it is still one job', async () => {
    const { api, service } = await openJobs();
    // The job is made, and the answer does not reach the page.
    service.script.push(() => fail(503, 'unavailable', 'the gateway lost the answer'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export the workspace' }));
    await within(section()).findByText(
      'Starting the export: The service is temporarily unavailable.',
    );
    // Never said to be accepted.
    expect(section().textContent).not.toContain('accepted');
    const again = within(section()).getByRole('button', { name: 'Try again' });
    again.focus();
    fireEvent.click(again);
    await within(section()).findByText(/^The export was accepted as Export of the workspace/);
    const keys = api.callsTo(EXPORT_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
    expect(service.jobs()).toHaveLength(1);
    // The notice left with focus on its button: focus is on the Export button, not the body.
    expect(document.activeElement).toBe(exportButton());
  });

  it('repeats a failed request under its key when Export is pressed again with the same choice', async () => {
    const { api, service } = await openJobs();
    service.script.push(() => fail(500, 'internal', 'the job could not be recorded'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export the workspace' }));
    await within(section()).findByText('Starting the export: Something went wrong in the service.');
    fireEvent.click(exportButton());
    await within(section()).findByText(/^The export was accepted/);
    // A new choice is a new request; the one after an accepted export is new too.
    fireEvent.click(historyBox());
    fireEvent.click(exportButton());
    await screen.findByText('Export of the workspace, job bbbbbbbb', {
      selector: `li[data-job="${SECOND}"] p`,
    });
    const keys = api.callsTo(EXPORT_WORKSPACE).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(3);
    expect(keys[1]).toBe(keys[0]);
    expect(keys[2]).not.toBe(keys[0]);
    expect(service.jobs().map((job) => job.id)).toEqual([SECOND, FIRST]);
  });

  it('reads "not available in this build" where exports are not built, never as accepted', async () => {
    const { service } = await openJobs();
    service.script.push(() => fail(501, 'not_implemented', 'export_workspace is not built'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export the workspace' }));
    await within(section()).findByText('Starting the export: Not available in this build.');
    expect(section().textContent).toContain('The service said: export_workspace is not built');
    expect(section().textContent).not.toContain('accepted');
    // The failure allows no repeat.
    expect(within(section()).queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('reads a refusal in the words of its code', async () => {
    const { service } = await openJobs();
    service.script.push(() => fail(403, 'forbidden', 'write is needed'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export the workspace' }));
    await within(section()).findByText(
      'Starting the export: You do not have permission to do this.',
    );
  });

  it('keeps the button, and its focus, while the export starts, and sends nothing more', async () => {
    const { api, service } = await openJobs();
    let answer: () => void = () => {};
    service.script.push(
      (_made, usual) =>
        new Promise<Reply>((resolve) => {
          answer = () => resolve(usual);
        }),
    );
    const button = await screen.findByRole('button', { name: 'Export the workspace' });
    button.focus();
    fireEvent.click(button);
    await within(section()).findByRole('button', { name: 'Starting the export…' });
    expect(exportButton().getAttribute('aria-disabled')).toBe('true');
    expect(document.activeElement).toBe(exportButton());
    fireEvent.click(exportButton());
    act(() => answer());
    await within(section()).findByText(/^The export was accepted/);
    expect(api.callsTo(EXPORT_WORKSPACE)).toHaveLength(1);
    expect(document.activeElement).toBe(exportButton());
  });

  it('is not offered to a person who may only read, and says why', async () => {
    const { api } = await openJobs({ ...writableWorkspace, permissions: ['read'] });
    await screen.findByText(
      'Exporting this workspace needs permission to change this workspace, which you do not have.',
    );
    expect(screen.queryByRole('group', { name: 'Export this workspace' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Export the workspace' })).toBeNull();
    expect(screen.queryByRole('checkbox')).toBeNull();
    expect(api.callsTo(EXPORT_WORKSPACE)).toEqual([]);
  });

  it('stays offered, and runs, in an archived workspace', async () => {
    const { api } = await openJobs({ ...writableWorkspace, archived_at: ARCHIVED_AT });
    const button = await screen.findByRole('button', { name: 'Export the workspace' });
    expect(button.getAttribute('aria-disabled')).toBe('false');
    expect(section().textContent).not.toContain('archived');
    fireEvent.click(button);
    await within(section()).findByText(/^The export was accepted/);
    expect(api.callsTo(EXPORT_WORKSPACE)).toHaveLength(1);
  });
});

/** "Export visual" by itself, as a View's tab will hold it, inside a router of its own. */
async function renderExportView(rights: Rights, revision = OLD) {
  const service = exportService();
  const api = mockApi(service.handlers);
  const queryClient = createQueryClient();
  const router = createRouter({
    routeTree: createRootRoute({
      component: () => (
        <ExportViewButton workspaceId={WS} itemId={NOTE} revision={revision} rights={rights} />
      ),
    }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { api, service, queryClient };
}

const writer: Rights = { permissions: ['read', 'write'], archivedAt: null };

describe('"Export visual" for a View', () => {
  it('exports the View at the revision its tab reads, never the latest', async () => {
    const { api, queryClient } = await renderExportView(writer);
    fireEvent.click(await screen.findByRole('button', { name: 'Export visual' }));
    await screen.findByText(
      'The export of this View was accepted as Export of a View, job aaaaaaaa: “waiting to start” when the service answered. The Jobs page shows how it stands now.',
    );
    expect(api.callsTo(EXPORT_VIEW)).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        at: { kind: 'revision', revision: OLD },
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    // Its state is followed where every job's is.
    expect(
      screen.getByRole('link', { name: 'All jobs of this workspace' }).getAttribute('href'),
    ).toBe(`/w/${WS}/jobs`);
    // The job is in the cache as the service answered it.
    const held = queryClient.getQueryData(
      getJobOptions({ body: { workspace_id: WS, job_id: FIRST } }).queryKey,
    );
    expect(held).toMatchObject({ id: FIRST, kind: 'export_view', state: 'queued' });
    expect(api.violations).toEqual([]);
  });

  it('says what it exports, and at which revision', async () => {
    await renderExportView(writer);
    const button = await screen.findByRole('button', { name: 'Export visual' });
    expect(descriptionOf(button)).toBe(
      `Exports this View as of revision ${OLD.slice(0, 10)}: its specification, its sources, its data table and what it needs to be drawn. The export is a job; the Jobs page shows how it goes.`,
    );
  });

  it('repeats a request whose answer was lost under its key, so it is still one job', async () => {
    const { api, service } = await renderExportView(writer);
    service.script.push(() => fail(503, 'unavailable', 'the gateway lost the answer'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export visual' }));
    await screen.findByText(
      'Starting the export of this View: The service is temporarily unavailable.',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await screen.findByText(/^The export of this View was accepted/);
    const keys = api.callsTo(EXPORT_VIEW).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
    expect(service.jobs()).toHaveLength(1);
  });

  it('starts a new job for a press after an accepted one', async () => {
    const { api, service } = await renderExportView(writer);
    fireEvent.click(await screen.findByRole('button', { name: 'Export visual' }));
    await screen.findByText(/job aaaaaaaa: “waiting to start”/);
    fireEvent.click(screen.getByRole('button', { name: 'Export visual' }));
    await screen.findByText(/job bbbbbbbb: “waiting to start”/);
    const keys = api.callsTo(EXPORT_VIEW).map((body) => body.idempotency_key);
    expect(keys).toHaveLength(2);
    expect(new Set(keys).size).toBe(2);
    expect(service.jobs().map((job) => job.id)).toEqual([SECOND, FIRST]);
  });

  it('reads "not available in this build" where View exports are not built', async () => {
    const { service } = await renderExportView(writer);
    service.script.push(() => fail(501, 'not_implemented', 'export_view is not built'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export visual' }));
    await screen.findByText('Starting the export of this View: Not available in this build.');
    expect(screen.queryByText(/accepted/)).toBeNull();
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('is not offered to a person who may only read, and says why', async () => {
    const { api } = await renderExportView({ permissions: ['read'], archivedAt: null });
    await screen.findByText(
      'Exporting this View needs permission to change this workspace, which you do not have.',
    );
    expect(screen.queryByRole('button')).toBeNull();
    expect(api.callsTo(EXPORT_VIEW)).toEqual([]);
  });

  it('stays offered, and runs, in an archived workspace', async () => {
    const { api } = await renderExportView({ ...writer, archivedAt: ARCHIVED_AT });
    const button = await screen.findByRole('button', { name: 'Export visual' });
    expect(button.getAttribute('aria-disabled')).toBe('false');
    fireEvent.click(button);
    await screen.findByText(/^The export of this View was accepted/);
    expect(api.callsTo(EXPORT_VIEW)).toHaveLength(1);
  });
});

/**
 * "Export visual" for one View whose tab moves to another revision, as a tab keyed by its item
 * does (`DocumentPanes`): the control stays mounted and is given the new revision.
 */
async function renderMovingView() {
  const service = exportService();
  const api = mockApi(service.handlers);
  const queryClient = createQueryClient();
  let moveTo: (revision: string) => void = () => {};
  function MovingTab() {
    const [revision, setRevision] = useState(OLD);
    moveTo = setRevision;
    return <ExportViewButton workspaceId={WS} itemId={NOTE} revision={revision} rights={writer} />;
  }
  const router = createRouter({
    routeTree: createRootRoute({ component: MovingTab }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  await screen.findByRole('button', { name: 'Export visual' });
  return { api, service, move: (revision: string) => act(() => moveTo(revision)) };
}

const viewButton = () =>
  screen.getByRole('button', { name: /^(Export visual|Starting the export…)$/ });
const unavailable = () => fail(503, 'unavailable', 'the gateway lost the answer');
const revisionsSent = (api: { callsTo: (path: string) => Json[] }) =>
  api.callsTo(EXPORT_VIEW).map((body) => (body.at as { revision: string }).revision);

describe('"Export visual" is about the revision its tab reads now', () => {
  it('neither shows nor retries a failure at the revision before, and keeps keyboard focus', async () => {
    const { api, service, move } = await renderMovingView();
    service.script.push(unavailable);
    fireEvent.click(viewButton());
    await screen.findByText(
      'Starting the export of this View: The service is temporarily unavailable.',
    );
    screen.getByRole('button', { name: 'Try again' }).focus();
    move(HEAD);
    expect(screen.queryByText(/temporarily unavailable/)).toBeNull();
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    // Focus that was in the notice is on the control the notice belonged to, never on the body.
    expect(document.activeElement).toBe(viewButton());
    expect(descriptionOf(viewButton())).toContain(`as of revision ${HEAD.slice(0, 10)}`);
    fireEvent.click(viewButton());
    await screen.findByText(/^The export of this View was accepted/);
    expect(revisionsSent(api)).toEqual([OLD, HEAD]);
    const keys = api.callsTo(EXPORT_VIEW).map((body) => body.idempotency_key);
    expect(keys[1]).not.toBe(keys[0]);
  });

  it('does not say of the next revision an export held at the one before, answered after it moved', async () => {
    const { api, service, move } = await renderMovingView();
    let release: () => void = () => {};
    service.script.push(
      (_made, usual) =>
        new Promise<Reply>((resolve) => {
          release = () => resolve(usual);
        }),
    );
    fireEvent.click(viewButton());
    await screen.findByRole('button', { name: 'Starting the export…' });
    move(HEAD);
    expect(viewButton().textContent).toBe('Export visual');
    expect(viewButton().getAttribute('aria-disabled')).toBe('false');
    expect(screen.queryByText('Starting the export…')).toBeNull();
    await act(async () => {
      release();
      await new Promise((settled) => setTimeout(settled, 50));
    });
    expect(screen.queryByText(/accepted/)).toBeNull();
    expect(screen.queryByRole('link', { name: 'All jobs of this workspace' })).toBeNull();
    // The export at the revision before still became a job: the Jobs page lists it.
    expect(service.jobs().map((job) => job.id)).toEqual([FIRST]);
    fireEvent.click(viewButton());
    await screen.findByText(/job bbbbbbbb: “waiting to start”/);
    expect(revisionsSent(api)).toEqual([OLD, HEAD]);
  });

  it('does not say an export accepted at the revision before is the next one’s', async () => {
    const { move } = await renderMovingView();
    fireEvent.click(viewButton());
    await screen.findByText(/job aaaaaaaa: “waiting to start”/);
    move(HEAD);
    expect(screen.queryByText(/accepted/)).toBeNull();
    expect(screen.queryByRole('link', { name: 'All jobs of this workspace' })).toBeNull();
  });

  it('asks again under the key a revision had, when the tab comes back to it unanswered', async () => {
    const { api, service, move } = await renderMovingView();
    service.script.push(unavailable, unavailable);
    fireEvent.click(viewButton());
    await screen.findByText(/temporarily unavailable/);
    move(HEAD);
    fireEvent.click(viewButton());
    await screen.findByText(/temporarily unavailable/);
    move(OLD);
    fireEvent.click(viewButton());
    await screen.findByText(/^The export of this View was accepted/);
    expect(revisionsSent(api)).toEqual([OLD, HEAD, OLD]);
    const keys = api.callsTo(EXPORT_VIEW).map((body) => body.idempotency_key);
    expect(keys[2]).toBe(keys[0]);
    expect(keys[1]).not.toBe(keys[0]);
  });
});

describe('"Export this workspace" at the version the person chooses (B3)', () => {
  /** The Jobs page of a workspace the person may change, its versions listed newest first. */
  async function openJobsWithHistory(
    log: (body: Json) => Reply = () => ok({ commits: VERSIONS, next_cursor: null }),
  ) {
    const service = exportService();
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        ...service.handlers,
        [LOG]: log,
      }),
    );
    renderAt(`/w/${WS}/jobs`);
    await screen.findByRole('heading', { name: 'Jobs' });
    return { api, service };
  }
  const choice = (name: string) => within(section()).getByRole<HTMLInputElement>('radio', { name });
  const versionList = () => within(section()).queryByRole('combobox', { name: 'Version' });

  it('exports the workspace as it is now unless a version is chosen, and says so', async () => {
    const { api } = await openJobsWithHistory();
    expect(choice('As it is now').checked).toBe(true);
    expect(choice('A version from its history').checked).toBe(false);
    expect(section().textContent).toContain(
      'its latest version when the service takes the request',
    );
    expect(versionList()).toBeNull();
    fireEvent.click(exportButton());
    await within(section()).findByText(/^The export was accepted/);
    expect(api.callsTo(EXPORT_WORKSPACE).map((body) => body.at)).toEqual([{ kind: 'latest' }]);
    // Nothing of the history is asked while no version is chosen.
    expect(api.callsTo(LOG)).toEqual([]);
  });

  it('exports a version chosen from its history at that revision, and asks again for it under its key', async () => {
    const { api, service } = await openJobsWithHistory();
    fireEvent.click(choice('A version from its history'));
    const list = await within(section()).findByRole<HTMLSelectElement>('combobox', {
      name: 'Version',
    });
    // The versions of the head the page shows, newest first; the newest is chosen at first.
    expect(api.callsTo(LOG)[0]).toMatchObject({ at: { kind: 'revision', revision: HEAD } });
    expect(Array.from(list.options, (option) => option.value)).toEqual(
      VERSIONS.map((version) => version.revision),
    );
    expect(list.value).toBe(HEAD);
    fireEvent.change(list, { target: { value: OLDER } });
    expect(section().textContent).toContain(`as of revision ${OLDER.slice(0, 10)}`);
    service.script.push(() => fail(503, 'unavailable', 'the gateway lost the answer'));
    fireEvent.click(exportButton());
    await within(section()).findByText(/temporarily unavailable/);
    fireEvent.click(within(section()).getByRole('button', { name: 'Try again' }));
    await within(section()).findByText(/^The export was accepted/);
    const sent = api.callsTo(EXPORT_WORKSPACE);
    expect(sent.map((body) => body.at)).toEqual([
      { kind: 'revision', revision: OLDER },
      { kind: 'revision', revision: OLDER },
    ]);
    expect(sent[1]?.idempotency_key).toBe(sent[0]?.idempotency_key);
    expect(service.jobs()).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('starts a new export for another version, under a new key', async () => {
    const { api } = await openJobsWithHistory();
    fireEvent.click(choice('A version from its history'));
    const list = await within(section()).findByRole<HTMLSelectElement>('combobox', {
      name: 'Version',
    });
    fireEvent.change(list, { target: { value: OLDER } });
    fireEvent.click(exportButton());
    await within(section()).findByText(/job aaaaaaaa/);
    fireEvent.click(choice('As it is now'));
    fireEvent.click(exportButton());
    await within(section()).findByText(/job bbbbbbbb/);
    const sent = api.callsTo(EXPORT_WORKSPACE);
    expect(sent.map((body) => body.at)).toEqual([
      { kind: 'revision', revision: OLDER },
      { kind: 'latest' },
    ]);
    expect(sent[1]?.idempotency_key).not.toBe(sent[0]?.idempotency_key);
  });

  // What is asked is the whole request: the version and the choice of history, not the workspace.
  const unavailable = () => fail(503, 'unavailable', 'the gateway lost the answer');
  const historyBox = () =>
    within(section()).getByRole('checkbox', { name: 'Include the workspace’s history' });

  it('neither shows nor retries a failed export once the history choice changes', async () => {
    const { api, service } = await openJobsWithHistory();
    service.script.push(unavailable);
    fireEvent.click(exportButton());
    await within(section()).findByText(/temporarily unavailable/);
    within(section()).getByRole('button', { name: 'Try again' }).focus();
    fireEvent.click(historyBox());
    expect(within(section()).queryByText(/temporarily unavailable/)).toBeNull();
    expect(within(section()).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(document.activeElement).not.toBe(document.body);
    fireEvent.click(exportButton());
    await within(section()).findByText(/^The export was accepted/);
    const sent = api.callsTo(EXPORT_WORKSPACE);
    expect(sent.map((body) => body.include_history)).toEqual([false, true]);
    expect(sent[1]?.idempotency_key).not.toBe(sent[0]?.idempotency_key);
  });

  it('does not say of another version an export still out for the one before', async () => {
    const { api, service } = await openJobsWithHistory();
    let release: () => void = () => {};
    service.script.push(
      (_made, usual) =>
        new Promise<Reply>((resolve) => {
          release = () => resolve(usual);
        }),
    );
    fireEvent.click(exportButton());
    await within(section()).findByRole('button', { name: 'Starting the export…' });
    fireEvent.click(choice('A version from its history'));
    const list = await within(section()).findByRole<HTMLSelectElement>('combobox', {
      name: 'Version',
    });
    fireEvent.change(list, { target: { value: OLDER } });
    expect(exportButton().textContent).toBe('Export the workspace');
    expect(exportButton().getAttribute('aria-disabled')).toBe('false');
    await act(async () => {
      release();
      await new Promise((settled) => setTimeout(settled, 50));
    });
    expect(within(section()).queryByText(/accepted/)).toBeNull();
    fireEvent.click(exportButton());
    await within(section()).findByText(/job bbbbbbbb/);
    expect(api.callsTo(EXPORT_WORKSPACE).map((body) => body.at)).toEqual([
      { kind: 'latest' },
      { kind: 'revision', revision: OLDER },
    ]);
  });

  it('asks again under the key a request had, when the choice comes back to it unanswered', async () => {
    const { api, service } = await openJobsWithHistory();
    service.script.push(unavailable, unavailable);
    fireEvent.click(exportButton());
    await within(section()).findByText(/temporarily unavailable/);
    fireEvent.click(choice('A version from its history'));
    const list = await within(section()).findByRole<HTMLSelectElement>('combobox', {
      name: 'Version',
    });
    fireEvent.change(list, { target: { value: OLDER } });
    fireEvent.click(exportButton());
    await within(section()).findByText(/temporarily unavailable/);
    fireEvent.click(choice('As it is now'));
    fireEvent.click(exportButton());
    await within(section()).findByText(/^The export was accepted/);
    const sent = api.callsTo(EXPORT_WORKSPACE);
    expect(sent.map((body) => body.at)).toEqual([
      { kind: 'latest' },
      { kind: 'revision', revision: OLDER },
      { kind: 'latest' },
    ]);
    expect(sent[2]?.idempotency_key).toBe(sent[0]?.idempotency_key);
    expect(sent[1]?.idempotency_key).not.toBe(sent[0]?.idempotency_key);
    expect(service.jobs()).toHaveLength(2);
  });

  it('says a failed listing of older versions, keeps those listed, and asks again from the same place', {
    timeout: 15_000,
  }, async () => {
    let olderAsked = 0;
    const { api } = await openJobsWithHistory((body) => {
      const page = body.page as { cursor?: string };
      if (page.cursor === undefined)
        return ok({ commits: VERSIONS.slice(0, 2), next_cursor: 'older' });
      olderAsked += 1;
      return olderAsked === 1
        ? // Not a transient failure: the query client does not ask again by itself.
          fail(500, 'internal', 'the history store is busy')
        : ok({ commits: VERSIONS.slice(2), next_cursor: null });
    });
    fireEvent.click(choice('A version from its history'));
    const list = await within(section()).findByRole<HTMLSelectElement>('combobox', {
      name: 'Version',
    });
    expect(list.options).toHaveLength(2);
    const older = within(section()).getByRole('button', { name: 'Show older versions' });
    older.focus();
    fireEvent.click(older);
    await within(section()).findByText(/the history store is busy/);
    // The versions already listed stay, and so does the choice among them.
    expect(list.options).toHaveLength(2);
    expect(list.value).toBe(VERSIONS[0]?.revision);
    expect(document.activeElement).not.toBe(document.body);
    fireEvent.click(within(section()).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(list.options).toHaveLength(VERSIONS.length));
    expect(within(section()).queryByText(/the history store is busy/)).toBeNull();
    expect(api.callsTo(LOG).map((body) => (body.page as { cursor?: string }).cursor)).toEqual([
      undefined,
      'older',
      'older',
    ]);
  });
});
