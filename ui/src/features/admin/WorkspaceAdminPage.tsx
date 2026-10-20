/**
 * A workspace's Administration (ruling C1), a page of its bar offered to a person whose grant here
 * includes `admin`: this workspace's details and lifecycle (edit, archive and unarchive, purge, the
 * search index), its backups and restore, the connectors that may read it, and its security
 * events. At its address a person without `admin` here is told so, and nothing is asked.
 *
 * Each section asks with this workspace's operations and shows each action by the person's
 * permissions (`explorer/rights.tsx`); the service decides, and its refusal is said in words
 * (ruling C2). Like every page of the bar its heading is a level-two heading, under the
 * workspace's name; each section's is a level-three heading.
 */
import { useId } from 'react';
import { Missing } from '../explorer/rights';
import { useWorkspaceChanges } from '../tree/context';
import { WorkspaceBackups } from './backups/WorkspaceBackups';
import { WorkspaceConnectors } from './connectors/WorkspaceConnectors';
import { WorkspaceEvents } from './events/WorkspaceEvents';
import { WorkspaceLifecycle } from './workspace/WorkspaceLifecycle';

export function WorkspaceAdminPage({ workspaceId }: { workspaceId: string }) {
  const headingId = useId();
  const { rights } = useWorkspaceChanges();
  const admin = rights.permissions.includes('admin');
  return (
    <section className="workspace-page" aria-labelledby={headingId}>
      <h2 id={headingId} className="panel-heading">
        Administration
      </h2>
      {admin ? (
        <>
          <WorkspaceLifecycle workspaceId={workspaceId} />
          <WorkspaceBackups workspaceId={workspaceId} />
          <WorkspaceConnectors workspaceId={workspaceId} />
          <WorkspaceEvents workspaceId={workspaceId} />
        </>
      ) : (
        <Missing action="Administering this workspace" permission="admin" />
      )}
    </section>
  );
}
