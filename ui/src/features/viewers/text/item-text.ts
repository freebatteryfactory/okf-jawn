/**
 * An item's text as the reader reads it (`read_item`, `view: 'text'`), for a viewer that shows it
 * another way: the spreadsheet's tables (`sheet/SpreadsheetView.tsx`) and the converted text
 * beside its original (`OriginalBesideDigest.tsx`).
 *
 * The request is the reader's own (`documents/DocumentView.tsx`): the exact revision, the
 * selection, no cursor, the same text budget and no images. So while the two agree, the reader
 * and a viewer share one read in the cache, keyed by workspace, item, revision, representation and
 * selection, and a part read by either is there for both. A partial read is continued with its
 * cursor, one part each time the person asks (`next`); nothing here reads on by itself.
 *
 * The parts of one read are consecutive pieces of the same selected text, each cut at a line end
 * when one fits (`crates/core/src/application/reads.rs`, `budget`), so joined in order they are
 * that text as far as it has been read.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { readItemInfiniteOptions } from '../../../api/generated/@tanstack/react-query.gen';
import type {
  ReadItemRequest,
  ReadItemResponse,
  Revision,
  Selection,
} from '../../../api/generated/types.gen';

/** The reader's text budget per part (`READ_BUDGET_BYTES` of `documents/DocumentView.tsx`). */
export const TEXT_READ_BUDGET_BYTES = 65_536;

/** The reader's request for the item's text at `revision`, of `selection`. */
export function textReadRequest(
  workspaceId: string,
  itemId: string,
  revision: Revision,
  selection: Selection,
): ReadItemRequest {
  return {
    workspace_id: workspaceId,
    item_id: itemId,
    at: { kind: 'revision', revision },
    view: 'text',
    selection,
    cursor: null,
    max_bytes: TEXT_READ_BUDGET_BYTES,
    max_images: 0,
  };
}

/** The read of an item's text, in the parts read so far. */
export function useItemText(
  workspaceId: string,
  itemId: string,
  revision: Revision,
  selection: Selection,
) {
  const request = textReadRequest(workspaceId, itemId, revision, selection);
  const read = useInfiniteQuery({
    ...readItemInfiniteOptions({ body: request }),
    initialPageParam: { body: request },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  return { request, read };
}

/** The selected text as far as it has been read: the parts joined in order. */
export function joinedText(pages: readonly ReadItemResponse[]): string {
  return pages.map((page) => page.markdown).join('');
}

/**
 * The words of a reading's live region: what the last continuation did. While a failure notice
 * is trying the continuation again, the notice says so and this line stays as it was.
 */
export function continuationStatus(
  read: { isFetchingNextPage: boolean; isFetchNextPageError: boolean; hasNextPage: boolean },
  parts: number,
  retrying: boolean,
): string {
  if (read.isFetchingNextPage && !retrying) return 'Reading more…';
  if (read.isFetchNextPageError) return 'The rest of this reading could not be read.';
  if (parts > 1) return `Part ${parts} read${read.hasNextPage ? '; more can be read' : ''}.`;
  return '';
}
