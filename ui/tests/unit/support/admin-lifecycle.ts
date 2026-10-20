/**
 * A workspace's details and lifecycle, and its purges (UI M4c, rulings C3, C4, C7 and C9): the
 * answers their tests share. A purge's record and its report are held to the generated schema
 * when they are answered, as every answer of the mocked client is (`support/admin.ts` adds the
 * operations to its contract); the 202 of a purge is checked here, as `accepted` checks a job.
 */
import type {
  Job,
  Purge,
  PurgeReport,
  PurgeTarget,
  Workspace,
} from '../../../src/api/generated/types.gen';
import { zJob, zPurge } from '../../../src/api/generated/zod.gen';
import { adminWorkspace } from './admin';
import type { Reply } from './api';
import { WS } from './workspace';

export const PURGE_ID = '66666666-6666-4666-8666-666666666666';
export const PURGE_JOB = '77777777-7777-4777-8777-777777777777';
export const REBUILD_JOB = '88888888-7777-4777-8777-777777777777';
export const ARCHIVED_AT = '2026-01-02T08:00:00.000Z';
export const REQUESTED_AT = '2026-01-03T09:00:00.000Z';
export const COMPLETED_AT = '2026-01-03T09:05:00.000Z';
export const NEWER = 'e'.repeat(40);

/** The Explorer's workspace for an administrator, archived. */
export const archivedAdminWorkspace: Workspace = { ...adminWorkspace, archived_at: ARCHIVED_AT };

/** An administrator whose grant here does not include `write`. */
export const adminWithoutWrite: Workspace = {
  ...adminWorkspace,
  permissions: ['read', 'admin'],
};

/** What a completed purge did: every count a different number, so each is found by its words. */
export const report: PurgeReport = {
  backups_removed: 2,
  backups_rewritten: 3,
  citations_invalidated: 4,
  citations_remapped: 5,
  drafts_removed: 6,
  exports_removed: 7,
  objects_kept_shared: '1234567',
  objects_removed: '9876543210',
  proposals_removed: 1,
  revisions_invalidated: 1200,
  views_invalidated: 11,
};

export const workspaceTarget: PurgeTarget = { kind: 'workspace', workspace_id: WS };

/** A purge record in `state`; a completed one has its report and when it completed. */
export function purgeOf(state: Purge['state'], extra: Partial<Purge> = {}): Purge {
  return {
    id: PURGE_ID,
    job_id: PURGE_JOB,
    requested_at: REQUESTED_AT,
    requested_by: 'user_01',
    state,
    target: workspaceTarget,
    ...(state === 'completed' ? { completed_at: COMPLETED_AT, report } : {}),
    ...extra,
  };
}

/** The answer of `purge_workspace` or `purge_item`: 202 with the purge, checked as a Purge. */
export function acceptedPurge(purge: Purge): Reply {
  return { status: 202, body: zPurge.parse(purge) };
}

/** A rebuild of the search index, as `rebuild_index` answers it. */
export const rebuildJob: Job = {
  id: REBUILD_JOB,
  kind: 'rebuild_index',
  state: 'queued',
  attempt: 1,
  progress: 0,
  item_ids: [],
  warnings: [],
  workspace_id: WS,
};

/** The answer of `rebuild_index`: 202 with its job, checked as a Job. */
export function acceptedRebuild(job: Job = rebuildJob): Reply {
  return { status: 202, body: zJob.parse(job) };
}

/** A `MutationResult` at `revision`. */
export function mutationAt(revision: string) {
  return {
    revision,
    receipt_id: '55555555-5555-4555-8555-555555555555',
    warnings: [],
  };
}
