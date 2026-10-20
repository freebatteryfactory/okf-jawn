/**
 * What the person may do in the workspace, as `open_workspace` answered it: their permissions
 * (`Workspace.permissions`, SPEC section 11: "Workspace list/open responses include the caller's
 * effective permissions so the UI can show the right actions") and whether it is archived.
 *
 * An action whose permission the person lacks is not offered, and the page says why where a person
 * would look for it (`Missing`). An action of an archived workspace stays offered and says why it
 * cannot run (`archivedWords`): the button is `aria-disabled`, described by those words, and
 * sends nothing. The service decides either way and is asked fresh on every action (SPEC section
 * 11: "Approve, Review and Admin are always checked fresh"): its refusal is shown as any failure is
 * (`failure.ts` words `forbidden` and `workspace_archived`).
 *
 * The archive does not stop everything. The service runs some operations in an archived workspace
 * although they need a permission beyond `read` (an export, a View's presentation: the contract's
 * `ALLOWED_WHILE_ARCHIVED`). An action of one of those is asked about by its operation
 * (`allowedOperation`) and runs as in any other workspace. `allowed` alone knows no operation, and
 * an archived workspace stops whatever it is asked about.
 *
 * Two more of the contract's guards are the service's to apply, and the page only says them
 * (ruling C2): some operations are asked only by an administrator in their own browser session
 * (`HUMAN_SESSION_ONLY`: the purges, backups and restore), and connectors are issued only to the
 * local owner (`LOCAL_OWNER_ONLY`). The page offers those actions by the person's permissions,
 * states the rule in words (`humanSessionWords`, `LOCAL_OWNER_WORDS`), and shows the service's
 * refusal when it answers one; it never refuses on its own.
 */
import type { OperationName, Permission, Timestamp } from '../../api/generated/types.gen';
import { formatInstant } from '../../lib/instant';

/** The person's grant in the workspace, and when it was archived (null while it is not). */
export interface Rights {
  permissions: readonly Permission[];
  archivedAt: Timestamp | null;
}

/** Whether an action needing `permission` can run here: yes, not for this person, or not now. */
export type Allowed = 'yes' | 'no-permission' | 'archived';

export function allowed(rights: Rights, permission: Permission): Allowed {
  if (!rights.permissions.includes(permission)) return 'no-permission';
  return rights.archivedAt === null ? 'yes' : 'archived';
}

/**
 * The operations the service runs while their workspace is archived: the contract's
 * `ALLOWED_WHILE_ARCHIVED` (`crates/contract/src/operations.rs`), `allowed_while_archived` in the
 * generated `api/operations.json`. A unit test holds this list to that metadata, both ways. It is
 * an allow-list there too: an operation added later is refused in an archived workspace until
 * someone decides otherwise.
 */
export const ALLOWED_WHILE_ARCHIVED: ReadonlySet<OperationName> = new Set<OperationName>([
  'list_workspaces',
  'open_workspace',
  'archive_workspace',
  'unarchive_workspace',
  'purge_workspace',
  'get_purge',
  'export_workspace',
  'backup_workspace',
  'list_items',
  'get_item',
  'list_drafts',
  'purge_item',
  'list_types',
  'read_item',
  'get_sources',
  'get_object',
  'create_sandbox_capability',
  'search_items',
  'get_links',
  'get_graph',
  'log_items',
  'diff_items',
  'blame_item',
  'list_proposals',
  'get_proposal',
  'list_comments',
  'list_reviews',
  'get_upload',
  'list_uploads',
  'get_job',
  'list_jobs',
  'get_tenant_job',
  'list_tenant_jobs',
  'cancel_job',
  'get_rules',
  'preview_names',
  'get_attention',
  'rebuild_index',
  'get_view',
  'present_view',
  'resolve_view',
  'export_view',
  'get_catalog',
  'get_receipt',
  'list_events',
  'list_tenant_events',
  'get_session',
  'create_connector',
  'list_connectors',
  'get_health',
  'get_readiness',
]);

/**
 * The operations the service takes only from a person in their own browser session, never from an
 * agent, a connector or a service: the contract's `HUMAN_SESSION_ONLY`, `human_session_only` in
 * `api/operations.json`, held to it by a unit test both ways.
 */
export const HUMAN_SESSION_ONLY: ReadonlySet<OperationName> = new Set<OperationName>([
  'purge_workspace',
  'backup_workspace',
  'restore_workspace',
  'backup_installation',
  'purge_item',
]);

/**
 * The operations the service takes only from the installation's local owner: the contract's
 * `LOCAL_OWNER_ONLY`, `local_owner_only` in `api/operations.json`, held to it by a unit test.
 */
export const LOCAL_OWNER_ONLY: ReadonlySet<OperationName> = new Set<OperationName>([
  'create_connector',
  'list_connectors',
  'revoke_connector',
]);

/** The rule of `HUMAN_SESSION_ONLY`, said where such an action is offered. */
export function humanSessionWords(action: string): string {
  return `${action} is done only by an administrator in their own browser session: an agent, a connector or a service cannot ask for it.`;
}

/** The rule of `LOCAL_OWNER_ONLY`, and what a hosted installation does instead (ruling C5). */
export const LOCAL_OWNER_WORDS =
  'Connectors are issued only on a local installation. Hosted agents connect through WorkOS Connect.';

/**
 * Whether `operation`, which needs `permission`, can run here: as `allowed`, except that the
 * archive does not stop an operation the service runs in an archived workspace.
 */
export function allowedOperation(
  rights: Rights,
  operation: OperationName,
  permission: Permission,
): Allowed {
  const can = allowed(rights, permission);
  return can === 'archived' && ALLOWED_WHILE_ARCHIVED.has(operation) ? 'yes' : can;
}

/** What a permission lets a person do, in words. */
const PERMISSION_WORDS: Record<Permission, string> = {
  read: 'permission to read this workspace',
  write: 'permission to change this workspace',
  propose: 'permission to propose changes in this workspace',
  approve: 'permission to approve proposals in this workspace',
  review: 'permission to verify items in this workspace',
  admin: 'administrator rights here',
};

/** Why an action is not offered: the person lacks its permission. */
export function missingWords(action: string, permission: Permission): string {
  return `${action} needs ${PERMISSION_WORDS[permission]}, which you do not have.`;
}

/** Why an action of an archived workspace cannot run. */
export function archivedWords(archivedAt: Timestamp): string {
  const when = formatInstant(archivedAt) ?? archivedAt;
  return `This workspace was archived on ${when}: nothing in it can be changed until it is unarchived. It can still be read.`;
}

/** The sentence that says why an action is not offered, where a person would look for it. */
export function Missing({ action, permission }: { action: string; permission: Permission }) {
  return <p className="meta">{missingWords(action, permission)}</p>;
}
