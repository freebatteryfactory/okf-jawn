/**
 * "New workspace" on the workspace list (ruling C9): the way to the form that creates one, which
 * is on the installation's Administration (`NewWorkspace.tsx`), at that section's fragment
 * (`/admin#new-workspace`), where the name field takes focus.
 *
 * Offered by the rule of the way to that page (`InstallationAdminLink.tsx`): to a person who holds
 * `admin` on a workspace listed here, or who is the installation's local owner, who may have none
 * yet. The contract names no tenant grant a client can read, so nobody else is offered it; the
 * service decides on the form either way. Nothing is said while the session is not known.
 *
 * Why the form's own section, and not an address of its own: creating a workspace is one of the
 * installation's actions (ruling C1), held on that page with the others, and the fragment names
 * the section, so a reload or a shared link lands on it too.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import type { Workspace } from '../../../api/generated/types.gen';
import { sessionOptions } from '../../imports/limits';
import { NEW_WORKSPACE_HASH } from './NewWorkspace';

export function NewWorkspaceLink({ workspaces }: { workspaces: readonly Workspace[] }) {
  const session = useQuery(sessionOptions());
  const administers = workspaces.some((workspace) => workspace.permissions.includes('admin'));
  const localOwner = session.data?.principal.route === 'local_owner';
  if (!administers && !localOwner) return null;
  return (
    <p>
      <Link to="/admin" hash={NEW_WORKSPACE_HASH}>
        New workspace
      </Link>
    </p>
  );
}
