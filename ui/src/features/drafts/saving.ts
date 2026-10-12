/**
 * Autosave: a session's text goes to `save_draft` a moment after the last change, one request at a
 * time per item, carrying the revision the draft started from.
 *
 * The saving runs here, not in the editor, so it goes on when the editor is unmounted. A change
 * made while a save is in flight is saved when that one settles. A failed save keeps the text in
 * the session and is tried again by the next change, by the person (the failure notice's retry) or
 * when the editor is left.
 *
 * Idempotency: a request is repeated under the key it had while it says the same thing (the same
 * body, properties and base), so a save that timed out and is tried again cannot be applied twice;
 * changed content is a new request with a new key. The service treats the same key with another
 * request as a conflict.
 *
 * The base: the first save of a session carries the revision its text was read at, or the base of
 * the draft it continues. After each save the session takes the base the service recorded, and
 * later saves carry that, never the head the shell shows: a save whose base is the current head is
 * the service's explicit rebase, which only "keep mine" may ask for.
 *
 * A body larger than the service accepts is not sent; the session says so until it is shortened.
 */
import type { QueryClient } from '@tanstack/react-query';
import { discardDraft, saveDraft } from '../../api/generated/sdk.gen';
import type { ItemDocument, Revision } from '../../api/generated/types.gen';
import { zSaveDraftRequest } from '../../api/generated/zod.gen';
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

const timers = new Map<string, ReturnType<typeof setTimeout>>();
const inFlight = new Map<string, Promise<void>>();
/** Sessions changed while their save was in flight: saved again when it settles. */
const changedMeanwhile = new Set<string>();

const sessions = () => useDraftSessions.getState();

/**
 * Start editing an item from what `get_item` answered at `revision`: the person's draft when it
 * carries one, otherwise the committed content. A session already there is left as it is.
 */
export function beginSession(workspaceId: string, revision: Revision, document: ItemDocument) {
  const itemId = document.summary.id;
  if (sessions().sessions[sessionKey(workspaceId, itemId)] !== undefined) return;
  const committed: DraftText = { body: document.body, properties: document.properties };
  const draft = document.draft ?? null;
  const text: DraftText =
    draft === null ? committed : { body: draft.body, properties: draft.properties };
  sessions().put({
    workspaceId,
    itemId,
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
  timers.set(
    key,
    setTimeout(() => {
      timers.delete(key);
      void save(client, key);
    }, AUTOSAVE_DELAY_MS),
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
  const session = sessions().sessions[key];
  if (session === undefined) return;
  if (!needsSave(session)) {
    // Back at what the service holds: there is nothing left to save, or to have failed.
    sessions().change(key, { failure: null, attempt: null, tooLong: null });
    return;
  }
  const bytes = new TextEncoder().encode(session.body).length;
  if (bytes > MAX_BODY_BYTES) {
    sessions().change(key, { tooLong: bytes, failure: null, attempt: null });
    return;
  }
  const text: DraftText = { body: session.body, properties: session.properties };
  const last = session.attempt;
  const attempt: SaveAttempt =
    last !== null && last.baseRevision === session.baseRevision && sameText(last, text)
      ? last
      : { key: crypto.randomUUID(), baseRevision: session.baseRevision, ...text };
  sessions().change(key, { saving: true, attempt, tooLong: null });
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
    sessions().change(key, {
      saving: false,
      failure: null,
      attempt: null,
      confirmed,
      baseRevision: data.base_revision,
    });
    rememberDraft(client, session.workspaceId, session.itemId, confirmed);
  } catch (error) {
    sessions().change(key, { saving: false, failure: error });
  }
}

/**
 * Discard the person's draft of an item: `discard_draft` when the service holds one, and the
 * session with its text either way. Rejects with what `discard_draft` threw, leaving the session
 * as it was. `idempotencyKey` is the same for every attempt of one discard.
 */
export async function discardSession(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  idempotencyKey: string,
): Promise<void> {
  const key = sessionKey(workspaceId, itemId);
  cancelTimer(key);
  changedMeanwhile.delete(key);
  // A save in flight may be creating the draft: what the service holds is known once it settles.
  await inFlight.get(key);
  const session = sessions().sessions[key];
  if (session === undefined || session.confirmed !== null) {
    await discardDraft({
      body: { workspace_id: workspaceId, item_id: itemId, idempotency_key: idempotencyKey },
      throwOnError: true,
    });
  }
  cancelTimer(key);
  sessions().remove(key);
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
  changedMeanwhile.clear();
  inFlight.clear();
  useDraftSessions.setState({ sessions: {} });
}
