/**
 * Editing sessions: for each item a person is editing, the text and properties in the editor and
 * how their save stands.
 *
 * This is ephemeral UI state and lasts as long as the page. What was typed and is not yet
 * confirmed by the server is kept here, outside the editor, so it survives the editor being
 * unmounted (another tab, another item, a closed tab) and is never lost silently. A draft the
 * server holds is server state and lives in the Query cache (`get_item`'s `draft`, `list_drafts`);
 * a session keeps the content the server last confirmed only to tell whether the editor differs
 * from it. A session is called saved only when `save_draft` answered for exactly the content it
 * holds and no save has failed since. A save that failed or timed out may still have been
 * applied, so what the service holds is then unknown: the session stays "not saved" and needs a
 * save, whatever its text, until one has answered. What the person entered in the properties form
 * and the draft has not taken (it would not fit the type) is kept here too, so it outlives the
 * form; no save carries it, and while it is there the item is not called saved either.
 *
 * Beside the sessions: the items of a Snapshot that conflicted, each until the person resolves it
 * (the answer of `commit_items`, kept only to show it), what the last Snapshot did, as words, and
 * the last Snapshot asked of each workspace that has not been answered as taken (`snapshot.ts`
 * repeats it under its key while it is the same request).
 */

import type { z } from 'zod';
import { create } from 'zustand';
import type { Draft, ItemSummary, Revision } from '../../api/generated/types.gen';
import type { zDraftConflictItem } from '../../api/generated/zod.gen';

/** An item's editable content: its Markdown body and its complete property map. */
export interface DraftText {
  body: string;
  properties: Record<string, unknown>;
}

/**
 * What the person entered in the properties form that the draft has not taken: the properties
 * would not fit the item's type with it, so none of it is put into the draft or sent. It is kept
 * here, with the session, so it is still there when the form is shown again.
 */
export interface HeldProperties {
  /** The form's data with the person's input in it: every property but the application's header. */
  form: Record<string, unknown>;
  /** The properties the person changed in a field of the form. */
  entered: string[];
  /** The property map's keys as the form found them: where a property entered again goes back. */
  opened: string[];
  /** The properties a choice in the form took out, which the form goes on naming. */
  takenOut: string[];
}

/** One `save_draft` request: repeated under the same key while it says the same thing. */
export interface SaveAttempt extends DraftText {
  key: string;
  baseRevision: Revision;
}

export interface DraftSession extends DraftText {
  workspaceId: string;
  itemId: string;
  /**
   * The one pane whose editor holds this session's text now; null while no pane does. An editor
   * reads the text once, when it opens, so two over one session would each write the other's
   * words away: a second pane of the item shows the committed text until it takes the editor over.
   */
  editor: string | null;
  /**
   * The revision the draft is on: every save carries it. It is the base the service recorded,
   * until "keep mine" names the revision the item is at now; the next save then asks for that.
   */
  baseRevision: Revision;
  /** The committed content the session started from. */
  committed: DraftText;
  /** The draft as the server last confirmed it; null while the server holds none. */
  confirmed: (DraftText & { draft: Draft }) | null;
  /** A save is in flight. */
  saving: boolean;
  /**
   * The save in flight has been under way for so long that the person is told, and may stop it.
   * It says something only while `saving`: each save starts with it false.
   */
  stillSending: boolean;
  /**
   * What the last save threw; null once a save has answered. While it is set, what the service
   * holds is unknown.
   */
  failure: unknown;
  /**
   * The person stopped this session's last save and has not acted since: until they do, nothing
   * of it is sent (`saving.ts` says what the person acting is).
   */
  stopped: boolean;
  /** The request in flight or failed. */
  attempt: SaveAttempt | null;
  /** The text is larger than the service accepts, in bytes; nothing is sent while it is. */
  tooLong: number | null;
  /**
   * Input of the properties form that is held back; null when the draft has all that was entered.
   * No save carries it, so while it is there the item is unsaved, whatever its text.
   */
  heldBack: HeldProperties | null;
  /**
   * The session is refused: what a pane's last read of the item said it is, when that is not a
   * Note; null otherwise. `saving.ts` says what a refused session is and what follows from it.
   */
  refused: NotANote | null;
}

/** What an item is when it is not a Note: no draft of such an item is saved. */
export type NotANote = Exclude<ItemSummary['kind'], 'note'>;

/**
 * How a session's save stands, as the person is told. "held-back": all that can be saved is
 * saved, and input of the properties form is held back.
 */
export type SaveState =
  | 'unchanged'
  | 'unsaved'
  | 'saving'
  | 'saved'
  | 'failed'
  | 'too-long'
  | 'held-back';

/** One item of a `draft_conflict` answer, as the generated schema reads it off the wire. */
export type ConflictItem = z.infer<typeof zDraftConflictItem>;

/** A Snapshot did not include this item: its content changed since the draft's base. */
export interface SnapshotConflict {
  /** What the service answered about the item: the two revisions and what changed between. */
  conflict: ConflictItem;
  /** The name of the Snapshot that did not include it; "keep mine" snapshots it under that name. */
  message: string;
}

