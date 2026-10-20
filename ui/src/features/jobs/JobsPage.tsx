/**
 * The one place that lists the workspace's jobs: imports, conversions, exports and the rest, newest
 * first as the service lists them, each as its row shows it (`JobRow`).
 *
 * How the list stays current is said on the page, in the words of what is happening: it follows
 * the workspace's event stream while that is open (`feed.ts`), with a slow clock beside it while a
 * listed job is waiting or running; otherwise it is asked again every few seconds while one is,
 * and on no clock once all have ended (`jobs.ts`). "List again" asks at once. A hidden tab asks
 * nothing. When the service answers that the person is not signed in, nothing is asked again by
 * itself: the rows stay as last answered, the failure is shown with its retry, and the page says
 * that it has stopped asking. The first answer after that starts the clock and the stream again.
 *
 * A change of a job's state is said in one polite status, by the job's name. The time the list
 * was last answered is shown beside it, outside the status, so a repeated listing that changed
 * nothing is not announced.
 *
 * A listing that failed with nothing listed is a failure notice with its retry. One that failed
 * with jobs on screen (a later page, or a repeat) keeps them as last answered, with its notice.
 * "Show more jobs" moves keyboard focus to the first job it brought.
 *
 * The page also starts an export of the workspace (`ExportWorkspace`, above the list): the job it
 * answers is told to the cache and listed here as every job is.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { Job, JobState } from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, messageFor } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { ExportWorkspace } from '../exports/ExportWorkspace';
import { useWorkspaceChanges } from '../tree/context';
import { type Feed, NotAnEventStream, useJobsFeed } from './feed';
import { JobRow } from './JobRow';
import { jobName, jobStateSaid } from './job-words';
import { JOBS_POLL_MS, JOBS_SAFETY_POLL_MS, jobsOptions, sessionEnded } from './jobs';

export interface JobsPageProps {
  workspaceId: string;
}

/** How the list stays current, in words. `signedOut`: the service said the session has ended. */
export function describeFeed(feed: Feed, signedOut: boolean): string {
  if (signedOut) {
    return 'You are not signed in, so this list is not asked again by itself. Sign in, then press “List again”.';
  }
  if (feed.state === 'live') {
    return `This list follows the workspace’s events: it is asked again when a job changes, and every ${JOBS_SAFETY_POLL_MS / 1000} seconds while a job is waiting or running.`;
  }
  const clock = `While a job is waiting or running, this list is asked again every ${JOBS_POLL_MS / 1000} seconds.`;
  if (feed.error === null || feed.error === undefined) return clock;
  // The service was reached and answered: the general words for an unknown error would say it was not.
  if (feed.error instanceof NotAnEventStream) {
    return `${clock} It cannot follow the workspace’s events now: the answer was not an event stream.`;
  }
  return `${clock} It cannot follow the workspace’s events now: ${messageFor(classifyFailure(feed.error)).title}`;
}

/** Say each job whose state differs from the one last seen. */
function useStateChanges(jobs: readonly Job[]): string {
  const seen = useRef(new Map<string, JobState>());
  const [said, setSaid] = useState('');
  useEffect(() => {
    const changes: string[] = [];
    for (const job of jobs) {
      const before = seen.current.get(job.id);
      if (before !== undefined && before !== job.state) {
        changes.push(`${jobName(job)}: ${jobStateSaid(job.state)}.`);
      }
      seen.current.set(job.id, job.state);
    }
    if (changes.length > 0) setSaid(changes.join(' '));
  }, [jobs]);
  return said;
}

/** The jobs of every page listed, each once: a list that moved between two pages repeats none. */
function listedOnce(pages: ReadonlyArray<{ items: Job[] }> | undefined): Job[] {
  const byId = new Map<string, Job>();
  for (const page of pages ?? []) {
    for (const job of page.items) if (!byId.has(job.id)) byId.set(job.id, job);
  }
  return [...byId.values()];
}

