/**
 * Attention: the workspace's advisory warnings (`get_attention`), in the cache, and their words.
 *
 * The wire lists the warnings of a whole workspace at one revision, page by page; a warning may
 * name the item it is about. There is no per-item request and no location inside an item, so an
 * item's warnings are those of the list that name it, and a link from a warning goes to the item,
 * pinned to the revision the answer says was inspected.
 *
 * They are asked at the revision the workspace shows, so the pages agree with each other and with
 * the tree. `ATTENTION_AUTO_PAGES` pages are asked without being asked for, so that the marks in
 * the tree and on the tabs cover them; the rest come when the person asks for more. Marks
 * therefore cover the warnings listed so far, and the Attention page says when more exist.
 *
 * What a warning is not. These are hints from automatic checks: advisory, with false alarms
 * expected (SPEC section 11). A list that comes back empty says only that the checks found
 * nothing: it is never worded as the workspace being safe, clean or correct. A list that could not
 * be read is a failure, never an empty list.
 */
import { type InfiniteData, infiniteQueryOptions, useInfiniteQuery } from '@tanstack/react-query';
import { getAttentionQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { getAttention } from '../../api/generated/sdk.gen';
import type {
  AttentionItem,
  AttentionKind,
  GetAttentionResponse,
  OperationName,
  Revision,
} from '../../api/generated/types.gen';
import { wordsFor } from '../../lib/words';

/** Warnings requested per page: the most the operation allows. */
export const ATTENTION_PAGE = 200;

/** Pages asked without being asked for: the marks cover this many warnings by themselves. */
export const ATTENTION_AUTO_PAGES = 5;

export type AttentionPages = InfiniteData<GetAttentionResponse, string | null>;

export function attentionOptions(workspaceId: string, head: Revision) {
  const body = {
    workspace_id: workspaceId,
    at: { kind: 'revision' as const, revision: head },
    page: { limit: ATTENTION_PAGE },
  };
  return infiniteQueryOptions({
    queryKey: [{ ...getAttentionQueryKey({ body })[0], _infinite: true }] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await getAttention({
        body: { ...body, page: { limit: ATTENTION_PAGE, cursor: pageParam } },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** The warnings listed so far. Its further pages are asked for by the workspace's Attention link. */
export function useAttention(workspaceId: string, head: Revision) {
  return useInfiniteQuery(attentionOptions(workspaceId, head));
}

/** Every warning of the pages listed, in the service's order. */
export function warningsOf(pages: readonly GetAttentionResponse[] | undefined): AttentionItem[] {
  return (pages ?? []).flatMap((page) => page.items);
}

/** The listed warnings that name this item. */
export function warningsFor(warnings: readonly AttentionItem[], itemId: string): AttentionItem[] {
  return warnings.filter((warning) => warning.item_id === itemId);
}

/** How many listed warnings name each item: the mark an item carries where it is named. */
export function useAttentionMarks(workspaceId: string, head: Revision): (itemId: string) => number {
  const warnings = warningsOf(useAttention(workspaceId, head).data?.pages);
  const counts = new Map<string, number>();
  for (const warning of warnings) {
    const itemId = warning.item_id ?? null;
    if (itemId !== null) counts.set(itemId, (counts.get(itemId) ?? 0) + 1);
  }
  return (itemId) => counts.get(itemId) ?? 0;
}

/** The mark's words: how many advisory warnings name the item. */
export function attentionMark(warnings: number): string | null {
  if (warnings === 0) return null;
  return warnings === 1 ? '1 warning' : `${warnings} warnings`;
}

/** What each kind of warning says, in plain words. */
export const ATTENTION_KIND: Record<AttentionKind, string> = {
  broken_link: 'A link that leads nowhere',
  stale: 'Due for a check',
  unreviewed: 'Written by an AI tool and not reviewed',
  duplicate: 'The same file appears more than once',
  missing_description: 'No description',
  unprocessed: 'Not converted, or not fully',
  extraction_warnings: 'Converted with warnings',
  newer_source: 'A View whose source has newer data',
  instruction_like: 'Text that reads like instructions to an AI tool',
  invalidated: 'Refers to content that was removed',
  unparseable_header: 'A header that could not be read',
};

/** A warning's kind in words; a kind this build does not know is said to be unknown, by its name. */
export function attentionKind(kind: string): string {
  return (
    wordsFor(ATTENTION_KIND, kind) ?? `A warning of a kind this page does not know (“${kind}”)`
  );
}

/** What the checks are, said wherever warnings are shown. */
export const ADVISORY =
  'These warnings are advisory. They come from automatic checks, and false alarms are expected.';

/** What an empty list is not, said beside it. */
export const NOT_A_CERTIFICATE =
  'That does not make the workspace safe to give to an AI tool, or correct: the checks look for a few known signs and can miss what matters.';

/** The operations a warning is known to suggest, as a next step in words. */
const NEXT_STEP: Partial<Record<OperationName, string>> = {
  get_item: 'open the item',
  read_item: 'read the item',
  redigest_item: 'convert the file again',
  get_view: 'open the View',
  get_links: 'look at the item’s links',
  get_sources: 'look at the item’s sources',
  set_lifecycle: 'change the item’s status',
};

/** The suggested next step in words; an operation with none here is named as the wire names it. */
export function nextStep(action: OperationName): string {
  return NEXT_STEP[action] ?? `the operation “${action}”`;
}
