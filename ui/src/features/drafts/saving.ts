/**
 * Autosave: a session's text goes to `save_draft` a moment after the last change (one second) and
 * at the latest ten seconds after the first change not yet sent, one request at a time per item,
 * carrying the revision the draft started from.
 *
 * The saving runs here, not in the editor, so it goes on when the editor is unmounted. A change
 * made while a save is in flight is saved when that one settles. A failed save keeps the text in
 * the session and is tried again by the next change, by the person (the failure notice's retry) or
 * when the editor is left. A save that failed or timed out may have been applied all the same, so
 * until a save has answered the session is not saved, whatever its text: putting the text back to
 * what was last confirmed still sends a save of that text.
 *
 * Idempotency: a request is repeated under the key it had while it says the same thing (the same
 * body, properties and base), so a save that timed out and is tried again cannot be applied twice;
 * changed content is a new request with a new key. The service treats the same key with another
 * request as a conflict.
 *
 * The base: the first save of a session carries the revision its text was read at, or the base of
 * the draft it continues. After each save the session takes the base the service recorded, and
 * later saves carry that, never the head the shell shows: a save whose base is the current head is
 * the service's explicit rebase, which is asked for in two cases only. "Keep mine" asks for it
 * (`rebaseSession`): it names the base and the session's own save carries it, so there is still
 * one request per item at a time, and its key is the key of exactly that text on that base. And a
 * Snapshot of the item that answered while its text had gone on names the revision it made
 * (`snapshot.ts`): the text was typed over what that revision holds.
 *
 * A discard asks the service unless the page knows it holds no draft, which it does only of text
 * no save of which is in doubt: after a save that failed or timed out it asks, and takes the
 * answer that there is no draft as the discard done. A discard is repeated under one key until it
 * answers, wherever it is pressed from: the key is
 * kept here, by item, not in the component that asked. A save sent after a discard that did not
 * answer makes the next discard another request (it would remove another draft), with a new key.
 *
 * A body larger than the service accepts is not sent; the session says so until it is shortened.
 */
import type { QueryClient } from '@tanstack/react-query';
import { discardDraft, saveDraft } from '../../api/generated/sdk.gen';
import type { ItemDocument, Revision } from '../../api/generated/types.gen';
import { zSaveDraftRequest } from '../../api/generated/zod.gen';
import { classifyFailure } from '../errors/failure';
import { rememberDraft } from './drafts-list';
import {
  atRisk,
  type DraftText,
  needsSave,
  type SaveAttempt,
  sameText,
  sessionKey,
  useDraftSessions,
} from './sessions';

/** How long after the last change a session is saved. */
export const AUTOSAVE_DELAY_MS = 1_000;

/** The service's bound on a drafted body in UTF-8 bytes, as the generated request schema has it. */
export const MAX_BODY_BYTES = zSaveDraftRequest.shape.body.maxLength ?? Number.POSITIVE_INFINITY;

/**
 * The longest a change waits to be sent, however fast the person goes on typing: someone who
 * never pauses for `AUTOSAVE_DELAY_MS` is saved all the same.
 */
export const AUTOSAVE_MAX_WAIT_MS = 10_000;

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** When the first change not yet sent was made, by session. */
const unsavedSince = new Map<string, number>();
const inFlight = new Map<string, Promise<void>>();
/** Sessions changed while their save was in flight: saved again when it settles. */
const changedMeanwhile = new Set<string>();
/** The key of the discard asked of an item and not answered yet, by session. */
const discardKeys = new Map<string, string>();

/** Why a draft could not be kept on a newer revision, when no request failed. */
export class DraftNotKept extends Error {
  /** This page holds no draft of the item (get_item carried none); or its text is too long to save. */
  readonly reason: 'no-draft' | 'too-long';

  constructor(reason: 'no-draft' | 'too-long') {
    super(`The draft was not kept: ${reason}`);
    this.name = 'DraftNotKept';
    this.reason = reason;
  }
}

const sessions = () => useDraftSessions.getState();

/**
 * Start editing an item from what `get_item` answered at `revision`: the person's draft when it
 * carries one, otherwise the committed content. A session already there is left as it is.
 */
export function beginSession(
  workspaceId: string,
  revision: Revision,
  document: ItemDocument,
  pane: string,
) {
  const itemId = document.summary.id;
  if (sessions().sessions[sessionKey(workspaceId, itemId)] !== undefined) return;
  const committed: DraftText = { body: document.body, properties: document.properties };
  const draft = document.draft ?? null;
  const text: DraftText =
    draft === null ? committed : { body: draft.body, properties: draft.properties };
  sessions().put({
    workspaceId,
    itemId,
    editor: pane,
    baseRevision: draft === null ? revision : draft.draft.base_revision,
    ...text,
    committed,
    confirmed: draft === null ? null : { ...text, draft: draft.draft },
    saving: false,
    failure: null,
    attempt: null,
    tooLong: null,
  });
}

