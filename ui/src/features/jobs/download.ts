/**
 * Who may download what a job produced, and where from (ruling B4).
 *
 * The rule is the artifact's own (`ArtifactKind::download_permission` and `download_routes` in
 * `crates/contract/src/workspace.rs`, held here as `DOWNLOAD`): an export of the workspace or of a
 * View needs `read`, and may be downloaded from a signed-in browser, by the local owner or by a
 * service identity, never by an agent connected through MCP (it holds originals); a backup, of the
 * workspace or of the installation, needs `admin`, and only in a person's own session (a browser,
 * or the local owner). The person's permissions are the workspace's, as `open_workspace` answered
 * them; their session's route is the one `get_session` answered, and while it is not read this
 * page, a browser, is taken for a browser session (as for cancelling a job, `job-control.ts`). The
 * service decides again on every download either way.
 *
 * The permission an installation backup needs is on the installation, which the contract names no
 * way to read. A workspace's Jobs page does not list one (it is a job of the installation); the
 * installation's Administration lists it from `list_tenant_jobs`, which the service answers only
 * to an administrator of the installation, and offers it with that `admin`
 * (`ArtifactDownload.tsx`, `TenantArtifactDownload`). The service decides either way.
 *
 * Where from. The download is a plain link, not a request through the transport (the one stated
 * exception to that rule: an archive can be larger than memory), so the browser sends the session
 * cookie itself and saves what it is given. Its address is the artifact's `download_path`, which is
 * application-relative, under the base the generated client reaches the API at (`getUrl` puts the
 * same base before every operation's path), so the link goes to the same origin as every request.
 * A path that is not application-relative, or that would leave that origin, is not followed.
 */
import { client } from '../../api/generated/client.gen';
import type {
  AccessRoute,
  ArtifactKind,
  DownloadArtifact,
  Permission,
} from '../../api/generated/types.gen';

/** What downloading an artifact needs. */
export interface DownloadRule {
  permission: Permission;
  /** The routes it may be downloaded from; every other route is refused whatever its grants. */
  routes: readonly AccessRoute[];
}

const PERSON: readonly AccessRoute[] = ['browser_session', 'local_owner'];

export const DOWNLOAD: Record<ArtifactKind, DownloadRule> = {
  export: { permission: 'read', routes: [...PERSON, 'service'] },
  view_export: { permission: 'read', routes: [...PERSON, 'service'] },
  workspace_backup: { permission: 'admin', routes: PERSON },
  installation_backup: { permission: 'admin', routes: PERSON },
};

/** Whether the link is offered, and why not when it is not. */
export type Download =
  | { kind: 'offered'; href: string }
  | { kind: 'unknown-kind' }
  | { kind: 'outside'; path: string }
  | { kind: 'no-permission'; permission: Permission }
  | { kind: 'not-this-session'; route: AccessRoute; routes: readonly AccessRoute[] };

/**
 * Whether a person holding `permissions`, in a session of `route` (null while it is not read), is
 * offered `artifact`, and the link's address under `base`, the API's base (the generated client's
 * `baseUrl`; empty: this page's own origin).
 */
export function downloadOf(
  artifact: DownloadArtifact,
  permissions: readonly Permission[],
  route: AccessRoute | null,
  base: string = client.getConfig().baseUrl ?? '',
): Download {
  const rule: DownloadRule | undefined = (DOWNLOAD as Partial<Record<string, DownloadRule>>)[
    artifact.kind
  ];
  if (rule === undefined) return { kind: 'unknown-kind' };
  const path = artifact.download_path;
  const href = `${base}${path}`;
  const api = new URL(base === '' ? '/' : base, window.location.href).origin;
  if (!path.startsWith('/') || new URL(href, window.location.href).origin !== api) {
    return { kind: 'outside', path };
  }
  if (!permissions.includes(rule.permission)) {
    return { kind: 'no-permission', permission: rule.permission };
  }
  if (route !== null && !rule.routes.includes(route)) {
    return { kind: 'not-this-session', route, routes: rule.routes };
  }
  return { kind: 'offered', href };
}
