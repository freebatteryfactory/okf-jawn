/**
 * Snapshot: commit the person's saved drafts of the selected items in one commit (`commit_items`),
 * and what the workspace shows afterwards.
 *
 * The service commits the drafts it holds, so the text of each selected item still being edited
 * is saved first. If some of it cannot be saved, nothing is asked: a Snapshot never commits an
 * older text than the one on screen.
 *
 * On success the shell shows the returned revision as its head at once, because the person made
 * that change: the cached answer of `open_workspace` is given that head (the cache owns it;
 * nothing is copied elsewhere). `open_workspace` is not asked again, so a newer head someone else
 * made after it is still only announced and waits for Refresh. What the commit changed is told to
 * the cache where the cache is not bound to a revision: the committed items have no draft (each
 * cached `get_item` of them, and the drafts list), and the checks that compare the shown head or a
 * cited revision with the latest run again. Listings, reads and search are keyed by the revision
 * they were asked at, so they are asked afresh under the new head and nothing of the old one is
 * stale: a commit changes no revision that exists. A session whose text was committed ends; one
 * the person changed again meanwhile goes on as a new draft on the committed revision.
 *
 * On `conflict` with the `draft_conflict` detail the service has committed nothing: it takes a
 * Snapshot of all the selected items or of none, and the detail lists every selected item that
 * changed or was deleted since its draft's base. Each is kept (`sessions.ts`) until the person
 * resolves it: keep mine is `save_draft` with `base_revision` set to the item's
 * `current_revision` (the session's own save, `rebaseSession` in `saving.ts`), then
 * `commit_items` again; take theirs is `discard_draft`. A conflict means
 * a newer head exists, so the shell's own check for one runs again and announces it.
 *
 * Idempotency: a Snapshot is repeated under the key it had while it is the same request: the same
 * items, the same name, and the same drafts (for each item, the digest and the base the service
 * answered when it was last saved, or listed). So one whose answer was lost is completed, not
 * committed twice; and text saved since, or a draft kept on a newer revision, is a new request
 * with a new key, which the service cannot answer with what it said before. Once an answer says
 * the Snapshot was taken, the request is forgotten: the same drafts under the same name again are
 * a new Snapshot.
 */
