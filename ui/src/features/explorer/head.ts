/**
 * The head the shell shows, after the person's own change: the revision the operation returned
 * becomes that head at once, without a Refresh.
 *
 * The cache owns the head: the cached answer of `open_workspace` is given the returned revision
 * and nothing is copied elsewhere. `open_workspace` is not asked again for it, so a head someone
 * else made right after is still only announced by the shell's own check and waits for Refresh.
 * Listings, reads, search and the naming rules are keyed by the revision they were asked at, so
 * under the new head they are asked afresh and nothing of the old one is stale: a commit changes
 * no revision that exists.
 *
 * The checks that compare the shown head, or a cited revision, with the latest are not bound to a
 * revision; they are run again after a change, and after a refusal that says the head moved.
 */
import type { QueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { openWorkspaceQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { openWorkspace } from '../../api/generated/sdk.gen';
import type { Revision, Workspace } from '../../api/generated/types.gen';

/** What the person's own change did to the workspace. */
export interface HeadChange {
  /** The revision the change made: the head the workspace now shows. */
  revision: Revision;
  /** The head the workspace showed before, when it was known. */
  previousHead: Revision | null;
  /** The items the change touched: a tab of one that cited the old head now cites the new one. */
  itemIds: string[];
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
export function checkHeadsAgain(client: QueryClient, workspaceId: string) {
  void client.invalidateQueries({ predicate: (query) => checksHead(query.queryKey, workspaceId) });
}

/** Show `revision` as the workspace's head. Answers the head shown before, when it was known. */
export function adoptHead(
  client: QueryClient,
  workspaceId: string,
  revision: Revision,
): Revision | null {
  const headKey = openWorkspaceQueryKey({ body: { workspace_id: workspaceId } });
  const previous = client.getQueryData<Workspace>(headKey)?.head ?? null;
  client.setQueryData<Workspace>(headKey, (workspace) =>
    workspace === undefined ? workspace : { ...workspace, head: revision },
  );
  return previous;
}

/**
 * Whether the service's head is another revision than `shown`, asked now and for this question
 * only (the shown head is not touched). Null when the service could not say.
 */
export async function headMoved(workspaceId: string, shown: Revision): Promise<boolean | null> {
  try {
    const { data } = await openWorkspace({
      body: { workspace_id: workspaceId },
      throwOnError: true,
    });
    return data.head !== shown;
  } catch {
    return null;
  }
}
