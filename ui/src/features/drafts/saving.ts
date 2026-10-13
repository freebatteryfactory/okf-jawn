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
 *
 * A save that has been under way for 30 seconds is said to be still sending, and the person may
 * stop it (`stopSaving`): the request is aborted, and since it may have been applied, the session
 * is then as after any save with no answer, but for one thing. After "Stop" nothing of the
 * session is sent until the person acts (`stopped`). The person acting is: typing again (a change
 * of the text, or of a property that goes into the draft: `editSession`), or pressing a control
 * that saves (the failure notice's "Try again", "Snapshot" with the item ticked, "Keep mine": each
 * sends a save, and a save that is sent ends the stop). Not the person acting: the autosave's
 * timer (the change that waited is dropped), leaving the editor (another tab or item, the
 * committed text, a closed tab, the editor moved to another pane), the page being hidden, and the
 * page being about to leave. The last still asks before the page goes: the session is at risk.
 *
 * A refused session. A session is refused while the last answer of `get_item` that a pane of its
 * item read says the item is not a Note (it is a Source or a View): the service takes no draft of
 * such an item. The pane tells the session at each read (`refuseSaving`, from `DocumentView`) and
 * the session carries it (`refused`: what the item is, or null). This is the one place that says
 * what follows; every function here and in `snapshot.ts` that sends or waits asks that field.
 * - Nothing of it is sent. Every `save_draft` goes through `save` (the autosave's timer, leaving
 *   the editor, a hidden page, a Snapshot's save, "Keep mine"), and `save` returns at once, with
 *   no request, for a refused session. A request already under way when the refusal comes is let
 *   to answer. The text and what was entered in the properties stay in the session.
 * - No control that would send for it is drawn: in place of the editor and of the conflict notice
 *   the pane shows what was typed, read-only, with "Discard draft" (`KeptText`).
 * - Nothing waits on it. `rebaseSession` ("Keep mine") rejects with `DraftNotKept('refused')`
 *   instead of saving until the text is saved: a result its caller can tell from "saved".
 *   `flushSession` settles at once, and its outcome is the session's state, as ever.
 * - A discard is still sent: it is the way out, and the refusal ends with the session.
 * - A session that holds nothing of the person's (nothing typed, nothing saved, nothing held
 *   back) is ended, not refused: there is nothing to keep, and nothing to discard.
 * - When a later read says the item is a Note, the refusal is lifted: the editor is drawn again
 *   with the text the session kept, and the save it owes is sent a moment later, as after a
 *   change. A session whose last save failed or was stopped is not sent by itself, here as
 *   anywhere: its notice has the retry.
 * Limits. The refusal is per item, whichever pane read it: a pane that reads the item at another
 * revision, where it is a Note, lifts what another pane set. And with no pane of the item open
 * nothing reads its kind for the session: a save sent then (the page is hidden) is the service's
 * to refuse, and is shown as a failed save.
 */
