/**
 * The workspace's jobs: what each row says, how the list stays current (the event stream, or a
 * clock while a job has not ended; nothing from a hidden tab), cancel with its question, and a
 * retry of a failed job.
 */
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Job, Workspace } from '../../src/api/generated/types.gen';
import { MAX_RECONNECT_MS, RECONNECT_MS } from '../../src/features/jobs/feed';
import { JOB_ITEMS_STEP } from '../../src/features/jobs/JobRow';
import { JOBS_POLL_MS, JOBS_SAFETY_POLL_MS } from '../../src/features/jobs/jobs';
import { fail, fromNewerService, type Handler, type Json, mockApi, ok } from './support/api';
import {
  accepted,
  CANCEL,
  eventOf,
  IMPORTED,
  JOB_ID,
  JOBS,
  jobOf,
  mockEvents,
  pageShown,
  RETRY,
} from './support/imports';
import {
  explorerHandlers,
  itemDocument,
  NOTE,
  noteSummary,
  OLD,
  renderAt,
  SOURCE,
  sourceSummary,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const ITEM = '/api/items/get-item';
const EVENTS = `/api/workspaces/${WS}/events`;
const SECOND = '99999999-9999-4999-8999-999999999999';
/** What the page says while it follows the event stream. */
const FOLLOWING = `This list follows the workspace’s events: it is asked again when a job changes, and every ${JOBS_SAFETY_POLL_MS / 1000} seconds while a job is waiting or running.`;

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** Let answers and renders finish, moving the test's clock on by `ms`. */
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

/**
 * Wait until `check` holds after the clock itself started something (a listing asked on the
 * clock), moving it on a millisecond at a time. A timer set while the clock is being moved, such
 * as the cache telling the page of the answer, is due a millisecond later: standing still would
 * never run it.
 */
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

const listOf = (jobs: Job[], next: string | null = null) => ok({ items: jobs, next_cursor: next });

/** The Jobs page of a writable workspace, with the clock in the test's hands. */
async function openJobs(overrides: Record<string, Handler>, who = writableWorkspace) {
  vi.useFakeTimers();
  const api = mockApi(explorerHandlers({ [OPEN]: () => ok(who), ...overrides }));
  return { api, ...(await showJobs()) };
}

async function showJobs() {
  const app = renderAt(`/w/${WS}/jobs`);
  await eventually(() => screen.getByRole('heading', { name: 'Jobs' }));
  return app;
}

const rowOf = (id = JOB_ID) => {
  const row = document.querySelector<HTMLElement>(`li[data-job="${id}"]`);
  if (row === null) throw new Error(`no row for job ${id}`);
  return row;
};
const listed = () => eventually(() => rowOf());

describe('what a job’s row says', () => {
  it('says what the job is, how it stands, its progress while it has not ended, and its attempt', async () => {
    await openJobs({
      [JOBS]: () =>
        listOf([
          jobOf('running', { progress: 40, attempt: 2 }),
          jobOf('queued', { id: SECOND, kind: 'rebuild_index' }),
        ]),
    });
    await listed();
    expect(rowOf().textContent).toContain('Import, job 88888888');
    expect(rowOf().textContent).toContain('Running, attempt 2.');
    const bar = within(rowOf()).getByRole('progressbar', {
      name: 'Import, job 88888888: progress',
    });
    expect(bar.getAttribute('value')).toBe('40');
    expect(rowOf().textContent).toContain('40%');
    expect(rowOf(SECOND).textContent).toContain('Rebuild of the search index, job 99999999');
    // A first attempt is not numbered.
    expect(rowOf(SECOND).textContent).toContain('Waiting to start.');
    expect(rowOf(SECOND).textContent).not.toContain('attempt');
  });

  it('shows no progress bar for a job that has ended, and each ending in its own words', async () => {
    const ids = ['a', 'b', 'c'].map((letter) => `${letter.repeat(8)}-8888-4888-8888-888888888888`);
    const [done, failed, cancelled] = ids;
    await openJobs({
      [JOBS]: () =>
        listOf([
          jobOf('succeeded', { id: done ?? '', progress: 100 }),
          jobOf('failed', {
            id: failed ?? '',
            progress: 30,
            error: { code: 'internal', message: 'the converter stopped', request_id: 'req-7' },
          }),
          jobOf('cancelled', { id: cancelled ?? '', progress: 60 }),
        ]),
    });
    await eventually(() => rowOf(done));
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(rowOf(done).textContent).toContain('Finished.');
    expect(rowOf(failed).textContent).toContain('Failed.');
    expect(rowOf(failed).textContent).toContain(
      'Why: Something went wrong in the service. The service said: the converter stopped Reference: req-7',
    );
    expect(rowOf(cancelled).textContent).toContain('Cancelled.');
    expect(rowOf(cancelled).textContent).toContain(
      'It was cancelled. Files that were uploaded are kept by the service.',
    );
  });

  it('never shows more than the whole as progress', async () => {
    await openJobs({ [JOBS]: () => listOf([jobOf('running', { progress: 250 })]) });
    await listed();
    expect(within(rowOf()).getByRole('progressbar').getAttribute('value')).toBe('100');
    expect(rowOf().textContent).toContain('100%');
  });

  it('leads from a finished import to its items, each pinned to the revision the job reports', async () => {
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf('succeeded', { item_ids: [NOTE, SOURCE], revision: IMPORTED })]),
      [ITEM]: (body) =>
        ok(
          itemDocument(
            body.item_id === NOTE
              ? noteSummary
              : {
                  ...sourceSummary,
                  extraction: { status: 'unsupported', text_origin: 'none', corrected: false },
                },
          ),
        ),
    });
    await eventually(() => screen.getByRole('link', { name: 'Meeting notes' }));
    const items = screen.getByRole('list', { name: 'Items made by Import, job 88888888' });
    const links = within(items).getAllByRole('link');
    expect(links.map((link) => link.getAttribute('href'))).toEqual([
      `/w/${WS}/i/${NOTE}?rev=${IMPORTED}`,
      `/w/${WS}/i/${SOURCE}?rev=${IMPORTED}`,
    ]);
    expect(rowOf().textContent).toContain('It made revision ffffffffff of the workspace.');
    // Each item was read at that revision, and an unsupported file is named as one.
    const read = api.callsTo(ITEM).filter((body) => body.item_id !== undefined);
    expect(
      read.every(
        (body) =>
          JSON.stringify(body.at) === JSON.stringify({ kind: 'revision', revision: IMPORTED }),
      ),
    ).toBe(true);
    const source = within(items).getAllByRole('listitem').at(1);
    expect(source?.textContent).toContain('Annual report');
    expect(source?.textContent).toContain('(format not supported)');
    expect(source?.textContent).not.toContain('converted');
    expect(api.violations).toEqual([]);
  });

  it('ends on the page’s heading when an item’s link is followed and its page leaves', async () => {
    await openJobs({
      [JOBS]: () => listOf([jobOf('succeeded', { item_ids: [NOTE], revision: IMPORTED })]),
    });
    const link = () => screen.getByRole('link', { name: 'Meeting notes' });
    await eventually(link);
    link().focus();
    fireEvent.click(link());
    await eventually(() => screen.getByRole('tab', { name: 'Meeting notes' }));
    expect(screen.queryByRole('heading', { name: 'Jobs' })).toBeNull();
    // The link went with its page: focus is on the workspace's name, not on the page body.
    expect(document.activeElement).toBe(
      screen.getByRole('heading', { name: 'Workshop notes', level: 1 }),
    );
  });

  it('leads back, by "Documents", to the item at the revision its pane cites', async () => {
    mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => listOf([jobOf('succeeded')]),
      }),
    );
    // An item opened at an older revision than the workspace shows.
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await screen.findByRole('tab', { name: 'Meeting notes', selected: true });
    fireEvent.click(screen.getByRole('link', { name: 'Jobs' }));
    await screen.findByRole('heading', { name: 'Jobs' });
    expect(screen.getByRole('link', { name: 'Documents' }).getAttribute('href')).toBe(
      `/w/${WS}/i/${NOTE}?rev=${OLD}`,
    );
  });

  it('links no item of a job that reports no revision', async () => {
    await openJobs({ [JOBS]: () => listOf([jobOf('succeeded', { item_ids: [NOTE, SOURCE] })]) });
    await listed();
    expect(rowOf().textContent).toContain('It made 2 items. The job reports no revision');
    expect(within(rowOf()).queryByRole('link')).toBeNull();
  });

  it('links the first items, and "Show more" brings the next and moves focus to them', async () => {
    const ids = Array.from(
      { length: JOB_ITEMS_STEP + 2 },
      (_, index) => `aaaaaaaa-aaaa-4aaa-8aaa-${String(index).padStart(12, '0')}`,
    );
    await openJobs({
      [JOBS]: () => listOf([jobOf('succeeded', { item_ids: ids, revision: IMPORTED })]),
    });
    await listed();
    const links = () => within(rowOf()).getAllByRole('link');
    await eventually(() => expect(links()).toHaveLength(JOB_ITEMS_STEP));
    const more = within(rowOf()).getByRole('button', { name: 'Show 2 more items of 2' });
    more.focus();
    fireEvent.click(more);
    await eventually(() => expect(links()).toHaveLength(JOB_ITEMS_STEP + 2));
    expect(document.activeElement).toBe(links().at(JOB_ITEMS_STEP));
    expect(within(rowOf()).queryByRole('button', { name: /^Show/ })).toBeNull();
  });

  it('says that a job is in a state this page does not know, with the service’s word, and goes on asking about it', async () => {
    const { api } = await openJobs({
      [JOBS]: () =>
        listOf([{ ...jobOf('running', { progress: 40 }), state: fromNewerService('paused') }]),
    });
    await listed();
    expect(rowOf().textContent).toBe(
      'Import, job 88888888In a state this page does not know (“paused”).',
    );
    // Nothing is guessed from it: no progress bar, no cancel, no retry.
    expect(within(rowOf()).queryByRole('progressbar')).toBeNull();
    expect(within(rowOf()).queryByRole('button')).toBeNull();
    // It is not known to have ended, so the clock goes on.
    expect(api.callsTo(JOBS)).toHaveLength(1);
    await pass(JOBS_POLL_MS);
    await later(() => expect(api.callsTo(JOBS)).toHaveLength(2));
    // The mock saw exactly the answers this test meant to send.
    expect(api.violations.every((seen) => seen.startsWith(`${JOBS} response:`))).toBe(true);
    expect(api.violations.length).toBeGreaterThan(0);
  });

  it('says, by the job’s name, when a job enters a state this page does not know', async () => {
    let paused = false;
    await openJobs({
      [JOBS]: () =>
        listOf([
          paused ? { ...jobOf('running'), state: fromNewerService('Paused') } : jobOf('running'),
        ]),
    });
    await listed();
    paused = true;
    await pass(JOBS_POLL_MS);
    await later(() =>
      screen.getByText('Import, job 88888888: in a state this page does not know (“Paused”).'),
    );
  });

  it('does not take a state named like a member of every object for one it has words for', async () => {
    // "constructor" and "toString" are found on any table by a plain look-up.
    await openJobs({
      [JOBS]: () =>
        listOf([
          { ...jobOf('running'), state: fromNewerService('constructor') },
          { ...jobOf('running'), id: SECOND, state: fromNewerService('toString') },
        ]),
    });
    await listed();
    expect(rowOf().textContent).toBe(
      'Import, job 88888888In a state this page does not know (“constructor”).',
    );
    expect(rowOf(SECOND).textContent).toBe(
      'Import, job 99999999In a state this page does not know (“toString”).',
    );
  });

  it('names a job of a kind this page does not know as unknown, with the service’s word', async () => {
    await openJobs({
      [JOBS]: () =>
        listOf([
          {
            ...jobOf('succeeded', {
              artifact: {
                artifact_id: '99999999-9999-4999-8999-999999999999',
                download_path: `/api/workspaces/${WS}/artifacts/99999999-9999-4999-8999-999999999999`,
                kind: fromNewerService('audit_trail'),
                sha256: 'c'.repeat(64),
                size: '120',
              },
            }),
            kind: fromNewerService('defragment'),
          },
        ]),
    });
    await listed();
    expect(rowOf().textContent).toContain(
      'A job of a kind this page does not know (“defragment”), job 88888888',
    );
    expect(rowOf().textContent).toContain('Finished.');
    expect(rowOf().textContent).toContain(
      'It produced something of a kind this page does not know (“audit_trail”): 120 bytes.',
    );
    expect(rowOf().textContent).not.toContain('undefined');
  });

  it('shows the warnings a job recorded', async () => {
    await openJobs({
      [JOBS]: () =>
        listOf([
          jobOf('succeeded', {
            warnings: [{ code: 'skipped', message: 'One file was empty', location: 'a/b.txt' }],
          }),
        ]),
    });
    await listed();
    expect(within(rowOf()).getByRole('note').textContent).toBe('One file was empty (a/b.txt)');
  });
});