/**
 * One `commit_items` request: the items and the name it carries, and the drafts it was made for.
 * The service commits the drafts it holds, so a request made after one of them was saved again is
 * another request, though it carries the same items and name.
 */
export interface SnapshotAttempt {
  key: string;
  itemIds: string[];
  message: string;
  /** For each item, the draft the service held when this was asked: its digest and its base. */
  drafts: string[];
}

/** What the last Snapshot or resolution did, said politely where the drafts are listed. */
export interface Said {
  text: string;
  /** Non-fatal issues the service recorded with it. */
  warnings: string[];
}

interface SessionsState {
  sessions: Record<string, DraftSession>;
  /** By session key: the items of a conflicting Snapshot the person has not resolved yet. */
  conflicts: Record<string, SnapshotConflict>;
  /** By workspace. */
  said: Record<string, Said>;
  /** By workspace: the last Snapshot asked that no answer has said was taken. */
  asked: Record<string, SnapshotAttempt>;
  put: (session: DraftSession) => void;
  change: (key: string, change: Partial<DraftSession>) => void;
  remove: (key: string) => void;
  setConflicts: (workspaceId: string, message: string, items: ConflictItem[]) => void;
  clearConflict: (key: string) => void;
  say: (workspaceId: string, text: string, warnings?: string[]) => void;
  /** Keep the Snapshot asked of a workspace, or forget it (`null`) once it was taken. */
  ask: (workspaceId: string, attempt: SnapshotAttempt | null) => void;
}

export const sessionKey = (workspaceId: string, itemId: string) => `${workspaceId}/${itemId}`;

export const useDraftSessions = create<SessionsState>()((set) => ({
  sessions: {},
  conflicts: {},
  said: {},
  asked: {},
  ask: (workspaceId, attempt) =>
    set((state) => {
      const { [workspaceId]: _last, ...asked } = state.asked;
      return { asked: attempt === null ? asked : { ...asked, [workspaceId]: attempt } };
    }),
  setConflicts: (workspaceId, message, items) =>
    set((state) => ({
      conflicts: {
        ...state.conflicts,
        ...Object.fromEntries(
          items.map((conflict) => [
            sessionKey(workspaceId, conflict.item_id),
            { conflict, message },
          ]),
        ),
      },
    })),
  clearConflict: (key) =>
    set((state) => {
      if (state.conflicts[key] === undefined) return state;
      const { [key]: _resolved, ...conflicts } = state.conflicts;
      return { conflicts };
    }),
  say: (workspaceId, text, warnings = []) =>
    set((state) => ({ said: { ...state.said, [workspaceId]: { text, warnings } } })),
  put: (session) =>
    set((state) => ({
      sessions: { ...state.sessions, [sessionKey(session.workspaceId, session.itemId)]: session },
    })),
  change: (key, change) =>
    set((state) => {
      const session = state.sessions[key];
      return session === undefined
        ? state
        : { sessions: { ...state.sessions, [key]: { ...session, ...change } } };
    }),
  remove: (key) =>
    set((state) => {
      const { [key]: _removed, ...sessions } = state.sessions;
      return { sessions };
    }),
}));

export function useDraftSession(workspaceId: string, itemId: string): DraftSession | undefined {
  return useDraftSessions((state) => state.sessions[sessionKey(workspaceId, itemId)]);
}

export function useConflict(workspaceId: string, itemId: string): SnapshotConflict | undefined {
  return useDraftSessions((state) => state.conflicts[sessionKey(workspaceId, itemId)]);
}

function ordered(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(ordered);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([key, entry]) => [key, ordered(entry)]),
    );
  }
  return value;
}

/** The same body and the same properties, whatever order the property keys come in. */
export function sameText(a: DraftText, b: DraftText): boolean {
  return (
    a.body === b.body &&
    JSON.stringify(ordered(a.properties)) === JSON.stringify(ordered(b.properties))
  );
}

/**
 * The session has something to save: content the service has not confirmed as the draft; a save
 * that failed, after which the service may hold another text than the one last confirmed; or a
 * base the draft the service holds is not on yet ("keep mine" asked for it).
 */
export function needsSave(session: DraftSession): boolean {
  return (
    session.failure !== null ||
    !sameText(session, session.confirmed ?? session.committed) ||
    (session.confirmed !== null && session.confirmed.draft.base_revision !== session.baseRevision)
  );
}

export function saveStateOf(session: DraftSession): SaveState {
  if (session.saving) return 'saving';
  if (session.tooLong !== null) return 'too-long';
  if (session.failure !== null) return 'failed';
  if (needsSave(session)) return 'unsaved';
  if (session.heldBack !== null) return 'held-back';
  return session.confirmed === null ? 'unchanged' : 'saved';
}

/**
 * A session holds something that would be lost if the page went away now: text not confirmed, or
 * input of the properties form that is held back (it is nowhere but in this page).
 */
export function atRisk(session: DraftSession): boolean {
  return session.saving || needsSave(session) || session.heldBack !== null;
}
