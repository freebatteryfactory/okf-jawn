/**
 * A workspace's jobs in the cache: the list (`list_jobs`, page by page through its cursor, newest
 * first as the service orders them) and single jobs (`get_job`), and what the cache is told when a
 * job changes.
 *
 * A job is the service's record of durable work; its acknowledgement is not completion. Nothing
 * here guesses a state: a job is shown in the state its last answer reported.
 *
 * How the list stays current (see `feed.ts` for the event stream):
 * - while the workspace's event stream is open, the jobs are asked again when it says one changed;
 * - otherwise the jobs are asked again every `JOBS_POLL_MS` while one that is listed is waiting or
 *   running, and not at all once every listed job has ended;
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
  queryOptions,
} from '@tanstack/react-query';
import { z } from 'zod';
import { getJobOptions, listJobsQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { listJobs } from '../../api/generated/sdk.gen';
import type { Job, ListJobsResponse } from '../../api/generated/types.gen';

/** Jobs requested per page. */
export const JOBS_PAGE = 50;

/** How often the jobs are asked again while one is waiting or running and no event stream is open. */
export const JOBS_POLL_MS = 5_000;

type JobPages = InfiniteData<ListJobsResponse, string | null>;

/** A job that has not ended: the only kind worth asking about again. */
export const unfinished = (job: Job) => job.state === 'queued' || job.state === 'running';

export function jobsQueryKey(workspaceId: string) {
  const body = { workspace_id: workspaceId, page: { limit: JOBS_PAGE } };
  return [{ ...listJobsQueryKey({ body })[0], _infinite: true }] as const;
}

/** The workspace's jobs. `live`: its event stream is open, so nothing is asked on a clock. */
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
      !live && (query.state.data?.pages ?? []).some((page) => page.items.some(unfinished))
        ? JOBS_POLL_MS
        : false,
  });
}

/** One job. `live` as for the list. */
export function jobOptions(workspaceId: string, jobId: string, live: boolean) {
  return queryOptions({
    ...getJobOptions({ body: { workspace_id: workspaceId, job_id: jobId } }),
    staleTime: JOBS_POLL_MS,
    refetchOnWindowFocus: true,
    refetchInterval: (query) =>
      !live && query.state.data !== undefined && unfinished(query.state.data)
        ? JOBS_POLL_MS
        : false,
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

/** The workspace's event stream said its jobs changed: ask for them again. */
export function jobsChanged(client: QueryClient, workspaceId: string) {
  void client.invalidateQueries({ queryKey: jobsQueryKey(workspaceId) });
  void client.invalidateQueries({ predicate: (query) => readsJob(query.queryKey, workspaceId) });
}
