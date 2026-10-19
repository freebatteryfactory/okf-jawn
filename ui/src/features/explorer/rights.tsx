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
 */
import type { Permission, Timestamp } from '../../api/generated/types.gen';
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