describe('listing the jobs', () => {
  it('says so when the workspace has none', async () => {
    await openJobs({ [JOBS]: () => listOf([]) });
    await eventually(() => screen.getByText(/This workspace has no jobs yet\./));
  });

  it('shows a failed listing in plain words with its retry, never as "no jobs"', async () => {
    let answers = 0;
    await openJobs({
      [JOBS]: () => {
        answers += 1;
        return answers === 1
          ? fail(500, 'internal', 'the records could not be read')
          : listOf([jobOf('queued')]);
      },
    });
    await eventually(() =>
      screen.getByText('Listing this workspace’s jobs: Something went wrong in the service.'),
    );
    expect(screen.queryByText(/has no jobs yet/)).toBeNull();
    const again = screen.getByRole('button', { name: 'Try again' });
    again.focus();
    fireEvent.click(again);
    await listed();
    // Focus was on the retry, which is gone: it is on the first job it listed.
    expect(document.activeElement).toBe(rowOf());
  });

  it('reads "not available in this build" where jobs are not built, never as an empty list', async () => {
    await openJobs({});
    await eventually(() =>
      screen.getByText('Listing this workspace’s jobs: Not available in this build.'),
    );
    expect(screen.queryByText(/has no jobs yet/)).toBeNull();
  });

  it('brings more jobs with "Show more jobs", each once, and moves focus to the first of them', async () => {
    const { api } = await openJobs({
      [JOBS]: (body) =>
        (body.page as Json).cursor === 'next'
          ? listOf([jobOf('succeeded'), jobOf('succeeded', { id: SECOND })])
          : listOf([jobOf('succeeded')], 'next'),
    });
    await listed();
    const more = screen.getByRole('button', { name: 'Show more jobs' });
    more.focus();
    fireEvent.click(more);
    await eventually(() => rowOf(SECOND));
    expect(document.querySelectorAll('li[data-job]')).toHaveLength(2);
    expect(document.activeElement).toBe(rowOf(SECOND));
    expect(api.callsTo(JOBS).map((body) => (body.page as Json).cursor ?? null)).toEqual([
      null,
      'next',
    ]);
    expect(api.violations).toEqual([]);
  });
});

