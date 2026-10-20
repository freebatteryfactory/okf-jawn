/**
 * The way from the workspace list to the installation's Administration (ruling C1), offered only
 * to a person who holds `admin` on something: on a workspace listed here, or as the installation's
 * local owner, the one identity that owns a local installation's workspaces (SPEC section 11), who
 * may have none yet. The contract names no tenant grant a client can read, so a hosted tenant
 * administrator with no workspace administered here is not offered it (a request to the
 * integration owner).
 *
 * Nothing is said while the session is not known or could not be read: the link is offered, or
 * not, and the workspace list says its own failures.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import type { Workspace } from '../../api/generated/types.gen';
import { sessionOptions } from '../imports/limits';

/**
 * Whether the installation's Administration is offered: the person holds `admin` on a workspace
 * listed here, or is the local owner. False while the session is not known. The ways to that page
 * (this link, `installation/NewWorkspaceLink.tsx`) all ask this one rule.
 */
export function useOffersInstallationAdmin(workspaces: readonly Workspace[]): boolean {
  const session = useQuery(sessionOptions());
  const administers = workspaces.some((workspace) => workspace.permissions.includes('admin'));
  return administers || session.data?.principal.route === 'local_owner';
}

export function InstallationAdminLink({ workspaces }: { workspaces: readonly Workspace[] }) {
  if (!useOffersInstallationAdmin(workspaces)) return null;
  return (
    <p>
      <Link to="/admin">Administration of this installation</Link>
    </p>
  );
}