import type { QueryClient } from '@tanstack/react-query';
import { discardDraft, saveDraft } from '../../api/generated/sdk.gen';
import type { ItemDocument, ItemSummary, Revision } from '../../api/generated/types.gen';
import { zSaveDraftRequest } from '../../api/generated/zod.gen';
import { REQUEST_TIMEOUT_MS, RequestStopped } from '../../app/api-client';
import { classifyFailure } from '../errors/failure';
import { rememberDraft } from './drafts-list';
import {
  atRisk,
  type DraftText,
  type HeldProperties,
  needsSave,
  type SaveAttempt,
  sameText,
  saveStateOf,
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

/**
 * How long a save is under way before the person is told it is still being sent and may stop
 * it: the bounded wait of an ordinary request. Only a save whose wait is longer (a large text:
 * `app/api-client.ts` allows for what is sent, up to 158 seconds) is still in flight then; an
 * ordinary one has timed out at that same moment.
 */
export const STILL_SENDING_AFTER_MS = REQUEST_TIMEOUT_MS;

const timers = new Map<string, ReturnType<typeof setTimeout>>();
/** When the first change not yet sent was made, by session. */
const unsavedSince = new Map<string, number>();
const inFlight = new Map<string, Promise<void>>();
/** Sessions changed while their save was in flight: saved again when it settles. */
const changedMeanwhile = new Set<string>();
/** The key of the discard asked of an item and not answered yet, by session. */
const discardKeys = new Map<string, string>();
/** What stops the save in flight, by session. */
const stoppers = new Map<string, AbortController>();

/** Why a draft could not be kept on a newer revision, when no request failed. */
export class DraftNotKept extends Error {
  /**
   * This page holds no draft of the item (get_item carried none); its text is too long to save;
   * or its session is refused (the item is not a Note), so nothing of it is saved.
   */
  readonly reason: 'no-draft' | 'too-long' | 'refused';

  constructor(reason: 'no-draft' | 'too-long' | 'refused') {
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
    stillSending: false,
    failure: null,
    stopped: false,
    attempt: null,
    tooLong: null,
    heldBack: null,
    refused: null,
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

/**
 * A pane read the item, and this is what the answer says it is. Anything but a Note refuses the
 * item's session (see the header: nothing of it is sent, and nothing waits on it); a Note lifts
 * the refusal, and the save the session owes is sent a moment later (the autosave the refusal
 * held up). An item with no session has nothing to refuse.
 */
export function refuseSaving(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  kind: ItemSummary['kind'],
) {
  const key = sessionKey(workspaceId, itemId);
  const session = sessions().sessions[key];
  const refused = kind === 'note' ? null : kind;
  if (session === undefined || session.refused === refused) return;
  if (refused !== null && saveStateOf(session) === 'unchanged') {
    // Nothing of the person's is in it: there is nothing to keep on screen.
    endSession(workspaceId, itemId);
    return;
  }
  sessions().change(key, { refused });
  // A save that failed is not sent again by itself: its notice, back with the editor, has the retry.
  if (refused === null && session.failure === null) saveSoon(client, key);
}

function cancelTimer(key: string) {
  const timer = timers.get(key);
  if (timer !== undefined) clearTimeout(timer);
  timers.delete(key);
}

/** Save a session a moment from now, and at the latest so long after its first change not yet sent. */
function saveSoon(client: QueryClient, key: string) {
  cancelTimer(key);
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
 * Keep, with the session, what the person entered in the properties form and the draft has not
 * taken; `null` once nothing is held back. Nothing is sent: no save carries it.
 */
export function holdProperties(workspaceId: string, itemId: string, held: HeldProperties | null) {
  sessions().change(sessionKey(workspaceId, itemId), { heldBack: held });
}

/**
 * The editor's content changed: keep it, and save it a moment after the last change. Properties
 * put into the draft are all that was entered in the form: nothing of it is held back any more.
 */
export function editSession(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  change: Partial<DraftText>,
) {
  const key = sessionKey(workspaceId, itemId);
  if (sessions().sessions[key] === undefined) return;
  // Typing again is the person acting: a save they stopped no longer holds anything back.
  sessions().change(key, {
    ...change,
    ...(change.properties === undefined ? {} : { heldBack: null }),
    stopped: false,
  });
  saveSoon(client, key);
}

/**
 * Save a session now if it holds content the service has not confirmed, because the person asked
 * (a retry, a Snapshot). Settles once nothing is in flight for it; it never rejects: the outcome
 * is the session's state.
 */
export function flushSession(client: QueryClient, workspaceId: string, itemId: string) {
  const key = sessionKey(workspaceId, itemId);
  cancelTimer(key);
  return save(client, key);
}

/**
 * Save a session now because its editor is left, or the page is hidden or about to go: nobody
 * asked. So a session whose last save the person stopped is left as it is; any other is saved
 * as `flushSession` saves it.
 */
export function leaveSession(client: QueryClient, workspaceId: string, itemId: string) {
  if (sessions().sessions[sessionKey(workspaceId, itemId)]?.stopped === true) {
    return Promise.resolve();
  }
  return flushSession(client, workspaceId, itemId);
}

/** The page is hidden or about to go: save every session, as when its editor is left. */
export function flushAllSessions(client: QueryClient) {
  for (const session of Object.values(sessions().sessions)) {
    void leaveSession(client, session.workspaceId, session.itemId);
  }
}

/** Some session holds text that would be lost if the page went away now. */
export function anySessionAtRisk(): boolean {
  return Object.values(sessions().sessions).some(atRisk);
}

function save(client: QueryClient, key: string): Promise<void> {
  // A refused session sends nothing, whoever asks; what it holds stays as it is.
  if (sessions().sessions[key]?.refused != null) return Promise.resolve();
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
  // A save that is sent ends a stop: whoever sent it, the person asked for it.
  sessions().change(key, {
    saving: true,
    stillSending: false,
    stopped: false,
    attempt,
    tooLong: null,
  });
  // A discard asked before this save and not answered would now remove another draft.
  discardKeys.delete(key);
  // The person may stop this save once it has been under way for a while (`stopSaving`).
  const stopper = new AbortController();
  stoppers.set(key, stopper);
  const slow = setTimeout(() => {
    if (stoppers.get(key) === stopper) sessions().change(key, { stillSending: true });
  }, STILL_SENDING_AFTER_MS);
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
      signal: stopper.signal,
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
    // A save the person stopped is said as that, whatever the aborted request threw. Like any
    // save with no answer it may have been applied: the session is not saved, and owes a save.
    sessions().change(key, {
      saving: false,
      failure: stopper.signal.aborted ? new RequestStopped() : error,
    });
  } finally {
    clearTimeout(slow);
    stoppers.delete(key);
  }
}

/**
 * Stop the save of an item that is in flight, at the person's word. The request is aborted; what
 * the service did with it is not known, so the session is "not saved" and a save is owed. It is
 * sent when the person acts (the header says what that is): by the next change, by the notice's
 * retry, by a Snapshot. Nothing is sent without them: the change that waited behind the stopped
 * save is dropped, and leaving the editor or the page being hidden sends nothing (`leaveSession`).
 */
export function stopSaving(workspaceId: string, itemId: string) {
  const key = sessionKey(workspaceId, itemId);
  const stopper = stoppers.get(key);
  if (stopper === undefined) return;
  cancelTimer(key);
  changedMeanwhile.delete(key);
  sessions().change(key, { stopped: true });
  stopper.abort();
}

/**
 * Throw why the session's draft is not kept, if it is not: its session is refused, so nothing of
 * it is saved; or its text is not saved, and then what its save threw, or that it is too long. A
 * session that is saved and not refused, or that is not there, passes.
 */
export function assertSaved(workspaceId: string, itemId: string) {
  const session = sessions().sessions[sessionKey(workspaceId, itemId)];
  if (session === undefined) return;
  if (session.refused !== null) throw new DraftNotKept('refused');
  if (!needsSave(session)) return;
  throw session.failure ?? new DraftNotKept('too-long');
}

/**
 * The first half of "keep mine": the draft is saved again with `revision` as its base, which the
 * service takes as the explicit rebase when that is the revision the item is at. The session's own
 * save carries it, after any save in flight and with the text the editor holds by then; text
 * typed while that save is in flight is saved after it, so this settles with the text on screen
 * saved. A draft already on that base, with that text, is not sent again. Rejects with what a
 * save threw, or with `DraftNotKept`.
 *
 * A refused session is not kept: this rejects with `DraftNotKept('refused')` at once, before it
 * waits for anything or names the base. A session refused while this waits ends the wait the same
 * way (`save` sends nothing for it, so asking again would never end); the base named by then
 * stays named, and the save sent once the refusal is lifted carries it.
 *
 * Why each wait ends. A save in flight answers, times out (the client's bounded wait) or is
 * stopped. Each turn of the loop either sends a request, which ends likewise, or leaves: the
 * session is gone or refused, nothing is left to save, a save failed, or the text is too long.
 */
export async function rebaseSession(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  revision: Revision,
): Promise<void> {
  const key = sessionKey(workspaceId, itemId);
  const session = sessions().sessions[key];
  if (session === undefined) throw new DraftNotKept('no-draft');
  if (session.refused !== null) throw new DraftNotKept('refused');
  cancelTimer(key);
  // A save in flight answers with the base it was sent on: the new base is named after it.
  await inFlight.get(key);
  sessions().change(key, { baseRevision: revision });
  // What is typed while a save is in flight is saved too, without waiting for the autosave's
  // moment: the draft is kept once the text on screen is saved, a save failed, or it is too long.
  for (;;) {
    cancelTimer(key);
    await save(client, key);
    const now = sessions().sessions[key];
    if (now === undefined || now.refused !== null) break;
    if (!needsSave(now) || now.failure !== null || now.tooLong !== null) break;
  }
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
  // A save still in flight is let go, not stopped: its answer finds no session.
  stoppers.clear();
  useDraftSessions.setState({ sessions: {}, conflicts: {}, said: {}, asked: {} });
}
