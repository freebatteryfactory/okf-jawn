/**
 * The installation's jobs in the cache (`list_tenant_jobs`): its backups and the purges of its
 * workspaces, newest first as the service orders them, page by page through its cursor. A job of
 * the installation belongs to no workspace (`Job.workspace_id` is absent), so no workspace's event
 * stream says when one changes: the list is asked again every `JOBS_POLL_MS` while a job on
 * screen is not known to have ended (`mayChange`), not at all on a clock once every one has,
 * nothing more once the service answered that the person is not signed in, and when the person
 * returns to the window after `JOBS_POLL_MS`. A hidden tab asks nothing on the clock (TanStack
 * Query's interval does not run in the background).
 *
 * A job the page just started (`backup_installation`) is told to the cache by asking the list
 * again (`rememberTenantJob`): the list says how it stands, the one place it is shown.
 */
import { infiniteQueryOptions, type QueryClient } from '@tanstack/react-query';
import { listTenantJobs } from '../../../api/generated/sdk.gen';
import { JOBS_PAGE, JOBS_POLL_MS, mayChange, sessionEnded } from '../../jobs/jobs';

/** The cache's name for the installation's jobs. */
export function tenantJobsQueryKey() {
  return [
    { _id: 'listTenantJobs', body: { page: { limit: JOBS_PAGE } }, _infinite: true },
  ] as const;
}

export function tenantJobsOptions() {
  return infiniteQueryOptions({
    queryKey: tenantJobsQueryKey(),
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listTenantJobs({
        body: { page: { limit: JOBS_PAGE, cursor: pageParam } },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    staleTime: JOBS_POLL_MS,
    refetchOnWindowFocus: true,
    refetchInterval: (query) => {
      const anyMayChange = (query.state.data?.pages ?? []).some((page) =>
        page.items.some(mayChange),
      );
      return anyMayChange && !sessionEnded(query.state.error) ? JOBS_POLL_MS : false;
    },
  });
}

/** The service accepted a job of the installation: the list is asked again, from its first page. */
export function rememberTenantJob(client: QueryClient) {
  void client.invalidateQueries({ queryKey: tenantJobsQueryKey() });
}
