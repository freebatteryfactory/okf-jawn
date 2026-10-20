/**
 * The words of the installation's Administration for the refusals the service answers there,
 * where the general words of `errors/failure.ts` would mislead (ruling C2). The contract names no
 * tenant grant a client can read, so the page offers every action of the installation and the
 * service decides: its `forbidden` is said as what it means here, an installation's administrator
 * rights (and, for a backup, a person's own session), with what the person can do. Every other
 * failure keeps its general words.
 */
import { classifyFailure, type FailureMessage } from '../../errors/failure';

const NONE = { kind: 'none' } as const;

/** The service answered `forbidden`. */
function isForbidden(error: unknown): boolean {
  const failure = classifyFailure(error);
  return failure.kind === 'api' && failure.error.code === 'forbidden';
}

/** A refused `create_workspace`. */
export function createWords(error: unknown): FailureMessage | undefined {
  if (!isForbidden(error)) return undefined;
  return {
    title:
      'Creating a workspace needs administrator rights for this installation, which the service says you do not have.',
    advice:
      'Ask an administrator of this installation to create it for you, or to give you those rights.',
    action: NONE,
  };
}

/** A refused `backup_installation`. */
export function installationBackupWords(error: unknown): FailureMessage | undefined {
  if (!isForbidden(error)) return undefined;
  return {
    title: 'The service did not allow this backup.',
    advice:
      'A backup of the installation is made only for an administrator of this installation, in their own browser session: not for an agent, a connector or a service.',
    action: NONE,
  };
}

/** A refused `list_tenant_jobs`. */
export function tenantJobsWords(error: unknown): FailureMessage | undefined {
  if (!isForbidden(error)) return undefined;
  return {
    title:
      'Listing the installation’s jobs needs administrator rights for this installation, which the service says you do not have.',
    advice: 'Ask an administrator of this installation for those rights.',
    action: NONE,
  };
}
