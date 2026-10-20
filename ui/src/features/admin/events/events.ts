/**
 * The two event logs in the cache (ruling C8): a workspace's (`list_events`) and the
 * installation's (`list_tenant_events`), each read oldest first, `EVENTS_PAGE` events a page, as
 * the service records them.
 *
 * A page is asked only when the person asks for it; nothing walks the log. The first page is read
 * from the start of the log. The next is read from the service's continuation: the request's
 * `page.cursor` set to the answer's `next_cursor`, which the service reads before `after`
 * (`crates/core/src/application/events.rs`). A page without a continuation was the end of the
 * log when it was read; newer events are then looked for after the last event listed, the
 * request's `after` (its "last seen event cursor"), or after the place the empty page was read
 * from. A page may hold fewer events than asked, or none, and still carry a continuation: the
 * service hides a workspace's security events from a caller without `admin` there.
 *
 * The pages stay while the section is shown and are let go once it is not (`gcTime: 0`): the
 * next visit reads from the start again, and nothing is asked again by itself meanwhile (a log
 * only grows, so the pages listed stay true).
 */
import { type InfiniteData, infiniteQueryOptions } from '@tanstack/react-query';
import { listEventsQueryKey } from '../../../api/generated/@tanstack/react-query.gen';
import { listEvents, listTenantEvents } from '../../../api/generated/sdk.gen';
import type { ListEventsResponse, PageRequest } from '../../../api/generated/types.gen';

/** Events asked for per page. */
export const EVENTS_PAGE = 50;

/** Where a page is read from: the service's continuation, or after an event (null: the start). */
export type EventsFrom = { cursor: string } | { after: string | null };

/** The pages listed, as `useInfiniteQuery` holds them. */
export type EventPages = InfiniteData<ListEventsResponse>;

/** The paging fields of a request reading from `from`. */
function paging(from: EventsFrom): { page: PageRequest; after?: string } {
  if ('cursor' in from) return { page: { limit: EVENTS_PAGE, cursor: from.cursor } };
  return from.after === null
    ? { page: { limit: EVENTS_PAGE } }
    : { page: { limit: EVENTS_PAGE }, after: from.after };
}

/**
 * Where the page after `last` (read from `from`) is read from: its continuation, else after its
 * last event, else after where it was read from. Always somewhere: past the end, newer events.
 */
export function nextFrom(last: ListEventsResponse, from: EventsFrom): EventsFrom {
  if (last.next_cursor !== null && last.next_cursor !== undefined && last.next_cursor !== '') {
    return { cursor: last.next_cursor };
  }
  const lastEvent = last.events.at(-1);
  if (lastEvent !== undefined) return { after: lastEvent.id };
  return 'cursor' in from ? { after: from.cursor } : from;
}

/** Whether the last page listed was the end of the log when it was read. */
export function atEnd(pages: EventPages | undefined): boolean {
  const last = pages?.pages.at(-1);
  return last !== undefined && (last.next_cursor === null || last.next_cursor === undefined);
}

const START: EventsFrom = { after: null };

/** A workspace's log. */
export function workspaceEventsOptions(workspaceId: string) {
  const first = { workspace_id: workspaceId, page: { limit: EVENTS_PAGE } };
  return infiniteQueryOptions({
    queryKey: [{ ...listEventsQueryKey({ body: first })[0], _infinite: true }] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listEvents({
        body: { workspace_id: workspaceId, ...paging(pageParam) },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: START,
    getNextPageParam: (last, _all, from) => nextFrom(last, from),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
  });
}

/** The installation's log. The generated helpers offer no query key for it; this is its own. */
export function installationEventsOptions() {
  return infiniteQueryOptions({
    queryKey: [
      { _id: 'listTenantEvents', body: { page: { limit: EVENTS_PAGE } }, _infinite: true },
    ] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listTenantEvents({
        body: paging(pageParam),
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: START,
    getNextPageParam: (last, _all, from) => nextFrom(last, from),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
  });
}
