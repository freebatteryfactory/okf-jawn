/**
 * Starting the import of the uploaded files (`start_import`) into the folder the person chose, and
 * the job it answers: shown at once in the state it reports, then followed.
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
import type { Job } from '../../src/api/generated/types.gen';
import { resetUploads } from '../../src/features/imports/uploading';
import { JOBS_POLL_MS, JOBS_SAFETY_POLL_MS } from '../../src/features/jobs/jobs';
import { fail, type Handler, mockApi, ok } from './support/api';
import {
  accepted,
  eventOf,
  fileOf,
  IMPORTED,
  JOB,
  JOB_ID,
  JOBS,
  jobOf,
  mockEvents,
  START,
  type UploadService,
  uploadService,
} from './support/imports';
import { mockTransfers } from './support/transfers';
import { explorerHandlers, HEAD, NOTE, renderAt, WS, writableWorkspace } from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  act(() => resetUploads());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

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

/** As `later` in jobs.test.tsx: after the clock itself asked for something. */
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

function choose(files: File[]) {
  const opened: HTMLInputElement[] = [];
  const click = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function open(
    this: HTMLInputElement,
  ) {
    opened.push(this);
  });
  fireEvent.click(screen.getByRole('button', { name: 'Choose files…' }));
  click.mockRestore();
  const picker = opened.at(0);
  if (picker === undefined) throw new Error('no picker');
  Object.defineProperty(picker, 'files', { configurable: true, value: files });
  fireEvent.change(picker);
}

/**
 * The import page with these files uploaded and verified, and then the clock in the test's hands.
 */
async function uploadedFiles(
  files: File[],
  overrides: Record<string, Handler>,
  service: UploadService = uploadService(),
) {
  const api = mockApi(
    explorerHandlers({ [OPEN]: () => ok(writableWorkspace), ...service.handlers, ...overrides }),
  );
  mockTransfers(service.transfer);
  const app = renderAt(`/w/${WS}/import`);
  await screen.findByRole('heading', { name: 'Import files' });
  choose(files);
  fireEvent.click(screen.getByRole('button', { name: /^Upload \d+ files?$/ }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: /^Import \d+ uploaded files?$/ }).textContent).toBe(
      `Import ${service.slots.filter((slot) => slot.complete).length} uploaded ${files.length === 1 ? 'file' : 'files'}`,
    ),
  );
  await waitFor(() => expect(screen.queryByText(/^Uploading: /)).toBeNull());
  vi.useFakeTimers();
  return { api, service, ...app };
}

const importButton = () => screen.getByRole('button', { name: /^Import \d+ uploaded files?$/ });
const folder = () => screen.getByRole('combobox', { name: 'Folder to import into' });
const jobRow = () => {
  const row = document.querySelector<HTMLElement>(`li[data-job="${JOB_ID}"]`);
  if (row === null) throw new Error('no job row');
  return row;
};

