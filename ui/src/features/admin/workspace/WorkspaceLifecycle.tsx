/**
 * This workspace (UI M4c, rulings C3, C4 and C9): the first section of its Administration page,
 * with its name and description (`WorkspaceDetails`), a rebuild of its search index
 * (`RebuildIndex`), Archive and Unarchive (`ArchiveWorkspace`) and its purge (`PurgeWorkspace`),
 * each a part with its own level-four heading under the section's level-three one.
 *
 * Each part offers its action by the person's permissions as `open_workspace` answered them
 * (`explorer/rights.tsx`); the service decides, asked fresh each time, and its refusal is said in
 * words (ruling C2). In an archived workspace the actions the service runs there (a rebuild,
 * Unarchive, the purge) stay offered and run; the name and description stay visible and say why
 * they cannot be changed.
 *
 * The workspace's name and description are read from the shell's own answer of `open_workspace`,
 * from the cache it fills (the shell is rendered only once that answer is there); nothing is asked
 * here, and the parts show the workspace again whenever the shell reads it again.
 */
import { useQuery } from '@tanstack/react-query';
import { useId } from 'react';
import { openWorkspaceOptions } from '../../../api/generated/@tanstack/react-query.gen';
import { PurgeWorkspace } from '../purges/PurgeWorkspace';
import { ArchiveWorkspace } from './ArchiveWorkspace';
import { RebuildIndex } from './RebuildIndex';
import { WorkspaceDetails } from './WorkspaceDetails';

export function WorkspaceLifecycle({ workspaceId }: { workspaceId: string }) {
  const headingId = useId();
  // The shell's own answer: never stale here, so this asks nothing.
  const workspace = useQuery({
    ...openWorkspaceOptions({ body: { workspace_id: workspaceId } }),
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
  });
  return (
    <section aria-labelledby={headingId}>
      <h3 id={headingId}>This workspace</h3>
      {workspace.data === undefined ? (
        <p role="status">Reading this workspace…</p>
      ) : (
        <>
          <WorkspaceDetails workspace={workspace.data} />
          <RebuildIndex />
          <ArchiveWorkspace />
          <PurgeWorkspace name={workspace.data.name} />
        </>
      )}
    </section>
  );
}