describe('how the list stays current without an event stream', () => {
  it('says why it cannot follow events, and asks for the stream once', async () => {
    const { api } = await openJobs({ [JOBS]: () => listOf([jobOf('succeeded')]) });
    await listed();
    await eventually(() =>
      screen.getByText(
        `While a job is waiting or running, this list is asked again every ${JOBS_POLL_MS / 1000} seconds. It cannot follow the workspace’s events now: Not available in this build.`,
      ),
    );
    await pass(MAX_RECONNECT_MS * 3);
    expect(api.calls.filter((call) => call.path === EVENTS)).toHaveLength(1);
  });

  it('asks again on the clock while a job has not ended, and stops when all have', async () => {
    let state: Job['state'] = 'running';
    const { api } = await openJobs({ [JOBS]: () => listOf([jobOf(state, { progress: 10 })]) });
    await listed();
    expect(api.callsTo(JOBS)).toHaveLength(1);
    await pass(JOBS_POLL_MS - 1);
    expect(api.callsTo(JOBS)).toHaveLength(1);
    state = 'succeeded';
    await pass(1);
    expect(api.callsTo(JOBS)).toHaveLength(2);
    await later(() => expect(rowOf().textContent).toContain('Finished.'));
    // The change is said, by the job's name.
    expect(screen.getByText('Import, job 88888888: finished.').getAttribute('role')).toBe('status');
    // Every listed job has ended: nothing more is asked, however long the page stays.
    await pass(JOBS_POLL_MS * 20);
    expect(api.callsTo(JOBS)).toHaveLength(2);
  });

  it('asks nothing from a hidden tab, and asks at once when the tab is shown again', async () => {
    const { api } = await openJobs({ [JOBS]: () => listOf([jobOf('running')]) });
    await listed();
    act(() => pageShown(false));
    await pass(JOBS_POLL_MS * 6);
    expect(api.callsTo(JOBS)).toHaveLength(1);
    act(() => pageShown(true));
    await later(() => expect(api.callsTo(JOBS)).toHaveLength(2));
    await pass(JOBS_POLL_MS);
    await later(() => expect(api.callsTo(JOBS)).toHaveLength(3));
  });

  it('asks at once on "List again", and keeps the jobs listed when that fails', async () => {
    let fails = false;
    const { api } = await openJobs({
      [JOBS]: () => (fails ? fail(503, 'unavailable', 'busy') : listOf([jobOf('succeeded')])),
    });
    await listed();
    fails = true;
    fireEvent.click(screen.getByRole('button', { name: 'List again' }));
    // A listing that failed for a reason that may pass is asked once more by itself, a moment later.
    await pass(2_000);
    await later(() =>
      screen.getByText('Listing the jobs again: The service is temporarily unavailable.'),
    );
    expect(api.callsTo(JOBS).length).toBeGreaterThan(1);
    // The job is still there, as last answered.
    expect(rowOf().textContent).toContain('Finished.');
  });
});

