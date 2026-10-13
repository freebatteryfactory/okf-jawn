/**
 * The person's own drafts in a workspace, from `list_drafts`, and what the cache says of a draft
 * once it changes.
 *
 * `list_drafts` is paged through its cursor; the pages are one infinite query, most recently saved
 * first as the service orders them. A draft is not bound to a revision: `get_item` returns the
 * caller's draft beside the committed content at whatever revision was asked for. So when a draft
 * is saved, discarded or snapshotted, every cached `get_item` of that item is told, and the list
 * is listed again unless the change is one it can take in place (a later save of a draft it
 * already lists).
 *
 * An answer that started before such a write never undoes it: the reads of the item still in
 * flight are cancelled before the cache is told, and asked again after, so what they then answer
 * has the write in it.
 */
import {
  type InfiniteData,
  infiniteQueryOptions,
  type QueryClient,
  useInfiniteQuery,
} from '@tanstack/react-query';
import { z } from 'zod';
import { listDraftsQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { listDrafts } from '../../api/generated/sdk.gen';
import type {
  Draft,
  DraftContent,
  ItemDocument,
  ListDraftsResponse,
} from '../../api/generated/types.gen';

/** Drafts requested per page: the most the operation allows. */
export const DRAFTS_PAGE = 200;

type DraftPages = InfiniteData<ListDraftsResponse, string | null>;

export function draftsQueryKey(workspaceId: string) {
  const body = { workspace_id: workspaceId, page: { limit: DRAFTS_PAGE } };
  return [{ ...listDraftsQueryKey({ body })[0], _infinite: true }] as const;
}

export function draftsOptions(workspaceId: string) {
  return infiniteQueryOptions({
    queryKey: draftsQueryKey(workspaceId),
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listDrafts({
        body: { workspace_id: workspaceId, page: { limit: DRAFTS_PAGE, cursor: pageParam } },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** The drafts listed so far. Its further pages are asked for by the Drafts panel alone. */
export function useDrafts(workspaceId: string) {
  return useInfiniteQuery(draftsOptions(workspaceId));
}

/** The listed drafts, by item. */
export function draftsByItem(pages: readonly ListDraftsResponse[] | undefined): Map<string, Draft> {
  const drafts = new Map<string, Draft>();
  for (const page of pages ?? []) for (const draft of page.items) drafts.set(draft.item_id, draft);
  return drafts;
}

/** The draft of an item as the list last answered it; undefined when it lists none. */
export function listedDraft(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
): Draft | undefined {
  const listed = client.getQueryData<DraftPages>(draftsQueryKey(workspaceId));
  return draftsByItem(listed?.pages).get(itemId);
}

const zItemKey = z.object({
  _id: z.literal('getItem'),
  body: z.object({ workspace_id: z.string(), item_id: z.string() }),
});

/** A cached `get_item` of this item, at whatever revision. */
function readsItem(queryKey: readonly unknown[], workspaceId: string, itemId: string): boolean {
  const key = zItemKey.safeParse(queryKey.at(0));
  return (
    key.success && key.data.body.workspace_id === workspaceId && key.data.body.item_id === itemId
  );
}

/**
 * Stop the reads of these items that are in flight: each began before the write the cache is
 * about to be told of, and its answer would put the old draft back after it. Returns what to do
 * once the cache is told: ask for those reads again.
 */
function stopReads(client: QueryClient, workspaceId: string, itemIds: readonly string[]) {
  const reads = client.getQueryCache().findAll({
    predicate: (query) =>
      query.state.fetchStatus === 'fetching' &&
      itemIds.some((itemId) => readsItem(query.queryKey, workspaceId, itemId)),
  });
  if (reads.length === 0) return () => {};
  const stopped = { predicate: (query: (typeof reads)[number]) => reads.includes(query) };
  // The read is taken back to what it held before it began, at once; the write goes on top.
  void client.cancelQueries(stopped);
  return () => void client.refetchQueries(stopped);
}

/** Tell the cache that the person has no draft of these items any more: they were snapshotted. */
export function forgetDrafts(client: QueryClient, workspaceId: string, itemIds: readonly string[]) {
  const readAgain = stopReads(client, workspaceId, itemIds);
  client.setQueriesData<ItemDocument>(
    {
      predicate: (query) =>
        itemIds.some((itemId) => readsItem(query.queryKey, workspaceId, itemId)),
    },
    (document) => (document === undefined ? document : { ...document, draft: null }),
  );
  readAgain();
  void client.invalidateQueries({ queryKey: draftsQueryKey(workspaceId) });
}

/**
 * Tell the cache what the server now holds as the person's draft of an item: `draft`, or null
 * once it is discarded.
 */
export function rememberDraft(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  draft: DraftContent | null,
): void {
  const readAgain = stopReads(client, workspaceId, [itemId]);
  client.setQueriesData<ItemDocument>(
    { predicate: (query) => readsItem(query.queryKey, workspaceId, itemId) },
    (document) => (document === undefined ? document : { ...document, draft }),
  );
  readAgain();
  const key = draftsQueryKey(workspaceId);
  const listed = client.getQueryData<DraftPages>(key);
  const known = draft !== null && draftsByItem(listed?.pages).has(itemId);
  if (listed === undefined || !known) {
    void client.invalidateQueries({ queryKey: key });
    return;
  }
  client.setQueryData<DraftPages>(key, {
    ...listed,
    pages: listed.pages.map((page) => ({
      ...page,
      items: page.items.map((entry) => (entry.item_id === itemId ? draft.draft : entry)),
    })),
  });
}
