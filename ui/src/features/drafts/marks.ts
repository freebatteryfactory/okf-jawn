/**
 * The mark an item carries wherever it is named (a tab, a row of the tree): how its draft stands.
 *
 * "draft": the service holds the person's draft of it. "unsaved": this page holds text of it the
 * service has not confirmed yet. "not saved": the last save of that text failed, or it is too long
 * to save. No mark: no draft and nothing typed. The mark belongs to the item, not to a revision,
 * so a tab that shows a cited revision carries it as well.
 */
import { draftsByItem, useDrafts } from './drafts-list';
import { type DraftSession, saveStateOf, sessionKey, useDraftSessions } from './sessions';

export type DraftMark = 'draft' | 'unsaved' | 'not saved';

/** `held`: the service holds a draft of the item (`get_item`'s `draft`, or `list_drafts`). */
export function draftMarkOf(session: DraftSession | undefined, held: boolean): DraftMark | null {
  const state = session === undefined ? null : saveStateOf(session);
  if (state === 'failed' || state === 'too-long') return 'not saved';
  if (state === 'unsaved' || state === 'saving') return 'unsaved';
  if (state === 'saved') return 'draft';
  return held ? 'draft' : null;
}

/** The mark of each item of a workspace, from the drafts listed and this page's sessions. */
export function useDraftMarks(workspaceId: string): (itemId: string) => DraftMark | null {
  const listed = draftsByItem(useDrafts(workspaceId).data?.pages);
  const sessions = useDraftSessions((state) => state.sessions);
  return (itemId) => draftMarkOf(sessions[sessionKey(workspaceId, itemId)], listed.has(itemId));
}
