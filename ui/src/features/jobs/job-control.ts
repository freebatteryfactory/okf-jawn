/**
 * Who may cancel or retry a job (ruling 8): the permission of the operation that started it, and a
 * human session where that operation needs one. `cancel_job` and `retry_job` say so in their
 * descriptions; the operation that starts each kind of job is the generated metadata's
 * `starts_job` (`api/operations.json`; `x-starts-job` and `x-human-session-only` in OpenAPI). This
 * table holds that metadata, and a unit test holds the table to `api/operations.json`, both ways:
 * every kind a starting operation names is here, with that operation's permission and session
 * rule, and nothing else is.
 *
 * The person's permissions are the workspace's (`open_workspace` answers them; the shell holds the
 * answer). The session is a human one when the service says the person signed in through a
 * browser or as the local owner (`get_session`, `principal.route`); while the session is not
 * read, this page, which is a browser, is taken for one, and the service checks again either way.
 * A kind this build does not know is controlled by no one here.
 */
import { useQuery } from '@tanstack/react-query';
import { openWorkspaceOptions } from '../../api/generated/@tanstack/react-query.gen';
import type {
  AccessRoute,
  Job,
  JobKind,
  OperationName,
  Permission,
} from '../../api/generated/types.gen';
import { sessionOptions } from '../imports/limits';

/** What starts a kind of job, and what that operation needs. */
export interface StartedBy {
  operation: OperationName;
  permission: Permission;
  humanSessionOnly: boolean;
}

export const STARTED_BY: Record<JobKind, StartedBy> = {
  import: { operation: 'start_import', permission: 'write', humanSessionOnly: false },
  redigest: { operation: 'redigest_item', permission: 'write', humanSessionOnly: false },
  export_workspace: { operation: 'export_workspace', permission: 'write', humanSessionOnly: false },
  backup_workspace: { operation: 'backup_workspace', permission: 'admin', humanSessionOnly: true },
  restore_workspace: {
    operation: 'restore_workspace',
    permission: 'admin',
    humanSessionOnly: true,
  },
  rebuild_index: { operation: 'rebuild_index', permission: 'admin', humanSessionOnly: false },
  export_view: { operation: 'export_view', permission: 'write', humanSessionOnly: false },
  backup_installation: {
    operation: 'backup_installation',
    permission: 'admin',
    humanSessionOnly: true,
  },
  purge_workspace: { operation: 'purge_workspace', permission: 'admin', humanSessionOnly: true },
  purge_item: { operation: 'purge_item', permission: 'admin', humanSessionOnly: true },
};

/** The routes of a human session. */
const HUMAN_ROUTES: ReadonlySet<AccessRoute> = new Set(['browser_session', 'local_owner']);

/**
 * Whether a person holding `permissions`, in a session of `route` (null while it is not known),
 * may cancel or retry `job`.
 */
export function mayControl(
  job: Job,
  permissions: readonly Permission[],
  route: AccessRoute | null,
): boolean {
  const started: StartedBy | undefined = (STARTED_BY as Partial<Record<string, StartedBy>>)[
    job.kind
  ];
  if (started === undefined) return false;
  if (!permissions.includes(started.permission)) return false;
  return !started.humanSessionOnly || route === null || HUMAN_ROUTES.has(route);
}

/**
 * The person's permissions in the workspace and their session's route, as the page holds them: the
 * workspace as the shell read it, and the session once read. Nothing is asked here.
 */
export function useControlRights(workspaceId: string): {
  permissions: readonly Permission[];
  route: AccessRoute | null;
} {
  const workspace = useQuery({
    ...openWorkspaceOptions({ body: { workspace_id: workspaceId } }),
    enabled: false,
  });
  const session = useQuery({ ...sessionOptions(), enabled: false });
  return {
    permissions: workspace.data?.permissions ?? [],
    route: session.data?.principal.route ?? null,
  };
}
