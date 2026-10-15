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
import startImportSchema from '../../../api/schemas/start_import.input.json';
import type { Job } from '../../src/api/generated/types.gen';
import {
  type ImportAttempt,
  importAttempt,
  importBase,
  MAX_IMPORT_UPLOADS,
} from '../../src/features/imports/import-start';
import { resetUploads } from '../../src/features/imports/uploading';
import { JOBS_POLL_MS, JOBS_SAFETY_POLL_MS } from '../../src/features/jobs/jobs';
import { fail, type Handler, mockApi, ok } from './support/api';
import {
  accepted,
  CANCEL,
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

describe('the identity of a start_import request', () => {
  const last: ImportAttempt = {
    parts: [{ key: 'the-first-key', uploadIds: ['u1', 'u2'], job: null }],
    destination: 'notes',
    baseRevision: HEAD,
  };
  const keyOf = (attempt: ImportAttempt) => attempt.parts.at(0)?.key ?? '';

  it('is the one before while the request names the same uploads, folder and base', () => {
    expect(importAttempt(last, ['u1', 'u2'], 'notes', HEAD)).toBe(last);
  });

  it.each([
    ['another base revision', ['u1', 'u2'], 'notes', OLD],
    ['another folder', ['u1', 'u2'], 'elsewhere', HEAD],
    ['one upload more', ['u1', 'u2', 'u3'], 'notes', HEAD],
    ['one upload fewer', ['u1'], 'notes', HEAD],
    ['another upload in the same place', ['u1', 'u9'], 'notes', HEAD],
    ['the same uploads in another order', ['u2', 'u1'], 'notes', HEAD],
  ])('is a new one, under a new key, for %s', (_what, uploadIds, destination, baseRevision) => {
    const next = importAttempt(last, uploadIds, destination, baseRevision);
    expect(keyOf(next)).not.toBe(keyOf(last));
    expect(keyOf(next)).toMatch(/^[0-9a-f-]{36}$/);
    expect(next).toMatchObject({ parts: [{ uploadIds, job: null }], destination, baseRevision });
  });

  it('is a new one when nothing was asked before', () => {
    expect(keyOf(importAttempt(null, ['u1'], '', HEAD))).toMatch(/^[0-9a-f-]{36}$/);
  });

  it(`names at most ${MAX_IMPORT_UPLOADS} uploads in one request, the contract's bound: more go in parts, in order, each under its key`, () => {
    expect(startImportSchema.properties.upload_ids.maxItems).toBe(MAX_IMPORT_UPLOADS);
    const ids = (n: number) => Array.from({ length: n }, (_, at) => `u${at}`);
    expect(importAttempt(null, ids(MAX_IMPORT_UPLOADS), '', HEAD).parts).toHaveLength(1);
    const split = importAttempt(null, ids(MAX_IMPORT_UPLOADS + 1), '', HEAD);
    expect(split.parts.map((part) => part.uploadIds.length)).toEqual([MAX_IMPORT_UPLOADS, 1]);
    expect(split.parts.flatMap((part) => part.uploadIds)).toEqual(ids(MAX_IMPORT_UPLOADS + 1));
    expect(new Set(split.parts.map((part) => part.key)).size).toBe(2);
    expect(
      importAttempt(null, ids(2 * MAX_IMPORT_UPLOADS + 1), '', HEAD).parts.map(
        (part) => part.uploadIds.length,
      ),
    ).toEqual([MAX_IMPORT_UPLOADS, MAX_IMPORT_UPLOADS, 1]);
  });
});

describe('the revision an import is based on', () => {
  const made = (revision: string) => jobOf('succeeded', { revision });

  it('is the one shown while no import started here has made another', () => {
    expect(importBase(HEAD, [])).toBe(HEAD);
    // Not read yet, still running, failed, or finished without reporting a revision.
    expect(
      importBase(HEAD, [undefined, jobOf('running'), jobOf('failed'), jobOf('succeeded')]),
    ).toBe(HEAD);
    // A revision reported by a job that did not succeed is not the head.
    expect(importBase(HEAD, [jobOf('failed', { revision: OLD })])).toBe(HEAD);
  });

  it('is the one made by the newest import started here that has finished', () => {
    expect(importBase(HEAD, [made(IMPORTED)])).toBe(IMPORTED);
    expect(importBase(HEAD, [jobOf('running'), made(IMPORTED), made(OLD)])).toBe(IMPORTED);
  });
});

describe('starting the import', () => {
  it('names the uploaded files, the folder typed and the version shown, and shows the job at once', async () => {
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

  it('sends the folder without the spaces typed around it, and takes other spaces for the same request', async () => {
    let answers = 0;
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => {
        answers += 1;
        return answers === 1 ? fail(503, 'unavailable', 'busy') : accepted(jobOf('queued'));
      },
      [JOB]: () => ok(jobOf('queued')),
    });
    fireEvent.change(folder(), { target: { value: '  notes/2025 ' } });
    fireEvent.click(importButton());
    await eventually(() =>
      screen.getByText('Starting the import: The service is temporarily unavailable.'),
    );
    expect(api.callsTo(START).at(0)?.destination).toBe('notes/2025');
    // The field is left as it was typed; only what is sent is trimmed.
    expect(folder()).toHaveProperty('value', '  notes/2025 ');
    await eventually(() => expect(importButton().getAttribute('aria-disabled')).toBe('false'));
    // The same folder with other spaces around it is the same request, key included.
    fireEvent.change(folder(), { target: { value: 'notes/2025    ' } });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    const sent = api.callsTo(START);
    expect(sent).toHaveLength(2);
    expect(sent.at(1)).toEqual(sent.at(0));
    expect(
      screen.getByText(
        'Import of 1 file into “notes/2025” was accepted as Import, job 88888888. It is not finished: its state is “waiting to start”.',
      ),
    ).toBeDefined();
  });

  it('takes a folder of spaces alone for the top of the workspace', async () => {
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(jobOf('queued')),
      [JOB]: () => ok(jobOf('queued')),
    });
    fireEvent.change(folder(), { target: { value: '   ' } });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    expect(api.callsTo(START).at(0)?.destination).toBe('');
    expect(
      screen.getByText(/^Import of 1 file into the top of the workspace was accepted/),
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
    // The job was shown from the answer itself, and asked for once after it.
    await later(() => expect(api.callsTo(JOB)).toHaveLength(1));
    job = jobOf('succeeded', { item_ids: [NOTE], revision: IMPORTED });
    await pass(JOBS_POLL_MS);
    await later(() => expect(jobRow().textContent).toContain('Finished.'));
    expect(api.callsTo(JOB)).toHaveLength(2);
    await later(() => within(jobRow()).getByRole('link', { name: 'Meeting notes' }));
    expect(within(jobRow()).getByRole('link', { name: 'Meeting notes' }).getAttribute('href')).toBe(
      `/w/${WS}/i/${NOTE}?rev=${IMPORTED}`,
    );
    // It has ended: it is not asked for again.
    await pass(JOBS_POLL_MS * 10);
    expect(api.callsTo(JOB)).toHaveLength(2);
  });

  it('asks for the job once the event stream is open: one that ended before then is not left waiting', async () => {
    // The service answers the job as waiting; its in-process runner then finishes it at once.
    let job: Job = jobOf('queued');
    // The service answers a read as waiting until the stream is open (the test's own clock).
    let streamOpen = false;
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => {
        const answered = job;
        job = jobOf('succeeded', { item_ids: [NOTE], revision: IMPORTED });
        return accepted(answered);
      },
      [JOB]: () => ok(streamOpen ? job : jobOf('queued')),
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
    await later(() => expect(api.callsTo(JOB)).toHaveLength(1));
    expect(jobRow().textContent).toContain('Waiting to start.');
    // It opens. No event follows: every change of the job was before it.
    streamOpen = true;
    open();
    await eventually(() => expect(events.streams).toHaveLength(1));
    await later(() => expect(jobRow().textContent).toContain('Finished.'));
    // Asked once after the answer and once by the stream's opening: the quick clock has not come round.
    expect(api.callsTo(JOB)).toHaveLength(2);
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
    // Asked after the answer and when the stream opened (the two may share one request).
    await later(() => expect(api.callsTo(JOB).length).toBeGreaterThanOrEqual(1));
    await pass(50);
    const base = api.callsTo(JOB).length;
    // Not on the quick clock while the stream is open.
    await pass(JOBS_POLL_MS * 4);
    expect(api.callsTo(JOB)).toHaveLength(base);
    // An event asks for the single job, not only for the list.
    job = jobOf('running', { progress: 60 });
    act(() => events.streams.at(0)?.send('3', eventOf('job_updated', '3', { job_id: JOB_ID })));
    await later(() =>
      expect(within(jobRow()).getByRole('progressbar').getAttribute('value')).toBe('60'),
    );
    expect(api.callsTo(JOB)).toHaveLength(base + 1);
    // No event tells of its end: the slow clock finds it, and then stops.
    job = jobOf('succeeded', { item_ids: [], revision: IMPORTED });
    await pass(JOBS_SAFETY_POLL_MS - 1_000);
    expect(api.callsTo(JOB)).toHaveLength(base + 1);
    await pass(1_000);
    await later(() => expect(jobRow().textContent).toContain('Finished.'));
    expect(api.callsTo(JOB)).toHaveLength(base + 2);
    await pass(JOBS_SAFETY_POLL_MS * 5);
    expect(api.callsTo(JOB)).toHaveLength(base + 2);
  });

  it('stops asking for a started job once the service says the person is not signed in, and starts again at the retry', async () => {
    let signedIn = true;
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(jobOf('running')),
      [JOB]: () => (signedIn ? ok(jobOf('running')) : fail(401, 'unauthenticated', 'no session')),
    });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    // The answer's own question.
    await later(() => expect(api.callsTo(JOB)).toHaveLength(1));
    signedIn = false;
    await pass(JOBS_POLL_MS);
    await later(() => screen.getByText('Reading this job: You are not signed in.'));
    expect(api.callsTo(JOB)).toHaveLength(2);
    await pass(600_000);
    expect(api.callsTo(JOB)).toHaveLength(2);
    // The row stays as last answered.
    expect(jobRow().textContent).toContain('Running.');
    signedIn = true;
    fireEvent.click(screen.getByRole('button', { name: 'Try again after signing in' }));
    await later(() => expect(api.callsTo(JOB)).toHaveLength(3));
    await pass(JOBS_POLL_MS);
    await later(() => expect(api.callsTo(JOB)).toHaveLength(4));
  });

  it('asks for the stream again, on the import page, once a job is answered after the session had ended', async () => {
    let signedIn = true;
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(jobOf('running')),
      [JOB]: () => (signedIn ? ok(jobOf('running')) : fail(401, 'unauthenticated', 'no session')),
    });
    const events = mockEvents(fail(401, 'unauthenticated', 'no session'));
    signedIn = false;
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    await eventually(() => expect(events.state.requests).toBe(1));
    // Refused as not signed in: not asked for again, however long the page is shown.
    await pass(JOBS_POLL_MS);
    await later(() => screen.getByText('Reading this job: You are not signed in.'));
    await pass(600_000);
    expect(events.state.requests).toBe(1);
    expect(api.callsTo(JOB)).toHaveLength(1);
    // The person signs in and retries the reading: with its answer the stream is asked for.
    signedIn = true;
    events.state.refusal = null;
    fireEvent.click(screen.getByRole('button', { name: 'Try again after signing in' }));
    await later(() => expect(events.streams).toHaveLength(1));
    expect(events.state.requests).toBe(2);
  });

  it('reads a started job again after a cancel that failed', async () => {
    let job: Job = jobOf('running');
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => accepted(job),
      [JOB]: () => ok(job),
      [CANCEL]: () => {
        // Carried out; the answer is lost on the way.
        job = jobOf('cancelled');
        return fail(503, 'unavailable', 'the answer was lost');
      },
    });
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    await later(() => expect(api.callsTo(JOB)).toHaveLength(1));
    fireEvent.click(within(jobRow()).getByRole('button', { name: 'Cancel' }));
    fireEvent.click(within(jobRow()).getByRole('button', { name: 'Cancel the job' }));
    await eventually(() =>
      screen.getByText('Cancelling this job: The service is temporarily unavailable.'),
    );
    // The single job was asked for again, with no clock moved, and the row shows what was answered.
    await later(() => expect(jobRow().textContent).toContain('Cancelled.'));
    expect(api.callsTo(JOB)).toHaveLength(2);
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

  it('sends an import of other uploads as a new request, after one that failed', async () => {
    let answers = 0;
    const { api, service } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => {
        answers += 1;
        return answers === 1 ? fail(503, 'unavailable', 'busy') : accepted(jobOf('queued'));
      },
      [JOB]: () => ok(jobOf('queued')),
    });
    fireEvent.click(importButton());
    await eventually(() =>
      screen.getByText('Starting the import: The service is temporarily unavailable.'),
    );
    // One more file is uploaded before the next press: the request is no longer the same.
    vi.useRealTimers();
    choose([fileOf('b.txt', 'abcd')]);
    fireEvent.click(screen.getByRole('button', { name: /^Upload \d+ files?$/ }));
    await waitFor(() => expect(importButton().textContent).toBe('Import 2 uploaded files'));
    fireEvent.click(importButton());
    await waitFor(() => expect(api.callsTo(START)).toHaveLength(2));
    const [first, second] = api.callsTo(START);
    expect(second?.upload_ids).toEqual(service.slots.map((slot) => slot.id));
    expect(second?.idempotency_key).not.toBe(first?.idempotency_key);
  });

  it('lists a job it started when the Jobs page is next shown, although that list was just read', async () => {
    let started = false;
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => {
        started = true;
        return accepted(jobOf('queued'));
      },
      [JOB]: () => ok(jobOf('queued')),
      [JOBS]: () => ok({ items: started ? [jobOf('queued')] : [], next_cursor: null }),
    });
    // The Jobs page is looked at first: its list is read, and holds nothing.
    fireEvent.click(screen.getByRole('link', { name: 'Jobs' }));
    await eventually(() => screen.getByText(/This workspace has no jobs yet\./));
    fireEvent.click(screen.getByRole('link', { name: 'Import' }));
    await eventually(() => screen.getByRole('heading', { name: 'Import files' }));
    fireEvent.click(importButton());
    await eventually(() => jobRow());
    // Back on the Jobs page at once, well inside the time its list counts as fresh.
    fireEvent.click(screen.getByRole('link', { name: 'All jobs of this workspace' }));
    await eventually(() => screen.getByRole('heading', { name: 'Jobs' }));
    await later(() => jobRow());
    expect(screen.queryByText(/This workspace has no jobs yet\./)).toBeNull();
    expect(api.callsTo(JOBS)).toHaveLength(2);
  });

  it('shows a refusal of the folder at its field, which takes focus, and not as a notice', async () => {
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [START]: () => ({
        status: 400,
        body: {
          code: 'invalid_input',
          message: 'a folder ends with no dot',
          field: '/destination',
        },
      }),
    });
    const folder = screen.getByRole('combobox', { name: 'Folder to import into' });
    fireEvent.change(folder, { target: { value: 'notes.' } });
    fireEvent.click(importButton());
    await eventually(() => expect(folder.getAttribute('aria-invalid')).toBe('true'));
    expect(document.activeElement).toBe(folder);
    expect(folder.getAttribute('aria-describedby') ?? '').toContain(
      screen.getByText('The service refused this folder: a folder ends with no dot').id,
    );
    expect(screen.queryByText(/^Starting the import:/)).toBeNull();
    // Typing another folder takes the refusal away.
    fireEvent.change(folder, { target: { value: 'notes' } });
    expect(folder.getAttribute('aria-invalid')).toBe('false');
    expect(api.callsTo(START)).toHaveLength(1);
  });

  // An import's base is stated by the contract (ruling 26): a head that moved does not refuse it.
  // So no conflict is read as a stale base, whatever field it names.
  it.each([
    ['names no field', {}],
    ['is about another field', { field: '/upload_ids' }],
    ['names the base revision', { field: '/base_revision' }],
  ])(
    'names no stale base for a conflict that %s: the general words, and the service’s own',
    async (_what, about) => {
      let answers = 0;
      const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
        [START]: () => {
          answers += 1;
          return answers === 1
            ? {
                status: 409,
                body: {
                  code: 'conflict',
                  message: 'this upload was already taken by another job',
                  ...about,
                },
              }
            : accepted(jobOf('queued'));
        },
        [JOB]: () => ok(jobOf('queued')),
      });
      fireEvent.click(importButton());
      await eventually(() =>
        screen.getByText('Starting the import: This changed since it was loaded.'),
      );
      expect(screen.getByText(/The service said:/).textContent).toContain(
        'this upload was already taken by another job',
      );
      // Nothing is said of the version shown, or of Refresh: the answer did not say that.
      expect(screen.queryByText(/the version this import was based on/)).toBeNull();
      expect(screen.queryByText(/Press Refresh/)).toBeNull();
      // The general words carry their own action, which repeats the same request.
      fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
      await eventually(() => jobRow());
      expect(api.callsTo(START).at(1)).toEqual(api.callsTo(START).at(0));
    },
  );

  it('bases the next import on the revision its own finished import made, until the workspace is refreshed', async () => {
    const SECOND = '99999999-9999-4999-8999-999999999999';
    const LATER = 'd'.repeat(40);
    const NEWER = 'e'.repeat(40);
    let head = HEAD;
    let first: Job = jobOf('running');
    let second: Job = jobOf('queued', { id: SECOND });
    const { api } = await uploadedFiles([fileOf('a.txt', 'abc')], {
      [OPEN]: () => ok({ ...writableWorkspace, head }),
      [START]: () => accepted(api.callsTo(START).length === 1 ? first : second),
      [JOB]: (body) => ok(body.job_id === SECOND ? second : first),
    });
    // The stream is open, so a job is read again when the test says it changed: no clock is needed.
    vi.useRealTimers();
    const events = mockEvents();
    const changed = (id: string, jobId: string) =>
      act(() => events.streams.at(0)?.send(id, eventOf('job_updated', id, { job_id: jobId })));
    /** Upload one more file and press Import. */
    const importOneMore = async (name: string, content: string) => {
      choose([fileOf(name, content)]);
      fireEvent.click(screen.getByRole('button', { name: /^Upload \d+ files?$/ }));
      await waitFor(() => expect(importButton().textContent).toBe('Import 1 uploaded file'));
      const before = api.callsTo(START).length;
      fireEvent.click(importButton());
      await waitFor(() => expect(api.callsTo(START)).toHaveLength(before + 1));
      return api.callsTo(START).at(-1)?.base_revision;
    };
    const note = () => screen.queryByText(/The next import is based on it/);

    fireEvent.click(importButton());
    await waitFor(() => jobRow());
    expect(api.callsTo(START).at(0)?.base_revision).toBe(HEAD);
    await waitFor(() => expect(events.streams).toHaveLength(1));
    // While it runs, the base is still the version shown: nothing says the head moved.
    expect(note()).toBeNull();

    // The import finishes: it made a revision, and the head is now that one.
    first = jobOf('succeeded', { item_ids: [], revision: IMPORTED });
    changed('1', JOB_ID);
    await waitFor(() => expect(jobRow().textContent).toContain('Finished.'));
    expect(note()?.textContent).toBe(
      'An import started here made revision ffffffffff of the workspace. The next import is based on it. The workspace above still shows the version from before: press Refresh to see what was imported.',
    );
    expect(note()?.getAttribute('role')).toBe('note');
    // A second import from the same page, with the version shown still the old one.
    expect(await importOneMore('b.txt', 'abcd')).toBe(IMPORTED);

    // That one finishes too. It was sent on another base than the version shown, and it is still
    // an import of this page under that version: the next is based on what it made.
    second = jobOf('succeeded', { id: SECOND, item_ids: [], revision: LATER });
    changed('2', SECOND);
    await waitFor(() =>
      expect(note()?.textContent).toContain('made revision dddddddddd of the workspace'),
    );
    expect(await importOneMore('c.txt', 'abcde')).toBe(LATER);

    // Refreshed: the version shown is the base again, whatever the page's own imports made.
    head = NEWER;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await screen.findByText(`Refreshed: now showing revision ${NEWER.slice(0, 10)}.`);
    await waitFor(() => expect(note()).toBeNull());
    expect(await importOneMore('d.txt', 'abcdef')).toBe(NEWER);
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
