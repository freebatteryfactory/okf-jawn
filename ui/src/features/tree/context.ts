/**
 * What every control that changes the workspace needs from the shell: the head it shows (the base
 * of each change), whether the person may write, what else they may do and whether the workspace
 * is archived (`explorer/rights.tsx`), how to refresh that head, and where to say that a change was
 * made. The shell provides it once; the tree, a pane, the naming rules and the history and review
 * pages read it.
 */
import { createContext, useContext } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import type { HeadChange } from '../explorer/head';
import type { Rights } from '../explorer/rights';

export interface WorkspaceChanges {
  workspaceId: string;
  /** The revision the workspace shows. */
  head: Revision;
  canWrite: boolean;
  /** The person's permissions here, and whether the workspace is archived. */
  rights: Rights;
  /** Resolve the head again, as the shell's Refresh does: the head then shown, or null when it failed. */
  refresh: () => Promise<string | null>;
  /** The person changed the workspace: tabs that cited the old head follow. */
  onChanged: (change: HeadChange) => void;
}

export const ChangesContext = createContext<WorkspaceChanges | null>(null);

export function useWorkspaceChanges(): WorkspaceChanges {
  const changes = useContext(ChangesContext);
  if (changes === null) throw new Error('a change control is rendered outside the Explorer');
  return changes;
}