describe('how the list stays current with the event stream', () => {
  const streaming = async (jobs: () => Job[]) => {
    vi.useFakeTimers();
    const api = mockApi(
      explorerHandlers({ [OPEN]: () => ok(writableWorkspace), [JOBS]: () => listOf(jobs()) }),
    );
    const events = mockEvents();
    await showJobs();
    await listed();
    await eventually(() => screen.getByText(FOLLOWING));
    // The listing asked for once the stream had opened has been answered and drawn: from here on
    // a test counts what it causes itself. The clock has not moved.
    await eventually(() => expect(api.callsTo(JOBS)).toHaveLength(2));
    await eventually(() =>
      expect(screen.getByRole('button', { name: /^List/ }).textContent).toBe('List again'),
    );
    return { api, events };
  };

  it('asks once when the stream has opened, the first time too: a job that ended before then is not left running', async () => {
    vi.useFakeTimers();
    let state: Job['state'] = 'running';
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => listOf([jobOf(state)]),
      }),
    );
    const events = mockEvents();
    let open: () => void = () => undefined;
    events.state.hold = new Promise<void>((resolve) => {
      open = resolve;
    });
    await showJobs();
    // A millisecond at a time: nothing else draws the page while the stream is not open yet.
    await later(() => rowOf());
    expect(events.state.requests).toBe(1);
    expect(rowOf().textContent).toContain('Running.');
    expect(api.callsTo(JOBS)).toHaveLength(1);
    // The job ends now. Its event goes to nobody: the stream is asked for and not open yet.
    state = 'succeeded';
    open();
    await later(() => screen.getByText(FOLLOWING));
    // A few milliseconds have passed, no clock of the page's: it is the stream's opening that asked.
    await later(() => expect(rowOf().textContent).toContain('Finished.'));
    expect(api.callsTo(JOBS)).toHaveLength(2);
    expect(events.streams).toHaveLength(1);
  });

  it('gives up a first listing still in flight when the stream opens, and asks anew', async () => {
    vi.useFakeTimers();
    let state: Job['state'] = 'running';
    // The first listing is read by the service at once and answered late.
    let answerFirst: () => void = () => undefined;
    let listings = 0;
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => {
          listings += 1;
          if (listings > 1) return listOf([jobOf(state)]);
          const read = listOf([jobOf(state)]);
          return new Promise((resolve) => {
            answerFirst = () => resolve(read);
          });
        },
      }),
    );
    const events = mockEvents();
    let open: () => void = () => undefined;
    events.state.hold = new Promise<void>((resolve) => {
      open = resolve;
    });
    await showJobs();
    await eventually(() => expect(listings).toBe(1));
    // The job ends after that listing was read and before the stream is open.
    state = 'succeeded';
    open();
    await later(() => screen.getByText(FOLLOWING));
    await later(() => expect(rowOf().textContent).toContain('Finished.'));
    // The first answer, late as it is, changes nothing: its request was given up.
    answerFirst();
    await pass(50);
    expect(rowOf().textContent).toContain('Finished.');
    expect(api.callsTo(JOBS)).toHaveLength(2);
    expect(events.streams).toHaveLength(1);
  });

  it('asks when the stream says a job changed, and not on the quick clock while it is open', async () => {
    let state: Job['state'] = 'running';
    const { api, events } = await streaming(() => [jobOf(state)]);
    expect(events.streams).toHaveLength(1);
    expect(events.streams.at(0)?.lastEventId).toBeNull();
    const before = api.callsTo(JOBS).length;
    await pass(JOBS_POLL_MS * 5);
    expect(api.callsTo(JOBS)).toHaveLength(before);
    state = 'succeeded';
    act(() => events.streams.at(0)?.send('7', eventOf('job_updated', '7', { job_id: JOB_ID })));
    await eventually(() => expect(rowOf().textContent).toContain('Finished.'));
    expect(api.callsTo(JOBS)).toHaveLength(before + 1);
  });

  it('asks on the slow clock beside an open stream while a job has not ended, and on none once all have', async () => {
    let state: Job['state'] = 'running';
    const { api, events } = await streaming(() => [jobOf(state)]);
    const before = api.callsTo(JOBS).length;
    // No event comes: the stream looks open, and says nothing of the job.
    await pass(JOBS_SAFETY_POLL_MS - 1);
    expect(api.callsTo(JOBS)).toHaveLength(before);
    state = 'succeeded';
    await pass(1);
    expect(api.callsTo(JOBS)).toHaveLength(before + 1);
    await later(() => expect(rowOf().textContent).toContain('Finished.'));
    // Every listed job has ended: no clock at all, with the stream still open.
    await pass(JOBS_SAFETY_POLL_MS * 10);
    expect(api.callsTo(JOBS)).toHaveLength(before + 1);
    expect(events.streams.at(0)?.closed).toBe(false);
    expect(events.streams).toHaveLength(1);
  });

  it('does not take an answer that is no event stream for an open one: the quick clock goes on, and it is asked for again', async () => {
    vi.useFakeTimers();
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => listOf([jobOf('running')]),
      }),
    );
    // A proxy answers the stream's address with a page, and 200.
    const events = mockEvents();
    events.state.mediaType = 'text/html';
    await showJobs();
    await listed();
    await eventually(() =>
      screen.getByText(
        `While a job is waiting or running, this list is asked again every ${JOBS_POLL_MS / 1000} seconds. It cannot follow the workspace’s events now: the answer was not an event stream.`,
      ),
    );
    expect(screen.queryByText(FOLLOWING)).toBeNull();
    expect(events.state.requests).toBe(1);
    // Nothing was asked because of it; the quick clock asks, as with no stream.
    expect(api.callsTo(JOBS)).toHaveLength(1);
    await pass(RECONNECT_MS);
    await later(() => expect(events.state.requests).toBe(2));
    await pass(JOBS_POLL_MS - RECONNECT_MS);
    await later(() => expect(api.callsTo(JOBS)).toHaveLength(2));
    expect(screen.queryByText(FOLLOWING)).toBeNull();
    // The real stream, once it is there, is followed.
    events.state.mediaType = 'text/event-stream';
    await pass(MAX_RECONNECT_MS);
    await later(() => screen.getByText(FOLLOWING));
    expect(events.streams).toHaveLength(1);
  });

  it('asks again for an imported event and for one it cannot read, not for another kind', async () => {
    const { api, events } = await streaming(() => [jobOf('running')]);
    const stream = events.streams.at(0);
    const before = api.callsTo(JOBS).length;
    act(() => stream?.send('1', eventOf('reviewed', '1')));
    await pass(0);
    await pass(0);
    expect(api.callsTo(JOBS)).toHaveLength(before);
    act(() => stream?.send('2', eventOf('imported', '2')));
    await eventually(() => expect(api.callsTo(JOBS)).toHaveLength(before + 1));
    act(() => stream?.send('3', { something: 'else' }));
    await eventually(() => expect(api.callsTo(JOBS)).toHaveLength(before + 2));
  });

  it('opens the stream again after it ends, from the last event it saw, and asks once', async () => {
    const { api, events } = await streaming(() => [jobOf('running')]);
    act(() => events.streams.at(0)?.send('41', eventOf('reviewed', '41')));
    await pass(0);
    const before = api.callsTo(JOBS).length;
    act(() => events.streams.at(0)?.end());
    await eventually(() =>
      screen.getByText(
        `While a job is waiting or running, this list is asked again every ${JOBS_POLL_MS / 1000} seconds.`,
      ),
    );
    await pass(RECONNECT_MS - 1);
    expect(events.streams).toHaveLength(1);
    await pass(1);
    await later(() => expect(events.streams).toHaveLength(2));
    expect(events.streams.at(1)?.lastEventId).toBe('41');
    await later(() => screen.getByText(FOLLOWING));
    // What changed while it was away is asked for.
    await later(() => expect(api.callsTo(JOBS).length).toBeGreaterThan(before));
  });

  it('tries again later when the stream fails for a reason that may pass', async () => {
    vi.useFakeTimers();
    mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => listOf([jobOf('succeeded')]),
      }),
    );
    const events = mockEvents(fail(503, 'unavailable', 'starting'));
    await showJobs();
    await listed();
    await eventually(() =>
      screen.getByText(
        /It cannot follow the workspace’s events now: The service is temporarily unavailable\./,
      ),
    );
    expect(events.state.requests).toBe(1);
    events.state.refusal = null;
    await pass(RECONNECT_MS);
    await eventually(() => expect(events.streams).toHaveLength(1));
    expect(events.state.requests).toBe(2);
  });

  it('waits longer before each new try of the stream, up to a bound, and starts over after an event', async () => {
    vi.useFakeTimers();
    mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => listOf([jobOf('succeeded')]),
      }),
    );
    const events = mockEvents(fail(503, 'unavailable', 'starting'));
    await showJobs();
    await listed();
    await eventually(() => expect(events.state.requests).toBe(1));
    // Each wait is twice the one before, and never more than the bound.
    const waits = [
      RECONNECT_MS,
      2 * RECONNECT_MS,
      4 * RECONNECT_MS,
      8 * RECONNECT_MS,
      MAX_RECONNECT_MS,
      MAX_RECONNECT_MS,
    ];
    expect(8 * RECONNECT_MS).toBeLessThan(MAX_RECONNECT_MS);
    expect(16 * RECONNECT_MS).toBeGreaterThan(MAX_RECONNECT_MS);
    let requests = 1;
    for (const wait of waits) {
      await pass(wait - 250);
      expect(events.state.requests).toBe(requests);
      requests += 1;
      await pass(250);
      await later(() => expect(events.state.requests).toBe(requests));
    }
    // The stream opens at last, says something, and ends: the next try comes after the first
    // wait again, not after the longest.
    events.state.refusal = null;
    await pass(MAX_RECONNECT_MS);
    await later(() => expect(events.streams).toHaveLength(1));
    act(() => events.streams.at(0)?.send('5', eventOf('reviewed', '5')));
    await pass(0);
    act(() => events.streams.at(0)?.end());
    await pass(RECONNECT_MS - 250);
    expect(events.streams).toHaveLength(1);
    await pass(250);
    await later(() => expect(events.streams).toHaveLength(2));
    expect(events.streams.at(1)?.lastEventId).toBe('5');
  });

  it('holds no stream while the page is hidden, and opens it again when the page is shown', async () => {
    const { api, events } = await streaming(() => [jobOf('running')]);
    const before = api.callsTo(JOBS).length;
    act(() => pageShown(false));
    await eventually(() => expect(events.streams.at(0)?.closed).toBe(true));
    await pass(JOBS_SAFETY_POLL_MS * 4);
    expect(events.streams).toHaveLength(1);
    // Nor is anything asked on a clock while it is hidden, the slow one included.
    expect(api.callsTo(JOBS)).toHaveLength(before);
    act(() => pageShown(true));
    await eventually(() => expect(events.streams).toHaveLength(2));
    // Shown again: the stream opens, and the jobs are asked.
    await later(() => expect(api.callsTo(JOBS).length).toBeGreaterThan(before));
  });

  it('closes the stream when the page is left', async () => {
    const { events } = await streaming(() => [jobOf('running')]);
    fireEvent.click(screen.getByRole('link', { name: 'Documents' }));
    await eventually(() => expect(events.streams.at(0)?.closed).toBe(true));
  });
});

