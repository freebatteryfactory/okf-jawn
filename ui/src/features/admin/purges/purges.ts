/**
 * The purges (SPEC section 8; rulings C3, C4 and C7): asking for one, reading its record, and the
 * words for both.
 *
 * A purge is the tenant's, not the workspace's: `purge_workspace` and `purge_item` authorize at
 * the installation (its `admin`, in a person's own browser session, `HUMAN_SESSION_ONLY`), and
 * `get_purge` asks with the purge's identity alone, so its record is read after what it removed
 * is gone. Both requests are bound to the `base_revision` the dialog was opened at, and sent
 * under the dialog's idempotency key (`admin/destructive.tsx`). A purge answers 202 with its
 * record, never with a finished removal: the record is told to the cache at once
 * (`rememberPurge`), shown from that answer, and asked for again once, since its job may have
 * moved on before the answer was read.
 *
 * The record is asked again every `PURGE_POLL_MS` while it is not known to have finished
 * (`requested`, `running`, or a state this build does not know), as a workspace's jobs are
 * (`jobs/jobs.ts`), and not at all once it has (`completed`, `failed`) or once the service has
 * answered that the person is not signed in: every such request would be refused the same way.
 * A hidden tab asks nothing (TanStack Query holds an interval while the page is hidden).
 *
 * Words. Each state of `PurgeState` and each count of `PurgeReport` has words here, keyed by the
 * generated types, so a new one fails type checking until it has words; a value this build does
 * not know (a newer service) is said to be unknown, with the service's own word. The counts the
 * wire carries as decimal strings (stored objects can number past 2^53) are grouped as numbers
 * through `BigInt`, never through a float.
 */
import { type QueryClient, queryOptions } from '@tanstack/react-query';
import { getPurge, purgeItem, purgeWorkspace } from '../../../api/generated/sdk.gen';
import type { Purge, PurgeReport, PurgeState, Revision } from '../../../api/generated/types.gen';
import { wordsFor } from '../../../lib/words';
import { classifyFailure, type FailureMessage } from '../../errors/failure';
import { JOBS_POLL_MS, sessionEnded } from '../../jobs/jobs';
import type { Attempt } from '../destructive';

/** How often a purge that has not finished is asked about again: as often as a workspace's jobs. */
export const PURGE_POLL_MS = JOBS_POLL_MS;

/** What a workspace purge is bound to: the workspace, and the head the page showed. */
export interface WorkspacePurge {
  workspaceId: string;
  baseRevision: Revision;
}

/** What an item purge is bound to: the item, its workspace, and the head the page showed. */
export interface ItemPurge extends WorkspacePurge {
  itemId: string;
}

