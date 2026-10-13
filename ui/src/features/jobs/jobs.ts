/**
 * A workspace's jobs in the cache: the list (`list_jobs`, page by page through its cursor, newest
 * first as the service orders them) and single jobs (`get_job`), and what the cache is told when a
 * job changes.
 *
 * A job is the service's record of durable work; its acknowledgement is not completion. Nothing
 * here guesses a state: a job is shown in the state its last answer reported.
 *
 * How the jobs on screen stay current (see `feed.ts` for the event stream):
 * - while the workspace's event stream is open, the jobs are asked again when it says one changed,
 *   once each time it opens, and every `JOBS_SAFETY_POLL_MS` while one on screen is waiting or
 *   running: an event can be lost while the stream still looks open;
 * - otherwise the jobs are asked again every `JOBS_POLL_MS` while one on screen is waiting or
 *   running;
 * - either way, not at all on a clock once every job on screen has ended;
 * - a hidden tab asks nothing either way (TanStack Query holds an interval while the page is
 *   hidden; the stream is closed while it is);
 * - returning to the window, or to the page, asks once when what is held is older than
 *   `JOBS_POLL_MS`. A job the service has just answered (`rememberJob`) is shown from that answer
 *   and not asked for again at once.
 */
import {
  type InfiniteData,
  infiniteQueryOptions,
  type QueryClient,
  type QueryFilters,
  queryOptions,
} from '@tanstack/react-query';
import { z } from 'zod';
import { getJobOptions, listJobsQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { listJobs } from '../../api/generated/sdk.gen';
import type { Job, JobState, ListJobsResponse } from '../../api/generated/types.gen';

/** Jobs requested per page. */
export const JOBS_PAGE = 50;

/** How often the jobs are asked again while one is waiting or running and no event stream is open. */
export const JOBS_POLL_MS = 5_000;

/**
 * How often they are asked while one is waiting or running and the event stream is open. The
 * stream is the way they stay current; this clock only bounds how long a job can be shown in a
 * state it has left when an event did not arrive (a proxy that holds events back, a change the
 * service announced to a stream that had just been replaced). 30 seconds: six times slower than
 * the clock without a stream, the same as the longest wait before the stream is opened again, and
 * one request a time for each page of jobs or each started job on screen.
 */
export const JOBS_SAFETY_POLL_MS = 30_000;

/** The clock for jobs on screen: none once all are known to have ended. */
function pollEvery(live: boolean, anyMayChange: boolean): number | false {
  if (!anyMayChange) return false;
  return live ? JOBS_SAFETY_POLL_MS : JOBS_POLL_MS;
}

type JobPages = InfiniteData<ListJobsResponse, string | null>;

/** A job that is waiting or running: the states in which the wire's progress means something. */
export const unfinished = (job: Job) => job.state === 'queued' || job.state === 'running';

/** The states in which a job has ended, as this build's types name them. */
const ENDED: ReadonlySet<string> = new Set<JobState>(['succeeded', 'failed', 'cancelled']);

/**
 * A job worth asking about again: one that is not known to have ended. A state this build does
 * not know is not known to be an end, so such a job is asked about like one that is running.
 */
export const mayChange = (job: Job) => !ENDED.has(job.state);

export function jobsQueryKey(workspaceId: string) {
  const body = { workspace_id: workspaceId, page: { limit: JOBS_PAGE } };
  return [{ ...listJobsQueryKey({ body })[0], _infinite: true }] as const;
}

/** The workspace's jobs. `live`: its event stream is open, so the clock is the slow one. */
export function jobsOptions(workspaceId: string, live: boolean) {
  return infiniteQueryOptions({
    queryKey: jobsQueryKey(workspaceId),
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listJobs({
        body: { workspace_id: workspaceId, page: { limit: JOBS_PAGE, cursor: pageParam } },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    staleTime: JOBS_POLL_MS,
    refetchOnWindowFocus: true,
    refetchInterval: (query) =>
      pollEvery(
        live,
        (query.state.data?.pages ?? []).some((page) => page.items.some(mayChange)),
      ),
  });
}

/** One job. `live` as for the list. */
export function jobOptions(workspaceId: string, jobId: string, live: boolean) {
  return queryOptions({
    ...getJobOptions({ body: { workspace_id: workspaceId, job_id: jobId } }),
    staleTime: JOBS_POLL_MS,
    refetchOnWindowFocus: true,
    refetchInterval: (query) =>
      pollEvery(live, query.state.data !== undefined && mayChange(query.state.data)),
  });
}

const zJobKey = z.object({
  _id: z.literal('getJob'),
  body: z.object({ workspace_id: z.string() }),
});

/** A cached `get_job` of this workspace. */
function readsJob(queryKey: readonly unknown[], workspaceId: string): boolean {
  const key = zJobKey.safeParse(queryKey.at(0));
  return key.success && key.data.body.workspace_id === workspaceId;
}

/**
 * Tell the cache what the service just answered about a job (`start_import`, `cancel_job`,
 * `retry_job`): its own entry, and its row of the list when it is listed. A job the list does not
 * hold yet makes the list be asked again.
 */
export function rememberJob(client: QueryClient, workspaceId: string, job: Job) {
  client.setQueryData(
    getJobOptions({ body: { workspace_id: workspaceId, job_id: job.id } }).queryKey,
    job,
  );
  const key = jobsQueryKey(workspaceId);
  const listed = client.getQueryData<JobPages>(key);
  if (listed === undefined) return;
  if (!listed.pages.some((page) => page.items.some((entry) => entry.id === job.id))) {
    void client.invalidateQueries({ queryKey: key });
    return;
  }
  client.setQueryData<JobPages>(key, {
    ...listed,
    pages: listed.pages.map((page) => ({
      ...page,
      items: page.items.map((entry) => (entry.id === job.id ? job : entry)),
    })),
  });
}

/**
 * Ask again for what `filters` names. A request in flight was sent before the reason to ask, so
 * its answer may be the older one: it is given up and asked anew. TanStack Query does that by
 * itself for a query that holds an answer; a first request it would wait for instead (`Query.fetch`
 * cancels only when there is data), so that one is cancelled here.
 */
async function askAgain(client: QueryClient, filters: QueryFilters) {
  await client.cancelQueries({
    ...filters,
    predicate: (query) => query.state.data === undefined && (filters.predicate?.(query) ?? true),
  });
  await client.invalidateQueries(filters);
}

/**
 * A request about one job failed, so what the service now holds of it is not known (it may have
 * been carried out with its answer lost, or refused because the job had moved on): ask for the
 * job again, as a single job and as a row of the list, whichever is on screen.
 */
export function jobUncertain(client: QueryClient, workspaceId: string, jobId: string) {
  void askAgain(client, { queryKey: jobsQueryKey(workspaceId) });
  void askAgain(client, {
    queryKey: getJobOptions({ body: { workspace_id: workspaceId, job_id: jobId } }).queryKey,
  });
}

/**
 * The workspace's event stream opened, or said its jobs changed: ask for them again, the list and
 * each single job.
 */
export function jobsChanged(client: QueryClient, workspaceId: string) {
  void askAgain(client, { queryKey: jobsQueryKey(workspaceId) });
  void askAgain(client, { predicate: (query) => readsJob(query.queryKey, workspaceId) });
}