describe('a session that ended while the jobs are followed', () => {
  const SIGNED_OUT =
    'You are not signed in, so this list is not asked again by itself. Sign in, then press “List again”.';
  const refused = () => fail(401, 'unauthenticated', 'no session');

  it('stops the clock when the service says the person is not signed in, keeps the rows, and starts again at the person’s retry', async () => {
    let signedIn = true;
    const { api } = await openJobs({
      [JOBS]: () => (signedIn ? listOf([jobOf('running')]) : refused()),
    });
    await listed();
    signedIn = false;
    await pass(JOBS_POLL_MS);
    await later(() => screen.getByText('Listing the jobs again: You are not signed in.'));
    const asked = api.callsTo(JOBS).length;
    expect(asked).toBe(2);
    // Ten minutes on, with the page shown: nothing more was asked.
    await pass(600_000);
    expect(api.callsTo(JOBS)).toHaveLength(asked);
    // The job is still there as last answered, and the page says why it does not move.
    expect(rowOf().textContent).toContain('Running.');
    expect(screen.getByText(SIGNED_OUT)).toBeDefined();
    // Signed in again, the person asks: the list is answered and the clock runs again.
    signedIn = true;
    fireEvent.click(screen.getByRole('button', { name: 'Try again after signing in' }));
    await later(() => expect(api.callsTo(JOBS)).toHaveLength(asked + 1));
    await later(() => expect(screen.queryByText(SIGNED_OUT)).toBeNull());
    await pass(JOBS_POLL_MS);
    await later(() => expect(api.callsTo(JOBS)).toHaveLength(asked + 2));
  });

  it('asks for the stream no more once it is refused as not signed in, and again after the person’s "List again"', async () => {
    vi.useFakeTimers();
    let signedIn = true;
    const api = mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => (signedIn ? listOf([jobOf('running')]) : refused()),
      }),
    );
    const events = mockEvents();
    await showJobs();
    await listed();
    await eventually(() => screen.getByText(FOLLOWING));
    await eventually(() => expect(api.callsTo(JOBS)).toHaveLength(2));
    // The session ends: the service closes the stream and refuses the next.
    signedIn = false;
    events.state.refusal = refused();
    act(() => events.streams.at(0)?.end());
    await pass(RECONNECT_MS);
    await later(() => expect(events.state.requests).toBe(2));
    await later(() => screen.getByText(SIGNED_OUT));
    // The quick clock asks the list once more, is refused, and stops too.
    await pass(JOBS_POLL_MS);
    await later(() => screen.getByText('Listing the jobs again: You are not signed in.'));
    const asked = api.callsTo(JOBS).length;
    await pass(600_000);
    expect(events.state.requests).toBe(2);
    expect(api.callsTo(JOBS)).toHaveLength(asked);
    expect(rowOf().textContent).toContain('Running.');
    // Signed in again: one press asks the list, and with its answer the stream is asked for.
    signedIn = true;
    events.state.refusal = null;
    fireEvent.click(screen.getByRole('button', { name: 'List again' }));
    await later(() => expect(events.streams).toHaveLength(2));
    await later(() => screen.getByText(FOLLOWING));
    expect(screen.queryByText(SIGNED_OUT)).toBeNull();
  });

  it('goes on trying the stream for a failure that may pass by itself', async () => {
    vi.useFakeTimers();
    mockApi(
      explorerHandlers({
        [OPEN]: () => ok(writableWorkspace),
        [JOBS]: () => listOf([jobOf('succeeded')]),
      }),
    );
    const events = mockEvents(fail(500, 'internal', 'restarting'));
    await showJobs();
    await listed();
    await eventually(() => expect(events.state.requests).toBe(1));
    await pass(RECONNECT_MS);
    await later(() => expect(events.state.requests).toBe(2));
    expect(screen.queryByText(SIGNED_OUT)).toBeNull();
  });
});