/** Ask for the purge of a workspace. */
export async function startWorkspacePurge(attempt: Attempt<WorkspacePurge>): Promise<Purge> {
  const { data } = await purgeWorkspace({
    body: {
      workspace_id: attempt.values.workspaceId,
      base_revision: attempt.values.baseRevision,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  return data;
}

/** Ask for the purge of one item. Not built on main: the service answers `not_implemented`. */
export async function startItemPurge(attempt: Attempt<ItemPurge>): Promise<Purge> {
  const { data } = await purgeItem({
    body: {
      workspace_id: attempt.values.workspaceId,
      item_id: attempt.values.itemId,
      base_revision: attempt.values.baseRevision,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  return data;
}

/** The cache's key for one purge's record. */
export function purgeQueryKey(purgeId: string) {
  return [{ _id: 'getPurge', body: { purge_id: purgeId } }] as const;
}

/** States in which a purge has ended: it is not asked about again. */
const ENDED: ReadonlySet<string> = new Set<PurgeState>(['completed', 'failed']);

/** A purge not known to have finished: one in a state this build does not know is asked about too. */
export const purgeMayChange = (purge: Purge) => !ENDED.has(purge.state);

/** One purge's record, asked again while it may change. */
export function purgeOptions(purgeId: string) {
  return queryOptions({
    queryKey: purgeQueryKey(purgeId),
    queryFn: async ({ signal }) => {
      const { data } = await getPurge({ body: { purge_id: purgeId }, signal, throwOnError: true });
      return data;
    },
    staleTime: PURGE_POLL_MS,
    refetchInterval: (query) => {
      const purge = query.state.data;
      if (purge === undefined || !purgeMayChange(purge) || sessionEnded(query.state.error)) {
        return false;
      }
      return PURGE_POLL_MS;
    },
  });
}

/** Tell the cache what the service just answered about a purge; it is asked for once more. */
export function rememberPurge(client: QueryClient, purge: Purge) {
  const key = purgeQueryKey(purge.id);
  client.setQueryData(key, purge);
  void client.invalidateQueries({ queryKey: key });
}

export const PURGE_STATE: Record<PurgeState, string> = {
  requested: 'Requested: accepted, and nothing removed yet.',
  running: 'Running: removing what it covers.',
  completed:
    'Completed: every copy the application manages is removed or rewritten, and no affected backup can be restored.',
  failed:
    'Failed: it stopped before it finished, and does not run again by itself. Asking for the same purge again resumes it.',
};

/** How a purge stands, in words. */
export function purgeStateWords(state: string): string {
  return (
    wordsFor(PURGE_STATE, state) ??
    `In a state this page does not know (“${state}”): it is asked about again until it says it has finished.`
  );
}

/** A count in words, grouped: a number, or a decimal string as the wire carries the large ones. */
export function grouped(count: number | string): string {
  if (typeof count === 'number') return count.toLocaleString('en');
  return /^\d+$/.test(count) ? BigInt(count).toLocaleString('en') : `“${count}”`;
}

const isOne = (count: number | string) => count === 1 || count === '1';

/** One count of a report: its words for one, and for any other number. */
interface CountWords {
  one: string;
  many: string;
}

/** Every count of `PurgeReport`, in the order the record says them. */
const REPORT_WORDS: { [K in keyof PurgeReport]: CountWords } = {
  objects_removed: { one: 'stored object deleted', many: 'stored objects deleted' },
  objects_kept_shared: {
    one: 'stored object kept, because a surviving item still references it',
    many: 'stored objects kept, because a surviving item still references them',
  },
  revisions_invalidated: {
    one: 'revision that no longer exists or was rewritten',
    many: 'revisions that no longer exist or were rewritten',
  },
  citations_invalidated: {
    one: 'stored citation of what was purged (a review, a receipt, a comment or a View binding) marked invalidated',
    many: 'stored citations of what was purged (reviews, receipts, comments, View bindings) marked invalidated',
  },
  citations_remapped: {
    one: 'stored citation of another item moved to a rewritten revision',
    many: 'stored citations of other items moved to a rewritten revision',
  },
  views_invalidated: {
    one: 'saved View with a binding marked invalidated',
    many: 'saved Views with a binding marked invalidated',
  },
  drafts_removed: { one: 'draft deleted', many: 'drafts deleted' },
  proposals_removed: {
    one: 'proposal deleted, with its candidate references and comments',
    many: 'proposals deleted, with their candidate references and comments',
  },
  exports_removed: {
    one: 'retained export or View export deleted',
    many: 'retained exports and View exports deleted',
  },
  backups_removed: { one: 'managed backup deleted', many: 'managed backups deleted' },
  backups_rewritten: {
    one: 'managed backup rewritten without what was purged',
    many: 'managed backups rewritten without what was purged',
  },
};

const isCount = (field: string): field is keyof PurgeReport => Object.hasOwn(REPORT_WORDS, field);

/** What a completed purge did, one sentence per count, every count of its report. */
export function reportWords(report: PurgeReport): string[] {
  return Object.entries(REPORT_WORDS).flatMap(([field, words]) => {
    if (!isCount(field)) return [];
    const count = report[field];
    return [`${grouped(count)} ${isOne(count) ? words.one : words.many}.`];
  });
}

/**
 * A purge's refusal in this place's words, where the general ones would mislead: `forbidden` here
 * is not about the workspace's own grant (a purge is the installation's administrator's, in their
 * own browser session), so it says who may ask and that nothing was removed. Any other failure is
 * said in the general words (undefined).
 */
export function purgeRefusedWords(error: unknown): FailureMessage | undefined {
  const failure = classifyFailure(error);
  if (failure.kind !== 'api' || failure.error.code !== 'forbidden') return undefined;
  return {
    title: 'The service refused this purge.',
    advice:
      'A purge is asked for only by an administrator of this installation, in their own browser session: an agent, a connector or a service cannot ask for it. Ask an administrator of this installation to purge it. Nothing was removed.',
    action: { kind: 'none' },
  };
}

/** Reading a purge's record refused, in this place's words: it is the installation's record. */
export function recordRefusedWords(error: unknown): FailureMessage | undefined {
  const failure = classifyFailure(error);
  if (failure.kind !== 'api' || failure.error.code !== 'forbidden') return undefined;
  return {
    title: 'You do not have permission to read this purge.',
    advice:
      'A purge’s record is read only by an administrator of this installation. Ask one of them to read it.',
    action: { kind: 'none' },
  };
}