/**
 * Make `pane` the one whose editor holds the session's text. A session another pane edits is left
 * to it unless `takeOver`: that is "Edit here", the person moving the editor to this pane.
 */
export function claimEditor(workspaceId: string, itemId: string, pane: string, takeOver = false) {
  const key = sessionKey(workspaceId, itemId);
  const session = sessions().sessions[key];
  if (session === undefined || session.editor === pane) return;
  if (session.editor === null || takeOver) sessions().change(key, { editor: pane });
}

/** `pane` no longer shows the session's editor (it reads, or it is gone): another may take it. */
export function releaseEditor(workspaceId: string, itemId: string, pane: string) {
  const key = sessionKey(workspaceId, itemId);
  if (sessions().sessions[key]?.editor === pane) sessions().change(key, { editor: null });
}

function cancelTimer(key: string) {
  const timer = timers.get(key);
  if (timer !== undefined) clearTimeout(timer);
  timers.delete(key);
}

/** The editor's content changed: keep it, and save it a moment after the last change. */
export function editSession(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  change: Partial<DraftText>,
) {
  const key = sessionKey(workspaceId, itemId);
  if (sessions().sessions[key] === undefined) return;
  sessions().change(key, change);
  cancelTimer(key);
  // A moment after this change, and at the latest so long after the first one not yet sent.
  const now = Date.now();
  const since = unsavedSince.get(key) ?? now;
  unsavedSince.set(key, since);
  timers.set(
    key,
    setTimeout(
      () => {
        timers.delete(key);
        void save(client, key);
      },
      Math.max(0, Math.min(AUTOSAVE_DELAY_MS, since + AUTOSAVE_MAX_WAIT_MS - now)),
    ),
  );
}

/**
 * Save a session now if it holds content the service has not confirmed. Settles once nothing is in
 * flight for it; it never rejects: the outcome is the session's state.
 */
export function flushSession(client: QueryClient, workspaceId: string, itemId: string) {
  const key = sessionKey(workspaceId, itemId);
  cancelTimer(key);
  return save(client, key);
}

/** Save every session that holds unconfirmed content. */
export function flushAllSessions(client: QueryClient) {
  for (const session of Object.values(sessions().sessions)) {
    void flushSession(client, session.workspaceId, session.itemId);
  }
}

/** Some session holds text that would be lost if the page went away now. */
export function anySessionAtRisk(): boolean {
  return Object.values(sessions().sessions).some(atRisk);
}

function save(client: QueryClient, key: string): Promise<void> {
  const running = inFlight.get(key);
  if (running !== undefined) {
    changedMeanwhile.add(key);
    return running;
  }
  const run = send(client, key)
    .finally(() => inFlight.delete(key))
    .then(() => (changedMeanwhile.delete(key) ? save(client, key) : undefined));
  inFlight.set(key, run);
  return run;
}

async function send(client: QueryClient, key: string): Promise<void> {
  // What was typed up to now goes with this save; a later change starts its own wait.
  unsavedSince.delete(key);
  const session = sessions().sessions[key];
  if (session === undefined) return;
  if (!needsSave(session)) {
    // What the service holds is known, and it is this text: there is nothing to save.
    sessions().change(key, { tooLong: null });
    return;
  }
  const bytes = new TextEncoder().encode(session.body).length;
  if (bytes > MAX_BODY_BYTES) {
    // A failure before this stays: only a save that answers says what the service holds.
    sessions().change(key, { tooLong: bytes });
    return;
  }
  const text: DraftText = { body: session.body, properties: session.properties };
  const last = session.attempt;
  const attempt: SaveAttempt =
    last !== null && last.baseRevision === session.baseRevision && sameText(last, text)
      ? last
      : { key: crypto.randomUUID(), baseRevision: session.baseRevision, ...text };
  sessions().change(key, { saving: true, attempt, tooLong: null });
  // A discard asked before this save and not answered would now remove another draft.
  discardKeys.delete(key);
  try {
    const { data } = await saveDraft({
      body: {
        workspace_id: session.workspaceId,
        item_id: session.itemId,
        base_revision: attempt.baseRevision,
        body: attempt.body,
        properties: attempt.properties,
        idempotency_key: attempt.key,
      },
      throwOnError: true,
    });
    const confirmed = { body: attempt.body, properties: attempt.properties, draft: data };
    // A base named while this save was in flight (a Snapshot of the item answered) stays: the
    // draft the service holds is then not on it, and the next save asks for it.
    const named = sessions().sessions[key]?.baseRevision ?? attempt.baseRevision;
    sessions().change(key, {
      saving: false,
      failure: null,
      attempt: null,
      confirmed,
      baseRevision: named === attempt.baseRevision ? data.base_revision : named,
    });
    rememberDraft(client, session.workspaceId, session.itemId, confirmed);
  } catch (error) {
    sessions().change(key, { saving: false, failure: error });
  }
}

