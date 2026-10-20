/**
 * History in the cache: the Timeline (`log_items`), Changes between two versions (`diff_items`),
 * Who changed each line (`blame_item`), and Rewind (`restore_items`).
 *
 * Every read names a resolved revision, never "the latest": a Timeline is asked at the revision
 * the workspace (or the tab) shows, and a continuation walks back from the tip its first page
 * resolved (SPEC section 7: "pagination stays tied to the same revision"). What a resolved
 * revision holds never changes, so an answer is kept for as long as the page shows it and is
 * not asked for again by itself. A new head (Refresh, the person's own change) is another
 * revision, and asks anew.
 *
 * Rewind restores the state of one item, or of the whole workspace, as it was at an earlier
 * version, as a new commit on the head the workspace shows (SPEC section 8: "Rewind restores
 * selected historical state in a new snapshot and does not erase history"). It is sent as the
 * tree's changes are (`tree/changes.ts`): on the shown head as its base, repeated under its key
 * while it says the same thing; the revision it answers becomes the head the workspace shows
 * when the workspace still shows that base, and the head checks run again.
 */
import { infiniteQueryOptions, type QueryClient } from '@tanstack/react-query';
import {
  blameItemQueryKey,
  diffItemsOptions,
  logItemsQueryKey,
} from '../../api/generated/@tanstack/react-query.gen';
import { blameItem, logItems, restoreItems } from '../../api/generated/sdk.gen';
import type { ReceiptId, Revision, Warning } from '../../api/generated/types.gen';
import { adoptHeadFrom, checkHeadsAgain, type HeadChange } from '../explorer/head';
import type { Attempt } from '../tree/changes';

/** Versions asked for per page of a Timeline. */
export const LOG_PAGE = 50;

/** Lines asked for per page of Who. */
export const WHO_PAGE = 200;

/** The Timeline of a workspace, or of one item when `itemId` is given, from `revision` back. */
export function logOptions(workspaceId: string, revision: Revision, itemId: string | null) {
  const body = {
    workspace_id: workspaceId,
    at: { kind: 'revision' as const, revision },
    ...(itemId === null ? {} : { item_id: itemId }),
    page: { limit: LOG_PAGE },
  };
  return infiniteQueryOptions({
    queryKey: [{ ...logItemsQueryKey({ body })[0], _infinite: true }] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await logItems({
        // The first page names no cursor; each later one, the one the page before answered.
        body: {
          ...body,
          page: pageParam === null ? { limit: LOG_PAGE } : { limit: LOG_PAGE, cursor: pageParam },
        },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // A resolved revision's history does not change.
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** Changes from `from` to `to`, of one item when `itemId` is given. */
export function diffOptions(
  workspaceId: string,
  from: Revision,
  to: Revision,
  itemId: string | null,
) {
  return {
    ...diffItemsOptions({
      body: {
        workspace_id: workspaceId,
        from,
        to,
        ...(itemId === null ? {} : { item_id: itemId }),
      },
    }),
    staleTime: Number.POSITIVE_INFINITY,
  };
}

/**
 * Who last changed each of an item's first `lines` lines at `revision`, `WHO_PAGE` lines a page.
 * The lines are the item's text without its properties header (the service numbers them so).
 */
export function whoOptions(workspaceId: string, itemId: string, revision: Revision, lines: number) {
  const body = {
    workspace_id: workspaceId,
    item_id: itemId,
    at: { kind: 'revision' as const, revision },
  };
  return infiniteQueryOptions({
    queryKey: [
      {
        ...blameItemQueryKey({ body: { ...body, lines: { start: 1, end: lines } } })[0],
        _infinite: true,
      },
    ] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await blameItem({
        body: {
          ...body,
          lines: { start: pageParam, end: Math.min(lines, pageParam + WHO_PAGE - 1) },
        },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: 1,
    getNextPageParam: (_last, _pages, start) =>
      start + WHO_PAGE <= lines ? start + WHO_PAGE : undefined,
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** How many lines a text has, as the service numbers them: a last line break opens no line. */
export function lineCount(text: string): number {
  if (text.length === 0) return 0;
  const lines = text.split('\n').length;
  return text.endsWith('\n') ? lines - 1 : lines;
}

/** What a Rewind restores: an item, or the whole workspace when `itemId` is null. */
export interface RewindValues {
  target: Revision;
  itemId: string | null;
  message: string;
}

/** A Rewind done: the head change, and what the commit answered beside its revision. */
export interface Rewound extends HeadChange {
  warnings: Warning[];
  receiptId: ReceiptId;
}

/** Rewind, on the attempt's base, under its key. */
export async function rewind(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<RewindValues>,
): Promise<Rewound> {
  const { target, itemId, message } = attempt.values;
  const { data } = await restoreItems({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      target,
      ...(itemId === null ? {} : { item_id: itemId }),
      message,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  const { previousHead, adopted } = adoptHeadFrom(client, workspaceId, data.revision, attempt.base);
  checkHeadsAgain(client, workspaceId);
  return {
    revision: data.revision,
    previousHead,
    itemIds: itemId === null ? [] : [itemId],
    adopted,
    warnings: data.warnings,
    receiptId: data.receipt_id,
  };
}
