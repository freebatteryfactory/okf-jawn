/**
 * Snapshot: commit the person's saved drafts of the selected items in one commit (`commit_items`),
 * and what the workspace shows afterwards.
 *
 * The service commits the drafts it holds, so the text of each selected item still being edited
 * is saved first. If some of it cannot be saved, nothing is asked: a Snapshot never commits an
 * older text than the one on screen.
 *
 * On success the shell shows the returned revision as its head at once, because the person made
 * that change (`adoptHead` in `explorer/head.ts`, shared with every other change of the tree). A
 * newer head someone else made after it is still only announced and waits for Refresh. What the commit changed is told to
 * the cache where the cache is not bound to a revision: the committed items have no draft (each
 * cached `get_item` of them, and the drafts list), and the checks that compare the shown head or a
 * cited revision with the latest run again. Listings, reads and search are keyed by the revision
 * they were asked at, so they are asked afresh under the new head and nothing of the old one is
 * stale: a commit changes no revision that exists.
 *
 * A session whose text was committed ends. One whose text went on while the Snapshot was taken
 * (typed since, being saved, or saved in between) goes on as a draft on the revision the Snapshot
 * made, whichever of the save and the commit answered first: what was committed becomes the text
 * it started from, its base becomes that revision, and it is saved at once. The service removes a
 * draft at a Snapshot only if it is still the content it committed, so a draft saved in between
 * is still held, on its old base; that save moves it onto the Snapshot's revision while that
 * revision is the head. The status says how many items go on so. Whether that text is in the
 * Snapshot the page often cannot know (a save sent while the Snapshot was taken may have reached
 * the service before or after it read the drafts), so the status says it "may not be in it".
 *
 * Known limit: when someone else commits right after the Snapshot, the revision it made is no
 * longer the head, and the service moves a draft only onto the head. That one save then leaves
 * the draft on its old base (the session takes the base the service answers, and sends no
 * other). The next Snapshot of the item conflicts, and what the Conflict flow shows as changed
 * underneath is the person's own earlier Snapshot: SPEC section 8 blocks a Snapshot when the item
 * "was itself changed ... since that draft's base". "Keep mine" resolves it.
 * A session that holds input of the properties form that is held back goes on too, on that
 * revision: no save carried that input, so the Snapshot does not have it, and the status says so.
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
import { commitItems } from '../../api/generated/sdk.gen';
import type { Draft, Revision, Warning } from '../../api/generated/types.gen';
import { short } from '../documents/describe';
import { classifyFailure } from '../errors/failure';
import { adoptHead, checkHeadsAgain } from '../explorer/head';
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
  /** Those of them whose text went on while the Snapshot was taken: each goes on as a draft. */
  stillDrafts: string[];
  /** Those of them whose properties form holds input back: no save carried it, so it is not in. */
  heldBack: string[];
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

/** The same saved draft: the same content, saved at the same time, on the same base. */
function sameDraft(a: Draft, b: Draft | undefined): boolean {
  return (
    b !== undefined &&
    a.content_digest === b.content_digest &&
    a.saved_at === b.saved_at &&
    a.base_revision === b.base_revision
  );
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
 * revision it made. Of text that went on while it was taken it says only what holds in either
 * order of the save and the commit: it may not be in the Snapshot, and it goes on as a draft.
 */
export function saySnapshotted(workspaceId: string, message: string, snapshotted: Snapshotted) {
  const items = snapshotted.itemIds.length;
  const still = snapshotted.stillDrafts.length;
  const held = snapshotted.heldBack.length;
  useDraftSessions.getState().say(
    workspaceId,
    `Snapshot “${message}” taken: ${items} ${items === 1 ? 'item' : 'items'}. ` +
      `The workspace now shows revision ${short(snapshotted.revision)}, which the Snapshot made.` +
      (still === 0
        ? ''
        : ` What you typed in ${still === 1 && items === 1 ? 'it' : `${still} of them`} while it was taken may not be in it, and goes on as a draft.`) +
      (held === 0
        ? ''
        : ` What you changed in ${held === 1 && items === 1 ? 'its properties' : `the properties of ${held} of them`} is held back, and is not in the Snapshot.`),
    snapshotted.warnings.map((warning) => warning.message),
  );
}

/** The items of a `draft_conflict` answer; null for any other failure. */
export function draftConflictsOf(thrown: unknown): ConflictItem[] | null {
  const failure = classifyFailure(thrown);
  if (failure.kind !== 'api' || failure.error.detail?.kind !== 'draft_conflict') return null;
  return failure.error.detail.items;
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
  // What the service is asked to commit: the draft each session had confirmed by now, or, of an
  // item not being edited, the one that is listed.
  const asked = new Map(
    itemIds.map((itemId) => {
      const session = useDraftSessions.getState().sessions[sessionKey(workspaceId, itemId)];
      return [
        itemId,
        session === undefined
          ? { edited: false as const, listed: listedDraft(client, workspaceId, itemId) }
          : { edited: true as const, confirmed: session.confirmed },
      ];
    }),
  );

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

  const previousHead = adoptHead(client, workspaceId, result.revision);
  const stillDrafts: string[] = [];
  const heldBack: string[] = [];
  /** Of those, the ones the service still holds a draft of: it was saved in between. */
  const stillHeld: string[] = [];
  for (const itemId of attempt.itemIds) {
    const key = sessionKey(workspaceId, itemId);
    const session = useDraftSessions.getState().sessions[key];
    useDraftSessions.getState().clearConflict(key);
    if (session === undefined) continue;
    const before = asked.get(itemId);
    const committed = before?.edited ? before.confirmed : null;
    // A save's answer puts a new record in the session. A session opened while the Snapshot was
    // taken holds what `get_item` carried: the listed draft, unless it was saved again.
    const savedSince = before?.edited
      ? session.confirmed !== committed
      : session.confirmed !== null && !sameDraft(session.confirmed.draft, before?.listed);
    const wentOn = session.saving || savedSince || needsSave(session);
    if (!wentOn && session.heldBack === null) {
      endSession(workspaceId, itemId);
      continue;
    }
    // The session goes on, on what was just committed: its text went on while the Snapshot was
    // taken, or it holds input of the properties form that no save carried, so the Snapshot does
    // not have it. A failure stays: only a save that answers says what the service holds.
    useDraftSessions.getState().change(key, {
      committed:
        committed === null
          ? session.committed
          : { body: committed.body, properties: committed.properties },
      // The draft that was committed is gone; one saved in between is still held.
      confirmed: savedSince ? session.confirmed : null,
      baseRevision: result.revision,
    });
    if (session.heldBack !== null) heldBack.push(itemId);
    // With nothing but held-back input there is nothing to save.
    if (!wentOn) continue;
    stillDrafts.push(itemId);
    if (savedSince) stillHeld.push(itemId);
    void flushSession(client, workspaceId, itemId);
  }
  forgetDrafts(
    client,
    workspaceId,
    attempt.itemIds.filter((itemId) => !stillHeld.includes(itemId)),
  );
  checkHeadsAgain(client, workspaceId);
  return {
    revision: result.revision,
    previousHead,
    itemIds: attempt.itemIds,
    stillDrafts,
    heldBack,
    warnings: result.warnings,
  };
}