export function JobsPage({ workspaceId }: JobsPageProps) {
  const heading = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const { head, rights } = useWorkspaceChanges();
  const feed = useJobsFeed(workspaceId);
  const jobs = useInfiniteQuery(jobsOptions(workspaceId, feed.live));
  const signedOut = sessionEnded(jobs.error) || sessionEnded(feed.error);
  // The jobs were answered: a stream that had stopped for a session that ended is asked for again.
  const { wake } = feed;
  const answeredAt = jobs.dataUpdatedAt;
  useEffect(() => {
    if (answeredAt > 0) wake();
  }, [answeredAt, wake]);
  const pages = jobs.data?.pages;
  const listed = useMemo(() => listedOnce(pages), [pages]);
  const said = useStateChanges(listed);

  const rowAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>(':scope > li').item(index) ?? null;
  const nothingListed = jobs.data === undefined;
  const listFailure = useRetry({
    request: workspaceId,
    failed: jobs.isError && nothingListed,
    error: jobs.error,
    run: () => jobs.refetch(),
    loaded: () => rowAt(0) ?? headingRef.current,
  });
  const laterFailure = useRetry({
    request: workspaceId,
    failed: jobs.isError && !nothingListed,
    error: jobs.error,
    run: () => (jobs.isFetchNextPageError ? jobs.fetchNextPage() : jobs.refetch()),
    loaded: () => rowAt(0) ?? headingRef.current,
  });

  // "Show more jobs" moves keyboard focus to the first job it brought, else stays on a control.
  const focusFrom = useRef<{ shown: number; pages: number } | null>(null);
  const pageCount = pages?.length ?? 0;
  const moreFailed = jobs.isFetchNextPageError;
  const loadingMore = jobs.isFetchingNextPage;
  const shownCount = listed.length;
  useEffect(() => {
    const asked = focusFrom.current;
    // Settled once another page is here (even an empty one), or the next page failed.
    if (asked === null || !(pageCount > asked.pages || (moreFailed && !loadingMore))) return;
    focusFrom.current = null;
    (rowAt(asked.shown) ?? moreRef.current ?? rowAt(shownCount - 1))?.focus();
  });

  return (
    <section className="workspace-page" aria-labelledby={heading}>
      <h2 id={heading} ref={headingRef} tabIndex={-1} className="page-heading">
        Jobs
      </h2>
      <ExportWorkspace workspaceId={workspaceId} head={head} rights={rights} />
      <p className="meta">{describeFeed(feed, signedOut)}</p>
      <p className="page-actions">
        {/* aria-disabled rather than disabled: the button keeps focus while the list is asked. */}
        <button
          type="button"
          aria-disabled={jobs.isFetching}
          onClick={() => {
            if (!jobs.isFetching) void jobs.refetch();
          }}
        >
          {jobs.isFetching && !jobs.isPending ? 'Listing again…' : 'List again'}
        </button>{' '}
        {jobs.dataUpdatedAt > 0 && (
          <span className="meta">
            Listed at <Instant value={new Date(jobs.dataUpdatedAt).toISOString()} />.
          </span>
        )}
      </p>
      {/* Present from the start, so a change of a job's state is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
      {jobs.isPending && !listFailure.retrying && <p role="status">Listing the jobs…</p>}
      {listFailure.shown && (
        <FailureNotice
          context="Listing this workspace’s jobs"
          error={listFailure.error}
          retry={listFailure}
          workspaceId={workspaceId}
        />
      )}
      {pages !== undefined && listed.length === 0 && (
        <p>This workspace has no jobs yet. An import or an export started here will be listed.</p>
      )}
      {listed.length > 0 && (
        <ul ref={listRef} className="job-list" aria-labelledby={heading}>
          {listed.map((job) => (
            <JobRow key={job.id} workspaceId={workspaceId} job={job} />
          ))}
        </ul>
      )}
      {laterFailure.shown && (
        <FailureNotice
          context="Listing the jobs again"
          error={laterFailure.error}
          retry={laterFailure}
          workspaceId={workspaceId}
        />
      )}
      {jobs.hasNextPage && (
        <button
          type="button"
          ref={moreRef}
          aria-disabled={loadingMore}
          onClick={() => {
            if (loadingMore) return;
            focusFrom.current = { shown: shownCount, pages: pageCount };
            void jobs.fetchNextPage();
          }}
        >
          {loadingMore ? 'Listing more jobs…' : 'Show more jobs'}
        </button>
      )}
    </section>
  );
}
