/**
 * Ground for the installation's Administration and a workspace's backups and restore (UI M4c,
 * rulings C6 and C9): contract-valid jobs of each kind these pages start, the artifact and report
 * a finished one carries, and a small service that keeps the rule the real one keeps for every
 * job-starting operation: a repeat under an idempotency key it has seen answers the job that key
 * started, and starts nothing. A test scripts the awkward answers, one step each: a refusal, or an
 * answer lost after the job was made.
 *
 * A tenant job (an installation backup) carries no `workspace_id` (`JobKind::is_tenant`); its
 * artifact is downloaded from the installation's own path, `/api/artifacts/<id>`.
 */
import type {
  DownloadArtifact,
  Job,
  RestoreReport,
  Workspace,
} from '../../../src/api/generated/types.gen';
import { fail, type Json, ok, type Reply } from './api';
import { ARCHIVE_SHA, ARTIFACT, artifactOf } from './exports';
import { accepted, jobOf } from './imports';
import { NOW, WS, workspace } from './workspace';

export const BACKUP_JOB = 'bbbbbbbb-7777-4777-8777-777777777777';
export const RESTORE_JOB = 'cccccccc-7777-4777-8777-777777777777';
export const INSTALLATION_JOB = 'dddddddd-7777-4777-8777-777777777777';
/** The workspace a `create_workspace` makes. */
export const CREATED = '12121212-1212-4121-8121-121212121212';

export { ARCHIVE_SHA, ARTIFACT };

/** The workspace's backup archive, at its workspace path. */
export const workspaceBackup: DownloadArtifact = artifactOf('workspace_backup');
/** The installation's backup archive, at its tenant path. */
export const installationBackup: DownloadArtifact = artifactOf('installation_backup');

/** A backup of the workspace, as the service records it. */
export function backupJobOf(state: Job['state'], extra: Partial<Job> = {}): Job {
  return jobOf(state, {
    id: BACKUP_JOB,
    kind: 'backup_workspace',
    created_at: NOW,
    ...(state === 'succeeded' ? { artifact: workspaceBackup, backup: { rows_left_out: 0 } } : {}),
    ...extra,
  });
}

/** A restore into the workspace, as the service records it. */
export function restoreJobOf(state: Job['state'], extra: Partial<Job> = {}): Job {
  return jobOf(state, { id: RESTORE_JOB, kind: 'restore_workspace', created_at: NOW, ...extra });
}

/** A backup of the installation: a tenant job, with no workspace. */
export function installationJobOf(state: Job['state'], extra: Partial<Job> = {}): Job {
  const { workspace_id: _none, ...tenant } = jobOf(state, {
    id: INSTALLATION_JOB,
    kind: 'backup_installation',
    created_at: NOW,
    ...(state === 'succeeded' ? { artifact: installationBackup } : {}),
  });
  return { ...tenant, ...extra };
}

/** What a finished restore reports. */
export const report: RestoreReport = {
  items: 12,
  drafts_restored: 3,
  drafts_unassigned: 1,
  rows_left_out: 2,
};

/** The workspace a `create_workspace` answers for these values. */
export function createdOf(body: Json): Workspace {
  return {
    ...workspace,
    id: CREATED,
    name: String(body.name),
    description: String(body.description),
    head: 'f'.repeat(40),
    permissions: ['read', 'write', 'propose', 'approve', 'review', 'admin'],
  };
}

/** One scripted answer: `made` is what the request made (or found), `usual` its answer. */
export type Step<T> = (made: T, usual: Reply) => Reply | Promise<Reply>;

/**
 * A job-starting operation as the service keeps it: one job per idempotency key, each made by
 * `make` from the request and the count of jobs before it. `script` replaces answers in order.
 */
export function keyedJobs(make: (body: Json, index: number) => Job) {
  const byKey = new Map<string, Job>();
  const script: Array<Step<Job>> = [];
  return {
    byKey,
    script,
    keys: () => [...byKey.keys()],
    answer: (body: Json): Reply | Promise<Reply> => {
      const key = String(body.idempotency_key);
      let made = byKey.get(key);
      if (made === undefined) {
        made = make(body, byKey.size);
        byKey.set(key, made);
      }
      const usual = accepted(made);
      const step = script.shift();
      return step === undefined ? usual : step(made, usual);
    },
  };
}

/** `create_workspace` as the service keeps it: one workspace per idempotency key. */
export function keyedWorkspaces() {
  const byKey = new Map<string, Workspace>();
  const script: Array<Step<Workspace>> = [];
  return {
    byKey,
    script,
    answer: (body: Json): Reply | Promise<Reply> => {
      const key = String(body.idempotency_key);
      let made = byKey.get(key);
      if (made === undefined) {
        made = createdOf(body);
        byKey.set(key, made);
      }
      const usual = ok(made);
      const step = script.shift();
      return step === undefined ? usual : step(made, usual);
    },
  };
}

/** The service's refusals these pages say in words. */
export const forbidden = (): Reply => fail(403, 'forbidden', 'Required capability is not granted');
export const lost = (): Reply => fail(503, 'unavailable', 'the gateway lost the answer');
export const notBlank = (): Reply => ({
  status: 409,
  body: {
    code: 'conflict',
    message: 'a restore fills only a blank workspace, and it holds items',
    field: '/workspace_id',
  },
});

export { WS };