/**
 * Throw why the session's text is not saved, if it is not: what its save threw, or that it is too
 * long. A session that is saved, or that is not there, passes.
 */
export function assertSaved(workspaceId: string, itemId: string) {
  const session = sessions().sessions[sessionKey(workspaceId, itemId)];
  if (session === undefined || !needsSave(session)) return;
  throw session.failure ?? new DraftNotKept('too-long');
}

/**
 * The first half of "keep mine": the draft is saved again with `revision` as its base, which the
 * service takes as the explicit rebase when that is the revision the item is at. The session's own
 * save carries it, after any save in flight and with the text the editor holds by then. A draft
 * already on that base, with that text, is not sent again. Rejects with what the save threw, or
 * with `DraftNotKept`.
 */
export async function rebaseSession(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  revision: Revision,
): Promise<void> {
  const key = sessionKey(workspaceId, itemId);
  if (sessions().sessions[key] === undefined) throw new DraftNotKept('no-draft');
  cancelTimer(key);
  // A save in flight answers with the base it was sent on: the new base is named after it.
  await inFlight.get(key);
  sessions().change(key, { baseRevision: revision });
  await save(client, key);
  assertSaved(workspaceId, itemId);
}

/** The service answered that it holds no such thing: of a discard, that there is no draft. */
function answeredNotFound(thrown: unknown): boolean {
  const failure = classifyFailure(thrown);
  return failure.kind === 'api' && failure.error.code === 'not_found';
}

/**
 * Discard the person's draft of an item: the session with its text, and the draft the service
 * holds. The service is asked (`discard_draft`) unless this page knows it holds none, and it knows
 * that of one case: a session no save of which was confirmed and none failed, so nothing of it
 * was sent, or what was sent has been snapshotted. After a save that failed or timed out the
 * service may hold a draft, and so it may when this page has no session of the item: it is asked.
 * Its answer that it holds no draft of the item (`not_found`) is the discard done. Rejects with
 * anything else `discard_draft` threw, leaving the session as it was. Every attempt of one
 * discard carries the same key, whoever asks.
 */
export async function discardSession(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
): Promise<void> {
  const key = sessionKey(workspaceId, itemId);
  cancelTimer(key);
  changedMeanwhile.delete(key);
  // A save in flight may be creating the draft: what the service holds is known once it settles.
  await inFlight.get(key);
  const session = sessions().sessions[key];
  const holdsNone = session !== undefined && session.confirmed === null && session.failure === null;
  if (!holdsNone) {
    const idempotencyKey = discardKeys.get(key) ?? crypto.randomUUID();
    discardKeys.set(key, idempotencyKey);
    try {
      await discardDraft({
        body: { workspace_id: workspaceId, item_id: itemId, idempotency_key: idempotencyKey },
        throwOnError: true,
      });
    } catch (error) {
      if (!answeredNotFound(error)) throw error;
    }
    discardKeys.delete(key);
  }
  cancelTimer(key);
  sessions().remove(key);
  // A conflict of this draft is over with it: there is nothing left to keep.
  sessions().clearConflict(key);
  rememberDraft(client, workspaceId, itemId, null);
}

/** End a session whose draft the service no longer holds (it was snapshotted or discarded). */
export function endSession(workspaceId: string, itemId: string) {
  const key = sessionKey(workspaceId, itemId);
  cancelTimer(key);
  changedMeanwhile.delete(key);
  sessions().remove(key);
}

/** Forget every session and timer: a fresh page, for tests. */
export function resetDraftSessions() {
  for (const key of [...timers.keys()]) cancelTimer(key);
  unsavedSince.clear();
  changedMeanwhile.clear();
  inFlight.clear();
  discardKeys.clear();
  useDraftSessions.setState({ sessions: {}, conflicts: {}, said: {}, asked: {} });
}