describe('cancelling a job, and trying a failed one again', () => {
  const button = (name: string, id = JOB_ID) => within(rowOf(id)).queryByRole('button', { name });

  it('offers Cancel only while a job is waiting or running, and a retry only for a failed job', async () => {
    const ids = ['a', 'b', 'c', 'd', 'e'].map(
      (letter) => `${letter.repeat(8)}-8888-4888-8888-888888888888`,
    );
    const states: Job['state'][] = ['queued', 'running', 'succeeded', 'failed', 'cancelled'];
    await openJobs({
      [JOBS]: () => listOf(states.map((state, index) => jobOf(state, { id: ids[index] ?? '' }))),
    });
    await eventually(() => rowOf(ids[0]));
    const offered = ids.map((id) => [
      button('Cancel', id) !== null,
      button('Try the job again', id) !== null,
    ]);
    expect(offered).toEqual([
      [true, false],
      [true, false],
      [false, false],
      [false, true],
      [false, false],
    ]);
  });

  it('offers a retry of a failed job only when the service says it will take it', async () => {
    const ids = ['a', 'b', 'c'].map((letter) => `${letter.repeat(8)}-8888-4888-8888-888888888888`);
    const failed = (id: string, retryable: boolean | undefined) => {
      const job = jobOf('failed', { id });
      if (retryable === undefined) {
        const { retryable: _left, ...older } = job;
        return older;
      }
      return { ...job, retryable };
    };
    await openJobs({
      [JOBS]: () =>
        listOf([
          failed(ids[0] ?? '', true),
          failed(ids[1] ?? '', false),
          failed(ids[2] ?? '', undefined),
        ]),
    });
    await eventually(() => rowOf(ids[0]));
    expect(ids.map((id) => button('Try the job again', id) !== null)).toEqual([true, false, false]);
    expect(rowOf(ids[1]).textContent).toContain('The service will not take this job again.');
    expect(rowOf(ids[2]).textContent).not.toContain('will not take');
  });

  it('offers control of a job only to a person with the permission of what started it', async () => {
    const purge = jobOf('running', { kind: 'purge_workspace' });
    const withRights = (permissions: Workspace['permissions']) => ({
      ...writableWorkspace,
      permissions,
    });
    await openJobs({ [JOBS]: () => listOf([purge]) }, withRights(['read', 'write']));
    await listed();
    // A purge is started by `purge_workspace`, which needs `admin`: a writer may not cancel it.
    expect(button('Cancel')).toBeNull();
    cleanup();
    vi.useRealTimers();
    await openJobs({ [JOBS]: () => listOf([purge]) }, withRights(['read', 'admin']));
    await listed();
    expect(button('Cancel')).not.toBeNull();
  });

  it('says when a job was accepted, started and ended, as the wire gives them', async () => {
    await openJobs({
      [JOBS]: () =>
        listOf([
          jobOf('succeeded', {
            created_at: '2026-01-02T08:15:00.000Z',
            started_at: '2026-01-02T08:16:00.000Z',
            finished_at: '2026-01-02T08:20:00.000Z',
          }),
        ]),
    });
    await listed();
    const times = [...rowOf().querySelectorAll('time')].map((time) =>
      time.getAttribute('datetime'),
    );
    expect(times).toEqual([
      '2026-01-02T08:15:00.000Z',
      '2026-01-02T08:16:00.000Z',
      '2026-01-02T08:20:00.000Z',
    ]);
    expect(rowOf().textContent).toMatch(/Accepted .+; Started .+; Ended .+\./);
  });

  it('offers neither to a person who may only read', async () => {
    await openJobs(
      { [JOBS]: () => listOf([jobOf('running'), jobOf('failed', { id: SECOND })]) },
      workspace,
    );
    await listed();
    expect(screen.queryByRole('button', { name: 'Cancel' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Try the job again' })).toBeNull();
  });

  it('asks before cancelling: the safe answer has focus and gives it back to Cancel', async () => {
    const { api } = await openJobs({ [JOBS]: () => listOf([jobOf('running')]) });
    await listed();
    const cancel = button('Cancel');
    cancel?.focus();
    if (cancel) fireEvent.click(cancel);
    expect(within(rowOf()).getByRole('group', { name: 'Cancel this job?' })).toBeDefined();
    expect(document.activeElement).toBe(button('Keep it running'));
    const keep = button('Keep it running');
    if (keep) fireEvent.click(keep);
    expect(within(rowOf()).queryByRole('group')).toBeNull();
    expect(document.activeElement).toBe(button('Cancel'));
    expect(api.callsTo(CANCEL)).toEqual([]);
  });

  it('cancels, shows the job as the service answered, and puts focus on the job', async () => {
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf('running', { progress: 30 })]),
      [CANCEL]: () => ok(jobOf('cancelled', { progress: 30 })),
    });
    await listed();
    const cancel = button('Cancel');
    cancel?.focus();
    if (cancel) fireEvent.click(cancel);
    const confirm = button('Cancel the job');
    confirm?.focus();
    if (confirm) fireEvent.click(confirm);
    await eventually(() => expect(rowOf().textContent).toContain('Cancelled.'));
    expect(api.callsTo(CANCEL)).toEqual([
      {
        workspace_id: WS,
        job_id: JOB_ID,
        idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
      },
    ]);
    // A cancelled job offers no cancel again, and no question.
    expect(button('Cancel')).toBeNull();
    expect(within(rowOf()).queryByRole('group')).toBeNull();
    expect(document.activeElement).toBe(rowOf());
    expect(screen.getByText('Import, job 88888888: cancelled.').getAttribute('role')).toBe(
      'status',
    );
    expect(api.violations).toEqual([]);
  });

  it('keeps the job as it was when the cancel fails, and repeats the same request', async () => {
    let answers = 0;
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf('running')]),
      [CANCEL]: () => {
        answers += 1;
        return answers === 1 ? fail(503, 'unavailable', 'busy') : ok(jobOf('cancelled'));
      },
    });
    await listed();
    const cancel = button('Cancel');
    if (cancel) fireEvent.click(cancel);
    const confirm = button('Cancel the job');
    if (confirm) fireEvent.click(confirm);
    await eventually(() =>
      screen.getByText('Cancelling this job: The service is temporarily unavailable.'),
    );
    expect(rowOf().textContent).toContain('Running.');
    const again = within(rowOf()).getByRole('button', { name: 'Try again' });
    fireEvent.click(again);
    await eventually(() => expect(rowOf().textContent).toContain('Cancelled.'));
    const sent = api.callsTo(CANCEL);
    expect(sent).toHaveLength(2);
    expect(sent.at(1)).toEqual(sent.at(0));
  });

  it('puts focus on the job when it ends by itself while Cancel has focus', async () => {
    let state: Job['state'] = 'running';
    await openJobs({ [JOBS]: () => listOf([jobOf(state)]) });
    await listed();
    button('Cancel')?.focus();
    state = 'succeeded';
    await pass(JOBS_POLL_MS);
    await later(() => expect(rowOf().textContent).toContain('Finished.'));
    expect(button('Cancel')).toBeNull();
    expect(document.activeElement).toBe(rowOf());
  });

  it('takes the question away when the job ends while it is asked, with focus on the job', async () => {
    let state: Job['state'] = 'running';
    await openJobs({ [JOBS]: () => listOf([jobOf(state)]) });
    await listed();
    const cancel = button('Cancel');
    cancel?.focus();
    if (cancel) fireEvent.click(cancel);
    expect(document.activeElement).toBe(button('Keep it running'));
    state = 'failed';
    await pass(JOBS_POLL_MS);
    await later(() => expect(rowOf().textContent).toContain('Failed.'));
    expect(within(rowOf()).queryByRole('group')).toBeNull();
    expect(document.activeElement).toBe(rowOf());
  });

  it('asks for another try of a failed job, under a new key at each press', async () => {
    let state: Job['state'] = 'failed';
    let answers = 0;
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf(state, { attempt: 1 })]),
      [RETRY]: () => {
        answers += 1;
        if (answers === 1) return fail(503, 'unavailable', 'busy');
        state = 'queued';
        return accepted(jobOf('queued', { attempt: 2 }));
      },
    });
    await listed();
    const press = button('Try the job again');
    press?.focus();
    if (press) fireEvent.click(press);
    await eventually(() =>
      screen.getByText(
        'Asking for another try of this job: The service is temporarily unavailable.',
      ),
    );
    // The job is still shown as failed: nothing was queued.
    expect(rowOf().textContent).toContain('Failed.');
    const repeat = within(rowOf()).getByRole('button', { name: 'Try again' });
    repeat.focus();
    fireEvent.click(repeat);
    await eventually(() => expect(rowOf().textContent).toContain('Waiting to start, attempt 2.'));
    const first = api.callsTo(RETRY);
    expect(first).toHaveLength(2);
    expect(first.at(0)).toEqual({
      workspace_id: WS,
      job_id: JOB_ID,
      idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
    });
    // The repeat of the request is the same request.
    expect(first.at(1)).toEqual(first.at(0));
    // The retry button is gone with the failed state; focus is on the job.
    expect(button('Try the job again')).toBeNull();
    expect(document.activeElement).toBe(rowOf());

    // It runs and fails again: another press is another request.
    state = 'failed';
    await pass(JOBS_POLL_MS);
    await later(() => expect(rowOf().textContent).toContain('Failed.'));
    const pressAgain = button('Try the job again');
    if (pressAgain) fireEvent.click(pressAgain);
    await eventually(() => expect(api.callsTo(RETRY)).toHaveLength(3));
    expect(api.callsTo(RETRY).at(2)?.idempotency_key).not.toBe(first.at(0)?.idempotency_key);
    expect(api.violations).toEqual([]);
  });

  it('reads the job again after a retry whose answer was lost, and sends a later press under a new key', async () => {
    let state: Job['state'] = 'failed';
    let attempt = 1;
    let answers = 0;
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf(state, { attempt })]),
      [RETRY]: () => {
        answers += 1;
        if (answers === 1) {
          // Carried out; the answer is lost on the way.
          state = 'queued';
          attempt = 2;
          return fail(503, 'unavailable', 'the answer was lost');
        }
        return accepted(jobOf(state, { attempt }));
      },
    });
    await listed();
    const press = button('Try the job again');
    if (press) fireEvent.click(press);
    await eventually(() => expect(api.callsTo(RETRY)).toHaveLength(1));
    // Nobody pressed "List again", and the clock has not come round: the failure itself made
    // the page ask, and the row shows what the service holds.
    await later(() => expect(rowOf().textContent).toContain('Waiting to start, attempt 2.'));
    expect(button('Try the job again')).toBeNull();
    // It runs and fails again. A new press is a new request, not the lost one's key again (the
    // service would answer that key with the job as it stands and queue nothing).
    state = 'failed';
    await pass(JOBS_POLL_MS);
    await later(() => expect(rowOf().textContent).toContain('Failed, attempt 2.'));
    const pressAgain = button('Try the job again');
    if (pressAgain) fireEvent.click(pressAgain);
    await eventually(() => expect(api.callsTo(RETRY)).toHaveLength(2));
    expect(api.callsTo(RETRY).at(1)?.idempotency_key).not.toBe(
      api.callsTo(RETRY).at(0)?.idempotency_key,
    );
  });

  it('reads the job again after a cancel that failed, so the row shows what the service holds', async () => {
    let state: Job['state'] = 'running';
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf(state)]),
      [CANCEL]: () => {
        // Carried out; the answer is lost on the way.
        state = 'cancelled';
        return fail(503, 'unavailable', 'the answer was lost');
      },
    });
    await listed();
    const before = api.callsTo(JOBS).length;
    const cancel = button('Cancel');
    if (cancel) fireEvent.click(cancel);
    const confirm = button('Cancel the job');
    if (confirm) fireEvent.click(confirm);
    await eventually(() =>
      screen.getByText('Cancelling this job: The service is temporarily unavailable.'),
    );
    await later(() => expect(rowOf().textContent).toContain('Cancelled.'));
    expect(api.callsTo(JOBS).length).toBeGreaterThan(before);
    // A second press of a cancel is another request under another key, were there one to press.
    expect(button('Cancel')).toBeNull();
    expect(within(rowOf()).queryByRole('group')).toBeNull();
  });

  it('sends each press of "Cancel the job" under a new key, and the notice’s retry under the failed one’s', async () => {
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf('running')]),
      [CANCEL]: () => fail(503, 'unavailable', 'busy'),
    });
    await listed();
    const cancel = button('Cancel');
    if (cancel) fireEvent.click(cancel);
    const confirm = () => within(rowOf()).getByRole('button', { name: 'Cancel the job' });
    fireEvent.click(confirm());
    await eventually(() => expect(api.callsTo(CANCEL)).toHaveLength(1));
    await eventually(() => expect(confirm().getAttribute('aria-disabled')).toBe('false'));
    // The question is still there (the job is still running): pressed again, a new request.
    fireEvent.click(confirm());
    await eventually(() => expect(api.callsTo(CANCEL)).toHaveLength(2));
    await eventually(() => expect(confirm().getAttribute('aria-disabled')).toBe('false'));
    const keys = () => api.callsTo(CANCEL).map((body) => body.idempotency_key);
    expect(keys().at(1)).not.toBe(keys().at(0));
    // The notice's retry repeats the request that failed last.
    fireEvent.click(within(rowOf()).getByRole('button', { name: 'Try again' }));
    await eventually(() => expect(api.callsTo(CANCEL)).toHaveLength(3));
    expect(keys().at(2)).toBe(keys().at(1));
  });

  it('says that the job changed when a cancel meets a conflict, reads it again, and offers no repeat', async () => {
    let state: Job['state'] = 'running';
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf(state)]),
      [CANCEL]: () => {
        // The job ended between the listing and the press.
        state = 'succeeded';
        return fail(409, 'conflict', 'the job is not running');
      },
    });
    await listed();
    const cancel = button('Cancel');
    if (cancel) fireEvent.click(cancel);
    const confirm = button('Cancel the job');
    if (confirm) fireEvent.click(confirm);
    await eventually(() =>
      screen.getByText('Cancelling this job: The job changed before this could be done.'),
    );
    expect(rowOf().textContent).toContain(
      'The job was read again: its row shows how it stands now',
    );
    expect(rowOf().textContent).toContain('The service said: the job is not running');
    expect(screen.queryByText(/This changed since it was loaded/)).toBeNull();
    // No way to send the same cancel again; the row is what the service holds.
    expect(within(rowOf()).queryByRole('button', { name: /Reload|Try again/ })).toBeNull();
    await later(() => expect(rowOf().textContent).toContain('Finished.'));
    expect(button('Cancel')).toBeNull();
    expect(api.callsTo(CANCEL)).toHaveLength(1);
  });

  it('says that the job changed when a retry meets a conflict, reads it again, and offers no repeat', async () => {
    let state: Job['state'] = 'failed';
    const { api } = await openJobs({
      [JOBS]: () => listOf([jobOf(state, { attempt: state === 'failed' ? 1 : 2 })]),
      [RETRY]: () => {
        // Someone else asked for another try a moment before.
        state = 'running';
        return fail(409, 'conflict', 'the job is not failed');
      },
    });
    await listed();
    const press = button('Try the job again');
    if (press) fireEvent.click(press);
    await eventually(() =>
      screen.getByText(
        'Asking for another try of this job: The job changed before this could be done.',
      ),
    );
    expect(within(rowOf()).queryByRole('button', { name: /Reload|Try again/ })).toBeNull();
    await later(() => expect(rowOf().textContent).toContain('Running, attempt 2.'));
    expect(button('Try the job again')).toBeNull();
    expect(api.callsTo(RETRY)).toHaveLength(1);
  });

  it('puts focus on the job when another try is accepted and its button leaves', async () => {
    await openJobs({
      [JOBS]: () => listOf([jobOf('failed')]),
      [RETRY]: () => accepted(jobOf('queued', { attempt: 2 })),
    });
    await listed();
    const press = button('Try the job again');
    press?.focus();
    if (press) fireEvent.click(press);
    await eventually(() => expect(rowOf().textContent).toContain('Waiting to start, attempt 2.'));
    expect(button('Try the job again')).toBeNull();
    expect(document.activeElement).toBe(rowOf());
  });

  it('reads a refused retry in plain words, never as queued', async () => {
    await openJobs({ [JOBS]: () => listOf([jobOf('failed')]) });
    await listed();
    const press = button('Try the job again');
    if (press) fireEvent.click(press);
    await eventually(() =>
      screen.getByText('Asking for another try of this job: Not available in this build.'),
    );
    expect(rowOf().textContent).toContain('Failed.');
  });
});
