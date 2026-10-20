/**
 * Installation jobs (ruling C1): the installation's own jobs (`list_tenant_jobs`), its backups and
 * the purges of its workspaces, newest first as the service lists them, each as its row says it
 * (`TenantJobRow`): its state in words, and for a finished backup of the installation the download
 * of its archive at the installation's own path. A workspace's jobs are on that workspace's Jobs
 * page.
 *
 * Paged by the service's cursor: "Show more installation jobs" asks the next page from the cursor
 * the last one answered, and moves keyboard focus to the first job it brought (or keeps it on a
 * control still there). Nothing walks the pages by itself. How the list stays current is said on
 * the page (`tenant-jobs.ts`: asked again every few seconds while a job on screen is waiting or
 * running); "List again" asks at once, and a change of a job's state is said in one polite
 * status, by the job's name.
 *
 * The session is read here (`get_session`), so a finished backup's download is offered by its
 * route as the artifact's rule says (a person's own session). A listing that failed with nothing
 * listed is a failure notice with its retry; one that failed with jobs on screen keeps them, as
 * last answered, with its notice. A `forbidden` is said as what it means here: an installation's
 * administrator rights (`installation-words.ts`).
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { Job, JobState } from '../../../api/generated/types.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { sessionOptions } from '../../imports/limits';
import { jobName, jobStateSaid } from '../../jobs/job-words';
import { JOBS_POLL_MS } from '../../jobs/jobs';
import { tenantJobsWords } from './installation-words';
import { TenantJobRow } from './TenantJobRow';
import { tenantJobsOptions } from './tenant-jobs';

/** The jobs of every page listed, each once: a list that moved between two pages repeats none. */
function listedOnce(pages: ReadonlyArray<{ items: Job[] }> | undefined): Job[] {
  const byId = new Map<string, Job>();
  for (const page of pages ?? []) {
    for (const job of page.items) if (!byId.has(job.id)) byId.set(job.id, job);
  }
  return [...byId.values()];
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

export function InstallationJobs() {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  // The session's route decides where a backup may be downloaded (`TenantArtifactDownload`).
  useQuery(sessionOptions());
  const jobs = useInfiniteQuery(tenantJobsOptions());
  const pages = jobs.data?.pages;
  const listed = useMemo(() => listedOnce(pages), [pages]);
  const said = useStateChanges(listed);

  const rowAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>(':scope > li').item(index) ?? null;
  const nothingListed = jobs.data === undefined;
  const listFailure = useRetry({
    request: 'list_tenant_jobs',
    failed: jobs.isError && nothingListed,
    error: jobs.error,
    run: () => jobs.refetch(),
    loaded: () => rowAt(0) ?? headingRef.current,
  });
  const laterFailure = useRetry({
    request: 'list_tenant_jobs',
    failed: jobs.isError && !nothingListed,
    error: jobs.error,
    run: () => (jobs.isFetchNextPageError ? jobs.fetchNextPage() : jobs.refetch()),
    loaded: () => rowAt(0) ?? headingRef.current,
  });

  // "Show more" moves keyboard focus to the first job it brought, else stays on a control.
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
    <section aria-labelledby={headingId} className="mt-8 grid gap-2">
      <h2 id={headingId} ref={headingRef} tabIndex={-1} className="page-heading">
        Installation jobs
      </h2>
      <p>
        The installation’s own jobs, newest first: its backups, and the purges of its workspaces. A
        workspace’s other jobs are on that workspace’s Jobs page. These are followed here, not
        cancelled or tried again: the service does that for a workspace’s jobs only.
      </p>
      <p className="meta">
        While a job here is waiting or running, this list is asked again every {JOBS_POLL_MS / 1000}{' '}
        seconds.
      </p>
      <p>
        {/* aria-disabled rather than disabled: the button keeps focus while the list is asked. */}
        <button
          type="button"
          aria-disabled={jobs.isFetching}
          onClick={() => {
            if (!jobs.isFetching) void jobs.refetch();
          }}
        >
          List again
        </button>
      </p>
      {jobs.isPending && !listFailure.retrying && (
        <p role="status">Listing the installation’s jobs…</p>
      )}
      {listFailure.shown && (
        <FailureNotice
          context="Listing the installation’s jobs"
          error={listFailure.error}
          retry={listFailure}
          words={tenantJobsWords(listFailure.error)}
        />
      )}
      {jobs.isSuccess && listed.length === 0 && <p>This installation has no jobs yet.</p>}
      {listed.length > 0 && (
        <ul ref={listRef} className="job-list">
          {listed.map((job) => (
            <TenantJobRow key={job.id} job={job} />
          ))}
        </ul>
      )}
      {jobs.hasNextPage && (
        <p>
          {/* aria-disabled rather than disabled: the button keeps focus while the page is asked. */}
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
            {loadingMore ? 'Loading more installation jobs…' : 'Show more installation jobs'}
          </button>
        </p>
      )}
      {laterFailure.shown && (
        <FailureNotice
          context={
            moreFailed
              ? 'Listing more of the installation’s jobs'
              : 'Listing the installation’s jobs'
          }
          error={laterFailure.error}
          retry={laterFailure}
          words={tenantJobsWords(laterFailure.error)}
        />
      )}
      {/* Present from the start, so a change of state is announced. */}
      <p role="status" className="sr-only">
        {said}
      </p>
    </section>
  );
}