describe('starting the import', () => {
  it('names the uploaded files, the folder as typed and the version shown, and shows the job at once', async () => {
    const { api, service } = await uploadedFiles(
      [fileOf('a.txt', 'abc'), fileOf('b.txt', 'abcd')],
      { [START]: () => accepted(jobOf('queued')), [JOB]: () => ok(jobOf('queued')) },
    );
    fireEvent.change(folder(), { target: { value: 'notes/2025' } });
    importButton().focus();
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    expect(api.callsTo(START)).toEqual([
      {
        workspace_id: WS,
        upload_ids: service.slots.map((slot) => slot.id),
        destination: 'notes/2025',
        base_revision: HEAD,
        apply_naming_rules: false,
        idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
      },
    ]);
    // The job, in the state the service reported; never as an import that has finished.
    expect(jobRow().textContent).toContain('Import, job 88888888');
    expect(jobRow().textContent).toContain('Waiting to start.');
    const said = screen.getByText(
      'Import of 2 files into “notes/2025” was accepted as Import, job 88888888. It is not finished: its state is “waiting to start”.',
    );
    expect(said.getAttribute('role')).toBe('status');
    expect(screen.queryByText(/import(ed)? (is )?(finished|complete|done)/i)).toBeNull();
    // The files it took have left the list; the button is still there, with focus.
    expect(screen.getByText('No files are listed yet.')).toBeDefined();
    expect(importButton().textContent).toBe('Import 0 uploaded files');
    expect(importButton().getAttribute('aria-disabled')).toBe('true');
    expect(document.activeElement).toBe(importButton());
    expect(screen.getByRole('heading', { name: 'Imports started here' })).toBeDefined();
    expect(api.violations).toEqual([]);
  });

  it('sends an empty folder for the top of the workspace, and says so', async () => {
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(jobOf('running')),
      [JOB]: () => ok(jobOf('running')),
    });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    expect(api.callsTo(START).at(0)?.destination).toBe('');
    expect(
      screen.getByText(
        'Import of 1 file into the top of the workspace was accepted as Import, job 88888888. It is not finished: its state is “running”.',
      ),
    ).toBeDefined();
  });

  it('imports only the files whose upload the service verified', async () => {
    const service = uploadService();
    service.script.complete.push(() => fail(409, 'conflict', 'the digest differs'));
    const { api } = await uploadedFiles(
      [fileOf('bad.txt', 'abc'), fileOf('good.txt', 'abcd')],
      { [START]: () => accepted(jobOf('queued')), [JOB]: () => ok(jobOf('queued')) },
      service,
    );
    expect(importButton().textContent).toBe('Import 1 uploaded file');
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    const good = service.slots.find((slot) => slot.filename === 'good.txt');
    expect(api.callsTo(START).at(0)?.upload_ids).toEqual([good?.id]);
    // The file that was not uploaded is still listed, with its notice.
    const rows = within(screen.getByRole('list', { name: /^Files to import/ })).getAllByRole(
      'listitem',
    );
    expect(rows).toHaveLength(1);
    expect(rows.at(0)?.textContent).toContain('bad.txt');
    expect(rows.at(0)?.textContent).toContain('Not uploaded.');
  });

  it('does nothing while nothing is uploaded', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace) }));
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    expect(importButton().getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(importButton());
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(api.callsTo(START)).toEqual([]);
  });

  it('follows the job it started, to the items it made, pinned to the job’s revision', async () => {
    let job: Job = jobOf('running', { progress: 20 });
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(job),
      [JOB]: () => ok(job),
    });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    expect(jobRow().textContent).toContain('Running.');
    expect(within(jobRow()).getByRole('progressbar').getAttribute('value')).toBe('20');
    // The job was shown from the answer itself: it has not been asked for yet.
    expect(api.callsTo(JOB)).toEqual([]);
    job = jobOf('succeeded', { item_ids: [NOTE], revision: IMPORTED });
    await pass(JOBS_POLL_MS);
    await later(() => expect(jobRow().textContent).toContain('Finished.'));
    expect(api.callsTo(JOB)).toEqual([{ workspace_id: WS, job_id: JOB_ID }]);
    await later(() => within(jobRow()).getByRole('link', { name: 'Meeting notes' }));
    expect(within(jobRow()).getByRole('link', { name: 'Meeting notes' }).getAttribute('href')).toBe(
      `/w/${WS}/i/${NOTE}?rev=${IMPORTED}`,
    );
    // It has ended: it is not asked for again.
    await pass(JOBS_POLL_MS * 10);
    expect(api.callsTo(JOB)).toHaveLength(1);
  });

  it('asks for the job once the event stream is open: one that ended before then is not left waiting', async () => {
    // The service answers the job as waiting; its in-process runner then finishes it at once.
    let job: Job = jobOf('queued');
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => {
        const answered = job;
        job = jobOf('succeeded', { item_ids: [NOTE], revision: IMPORTED });
        return accepted(answered);
      },
      [JOB]: () => ok(job),
    });
    const events = mockEvents();
    let open: () => void = () => undefined;
    events.state.hold = new Promise<void>((resolve) => {
      open = resolve;
    });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    // The stream is asked for only now, after the answer, and is not open yet.
    await eventually(() => expect(events.state.requests).toBe(1));
    expect(jobRow().textContent).toContain('Waiting to start.');
    expect(api.callsTo(JOB)).toEqual([]);
    // It opens. No event follows: every change of the job was before it.
    open();
    await eventually(() => expect(events.streams).toHaveLength(1));
    await later(() => expect(jobRow().textContent).toContain('Finished.'));
    // Asked once, by the stream's opening: the quick clock has not come round.
    expect(api.callsTo(JOB)).toEqual([{ workspace_id: WS, job_id: JOB_ID }]);
    expect(events.streams.at(0)?.closed).toBe(false);
  });

  it('asks for a started job when the stream says it changed, and on the slow clock only beside it', async () => {
    let job: Job = jobOf('running', { progress: 20 });
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(job),
      [JOB]: () => ok(job),
    });
    const events = mockEvents();
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    await eventually(() => expect(events.streams).toHaveLength(1));
    // Asked once when the stream opened.
    await later(() => expect(api.callsTo(JOB)).toHaveLength(1));
    await pass(50);
    // Not on the quick clock while the stream is open.
    await pass(JOBS_POLL_MS * 4);
    expect(api.callsTo(JOB)).toHaveLength(1);
    // An event asks for the single job, not only for the list.
    job = jobOf('running', { progress: 60 });
    act(() => events.streams.at(0)?.send('3', eventOf('job_updated', '3', { job_id: JOB_ID })));
    await later(() =>
      expect(within(jobRow()).getByRole('progressbar').getAttribute('value')).toBe('60'),
    );
    expect(api.callsTo(JOB)).toHaveLength(2);
    // No event tells of its end: the slow clock finds it, and then stops.
    job = jobOf('succeeded', { item_ids: [], revision: IMPORTED });
    await pass(JOBS_SAFETY_POLL_MS - 1_000);
    expect(api.callsTo(JOB)).toHaveLength(2);
    await pass(1_000);
    await later(() => expect(jobRow().textContent).toContain('Finished.'));
    expect(api.callsTo(JOB)).toHaveLength(3);
    await pass(JOBS_SAFETY_POLL_MS * 5);
    expect(api.callsTo(JOB)).toHaveLength(3);
  });

  it('repeats the same request on "Try again", and sends another folder as a new request', async () => {
    let answers = 0;
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => {
        answers += 1;
        return answers < 4 ? fail(503, 'unavailable', 'busy') : accepted(jobOf('queued'));
      },
      [JOB]: () => ok(jobOf('queued')),
    });
    fireEvent.click(importButton());
    await eventually(() =>
      screen.getByText('Starting the import: The service is temporarily unavailable.'),
    );
    // Nothing started: the file is still listed, and no job is shown.
    expect(screen.queryByRole('heading', { name: 'Imports started here' })).toBeNull();
    expect(importButton().textContent).toBe('Import 1 uploaded file');
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await eventually(() => expect(api.callsTo(START)).toHaveLength(2));
    expect(api.callsTo(START).at(1)).toEqual(api.callsTo(START).at(0));
    await eventually(() =>
      expect(screen.getByRole('button', { name: 'Try again' }).getAttribute('aria-disabled')).toBe(
        'false',
      ),
    );
    // The button pressed again with nothing changed is the same request too.
    fireEvent.click(importButton());
    await eventually(() => expect(api.callsTo(START)).toHaveLength(3));
    expect(api.callsTo(START).at(2)).toEqual(api.callsTo(START).at(0));
    await eventually(() => expect(importButton().getAttribute('aria-disabled')).toBe('false'));
    fireEvent.change(folder(), { target: { value: 'elsewhere' } });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    const sent = api.callsTo(START);
    expect(sent).toHaveLength(4);
    expect(sent.at(3)?.destination).toBe('elsewhere');
    expect(sent.at(3)?.idempotency_key).not.toBe(sent.at(0)?.idempotency_key);
  });

  it('says to refresh when the version shown is not the one to import onto, with no retry of its own', async () => {
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => fail(409, 'conflict', 'the workspace head moved'),
    });
    fireEvent.click(importButton());
    await eventually(() =>
      screen.getByText(
        'Starting the import: The import could not be started on the version shown.',
      ),
    );
    expect(
      screen.getByText(/press Refresh in the workspace’s bar, then start the import again\./),
    ).toBeDefined();
    expect(screen.getByText(/The service said:/).textContent).toContain('the workspace head moved');
    expect(screen.queryByText(/This changed since it was loaded/)).toBeNull();
    expect(screen.queryByRole('button', { name: /Try again|Reload/ })).toBeNull();
    expect(screen.queryByRole('heading', { name: 'Imports started here' })).toBeNull();
    expect(api.callsTo(START)).toHaveLength(1);
  });

  it('reads "not available in this build" where imports are not built, and starts nothing', async () => {
    await uploadedFiles([fileOf('a.txt', 'abc')], {});
    fireEvent.click(importButton());
    await eventually(() => screen.getByText('Starting the import: Not available in this build.'));
    expect(screen.queryByRole('heading', { name: 'Imports started here' })).toBeNull();
    expect(screen.queryByText(/was accepted as/)).toBeNull();
    expect(importButton().textContent).toBe('Import 1 uploaded file');
  });

  it('suggests the folders the tree has listed', async () => {
    mockApi(explorerHandlers({ [OPEN]: () => ok(writableWorkspace) }));
    renderAt(`/w/${WS}/import`);
    await screen.findByRole('heading', { name: 'Import files' });
    await screen.findByRole('treeitem', { name: 'notes' });
    // Any change of the page reads the cache again; typing is one.
    fireEvent.change(folder(), { target: { value: 'n' } });
    const list = folder().getAttribute('list');
    expect(
      document.querySelector(`datalist#${CSS.escape(list ?? '')} option[value="notes"]`),
    ).not.toBeNull();
  });

  it('leads from a started job to all the jobs of the workspace', async () => {
    await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(jobOf('queued')),
      [JOB]: () => ok(jobOf('queued')),
      [JOBS]: () => ok({ items: [jobOf('queued')], next_cursor: null }),
    });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    fireEvent.click(screen.getByRole('link', { name: 'All jobs of this workspace' }));
    await eventually(() => screen.getByRole('heading', { name: 'Jobs' }));
    await eventually(() => jobRow());
    expect(jobRow().textContent).toContain('Waiting to start.');
  });
});