import type { QueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { openWorkspaceQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { commitItems } from '../../api/generated/sdk.gen';
import type { Revision, Warning, Workspace } from '../../api/generated/types.gen';
import { short } from '../documents/describe';
import { classifyFailure } from '../errors/failure';
import { forgetDrafts, listedDraft } from './drafts-list';
import { endSession, flushSession } from './saving';
import {
  type ConflictItem,
  needsSave,
  type SnapshotAttempt,
  sessionKey,
  useDraftSessions,
} from './sessions';

/** What a Snapshot did. */
export interface Snapshotted {
  /** The revision the commit made: the head the workspace now shows. */
  revision: Revision;
  /** The head the workspace showed before, when it was known. */
  previousHead: Revision | null;
  itemIds: string[];
  /** Non-fatal issues the service recorded. */
  warnings: Warning[];
}

/** Some selected items hold text the service has not confirmed: nothing was asked. */
export class UnsavedSelection extends Error {
  readonly itemIds: string[];

  constructor(itemIds: string[]) {
    super('Some selected items have text that is not saved');
    this.name = 'UnsavedSelection';
    this.itemIds = itemIds;
  }
}

const sameList = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((entry, index) => entry === b[index]);

/**
 * The request for these items, this name and these drafts: the last one again while it is the
 * same in all three.
 */
export function snapshotAttempt(
  last: SnapshotAttempt | null,
  itemIds: string[],
  message: string,
  drafts: string[],
): SnapshotAttempt {
  const same =
    last !== null &&
    last.message === message &&
    sameList(last.itemIds, itemIds) &&
    sameList(last.drafts, drafts);
  return same ? last : { key: crypto.randomUUID(), itemIds, message, drafts };
}

/**
 * The draft of an item the service holds, as far as this page knows: what its last save answered,
 * else what the list of drafts says. Named by its digest and its base; empty when it is not known.
 */
function draftHeld(client: QueryClient, workspaceId: string, itemId: string): string {
  const session = useDraftSessions.getState().sessions[sessionKey(workspaceId, itemId)];
  const draft = session?.confirmed?.draft ?? listedDraft(client, workspaceId, itemId);
  return draft === undefined ? '' : `${draft.content_digest} on ${draft.base_revision}`;
}

/**
 * Say, where the drafts are listed, what a Snapshot did. It says what the answer shows: that the
 * Snapshot was taken, of how many items (the service takes all that were asked or none), and the
 * revision it made. Each item was committed as it was last saved before the Snapshot was asked.
 */
export function saySnapshotted(workspaceId: string, message: string, snapshotted: Snapshotted) {
  const items = snapshotted.itemIds.length;
  useDraftSessions.getState().say(
    workspaceId,
    `Snapshot “${message}” taken: ${items} ${items === 1 ? 'item' : 'items'}. ` +
      `The workspace now shows revision ${short(snapshotted.revision)}, which the Snapshot made.`,
    snapshotted.warnings.map((warning) => warning.message),
  );
}

/** The items of a `draft_conflict` answer; null for any other failure. */
export function draftConflictsOf(thrown: unknown): ConflictItem[] | null {
  const failure = classifyFailure(thrown);
  if (failure.kind !== 'api' || failure.error.detail?.kind !== 'draft_conflict') return null;
  return failure.error.detail.items;
}

const zHeadCheck = z.object({
  _id: z.literal('openWorkspace'),
  body: z.object({ workspace_id: z.string() }),
});

/** A query that compares the shown head, or a cited revision, with the latest head. */
function checksHead(queryKey: readonly unknown[], workspaceId: string): boolean {
  const key = queryKey.at(0);
  const parsed = zHeadCheck.safeParse(key);
  if (!parsed.success || parsed.data.body.workspace_id !== workspaceId) return false;
  return key !== null && typeof key === 'object' && ('_check' in key || '_comparedWith' in key);
}

/** Run the head checks of a workspace again. */
function checkHeadsAgain(client: QueryClient, workspaceId: string) {
  void client.invalidateQueries({ predicate: (query) => checksHead(query.queryKey, workspaceId) });
}

/**
 * Take a Snapshot. Resolves with what it did. Rejects with `UnsavedSelection` before anything is
 * asked, or with what `commit_items` threw; a `draft_conflict` among it is kept for each item.
 */
export async function takeSnapshot(
  client: QueryClient,
  workspaceId: string,
  itemIds: string[],
  message: string,
): Promise<Snapshotted> {
  await Promise.all(itemIds.map((itemId) => flushSession(client, workspaceId, itemId)));
  const held = useDraftSessions.getState().sessions;
  const unsaved = itemIds.filter((itemId) => {
    const session = held[sessionKey(workspaceId, itemId)];
    return session !== undefined && needsSave(session);
  });
  if (unsaved.length > 0) throw new UnsavedSelection(unsaved);
  // The request is named once the saves have answered: what it commits is known only then.
  const attempt = snapshotAttempt(
    useDraftSessions.getState().asked[workspaceId] ?? null,
    itemIds,
    message,
    itemIds.map((itemId) => draftHeld(client, workspaceId, itemId)),
  );
  useDraftSessions.getState().ask(workspaceId, attempt);

  let result: { revision: Revision; warnings: Warning[] };
  try {
    const { data } = await commitItems({
      body: {
        workspace_id: workspaceId,
        item_ids: attempt.itemIds,
        message: attempt.message,
        idempotency_key: attempt.key,
      },
      throwOnError: true,
    });
    result = data;
  } catch (error) {
    const conflicts = draftConflictsOf(error);
    if (conflicts !== null) {
      useDraftSessions.getState().setConflicts(workspaceId, attempt.message, conflicts);
      checkHeadsAgain(client, workspaceId);
    }
    throw error;
  }
  // Taken: the same drafts under the same name again would be a new Snapshot.
  useDraftSessions.getState().ask(workspaceId, null);

  const headKey = openWorkspaceQueryKey({ body: { workspace_id: workspaceId } });
  const previousHead = client.getQueryData<Workspace>(headKey)?.head ?? null;
  client.setQueryData<Workspace>(headKey, (workspace) =>
    workspace === undefined ? workspace : { ...workspace, head: result.revision },
  );
  for (const itemId of attempt.itemIds) {
    const key = sessionKey(workspaceId, itemId);
    const session = useDraftSessions.getState().sessions[key];
    useDraftSessions.getState().clearConflict(key);
    if (session === undefined) continue;
    if (needsSave(session)) {
      // Changed again while the Snapshot was taken: a new draft, on what was just committed.
      useDraftSessions.getState().change(key, {
        committed: session.confirmed ?? session.committed,
        confirmed: null,
        baseRevision: result.revision,
        attempt: null,
        failure: null,
      });
    } else endSession(workspaceId, itemId);
  }
  forgetDrafts(client, workspaceId, attempt.itemIds);
  checkHeadsAgain(client, workspaceId);
  return {
    revision: result.revision,
    previousHead,
    itemIds: attempt.itemIds,
    warnings: result.warnings,
  };
}
