/**
 * One read of an item, as the reader asks it (ruling A1; SPEC section 7: "Browser caching keys
 * include workspace, source, revision, representation and selection").
 *
 * A read is one `read_item` request, and its browser cache entry is keyed by that whole request
 * (`readItemInfiniteQueryKey`): the workspace (`workspace_id`), the item (`item_id`), the exact
 * revision (`at`, always a resolved revision here: "the latest" is resolved once, by the pinned
 * revision's notice), the representation (`view`) and the selection, with the budgets. So two
 * reads that differ in any one of those never share an entry, and a continuation (`next_cursor`)
 * is a further page of the entry it continues: the same revision and representation.
 *
 * The budgets: text in parts of `READ_BUDGET_BYTES`, each further part asked with the cursor the
 * last one gave; images (pictures, page renders) at most `IMAGE_BUDGET` per read, never more than
 * the contract's `MAX_IMAGES`. Images the budget leaves out are said to be by the service's own
 * warning (`images_omitted`); the pages view offers the pages after those shown as a selection of
 * their own, so a page render is always one selection away.
 */
import { readItemInfiniteOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { ReadItemRequest, ReadView, Revision, Selection } from '../../api/generated/types.gen';

/** Text budget per read; a longer item is read in parts through the continuation. */
export const READ_BUDGET_BYTES = 65_536;

/** Images asked at first, in a view that shows them. */
export const IMAGE_BUDGET = 8;

/** The most images one read may ask (`ReadItemRequest.max_images`, at most 16). */
export const MAX_IMAGES = 16;

/** The views whose answers carry images: pictures, and page renders. */
export function showsImages(view: ReadView): view is 'multimodal' | 'pages' {
  return view === 'multimodal' || view === 'pages';
}

export interface ReadAsked {
  workspaceId: string;
  itemId: string;
  /** The exact revision read. */
  revision: Revision;
  view: ReadView;
  selection: Selection;
  /** Images asked, in a view that shows them; `IMAGE_BUDGET` when left out. */
  maxImages?: number;
}

/** The first request of a read: no cursor, the reader's budgets. */
export function readRequest({
  workspaceId,
  itemId,
  revision,
  view,
  selection,
  maxImages = IMAGE_BUDGET,
}: ReadAsked): ReadItemRequest {
  return {
    workspace_id: workspaceId,
    item_id: itemId,
    at: { kind: 'revision', revision },
    view,
    selection,
    cursor: null,
    max_bytes: READ_BUDGET_BYTES,
    max_images: showsImages(view) ? Math.min(Math.max(maxImages, 0), MAX_IMAGES) : 0,
  };
}

/** The read's query: one cache entry per request, each further part asked with its cursor. */
export function readOptions(request: ReadItemRequest) {
  return {
    ...readItemInfiniteOptions({ body: request }),
    initialPageParam: { body: request },
    getNextPageParam: (last: { next_cursor?: string | null }) => last.next_cursor ?? undefined,
  };
}

/** The browser cache key of a read. */
export function readKey(request: ReadItemRequest) {
  return readItemInfiniteOptions({ body: request }).queryKey;
}
