/**
 * Editing sessions: for each item a person is editing, the text and properties in the editor and
 * how their save stands.
 *
 * This is ephemeral UI state and lasts as long as the page. What was typed and is not yet
 * confirmed by the server is kept here, outside the editor, so it survives the editor being
 * unmounted (another tab, another item, a closed tab) and is never lost silently. A draft the
 * server holds is server state and lives in the Query cache (`get_item`'s `draft`, `list_drafts`);
 * a session keeps the content the server last confirmed only to tell whether the editor differs
 * from it. Nothing here is called saved unless `save_draft` answered for exactly that content.
 */
import { create } from 'zustand';
import type { Draft, Revision } from '../../api/generated/types.gen';

/** An item's editable content: its Markdown body and its complete property map. */
export interface DraftText {
  body: string;
  properties: Record<string, unknown>;
}

/** One `save_draft` request: repeated under the same key while it says the same thing. */
export interface SaveAttempt extends DraftText {
  key: string;
  baseRevision: Revision;
}

export interface DraftSession extends DraftText {
  workspaceId: string;
  itemId: string;
  /** The revision the draft started from; every save carries it. */
  baseRevision: Revision;
  /** The committed content the session started from. */
  committed: DraftText;
  /** The draft as the server last confirmed it; null while the server holds none. */
  confirmed: (DraftText & { draft: Draft }) | null;
  /** A save is in flight. */
  saving: boolean;
  /** What the last save threw; null once a save has answered. */
  failure: unknown;
  /** The request in flight or failed. */
  attempt: SaveAttempt | null;
  /** The text is larger than the service accepts, in bytes; nothing is sent while it is. */
  tooLong: number | null;
}

/** How a session's save stands, as the person is told. */
export type SaveState = 'unchanged' | 'unsaved' | 'saving' | 'saved' | 'failed' | 'too-long';

interface SessionsState {
  sessions: Record<string, DraftSession>;
  put: (session: DraftSession) => void;
  change: (key: string, change: Partial<DraftSession>) => void;
  remove: (key: string) => void;
}

export const sessionKey = (workspaceId: string, itemId: string) => `${workspaceId}/${itemId}`;

export const useDraftSessions = create<SessionsState>()((set) => ({
  sessions: {},
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

/** The editor holds content the server has not confirmed as the draft. */
export function needsSave(session: DraftSession): boolean {
  return !sameText(session, session.confirmed ?? session.committed);
}

export function saveStateOf(session: DraftSession): SaveState {
  if (session.saving) return 'saving';
  if (session.tooLong !== null) return 'too-long';
  if (session.failure !== null) return 'failed';
  if (needsSave(session)) return 'unsaved';
  return session.confirmed === null ? 'unchanged' : 'saved';
}

/** A session holds text that would be lost if the page went away now. */
export function atRisk(session: DraftSession): boolean {
  return session.saving || needsSave(session);
}
