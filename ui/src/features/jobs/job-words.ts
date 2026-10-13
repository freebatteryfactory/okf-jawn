/**
 * Plain words for a job: what it is, and how it stands. Both tables are keyed by every value the
 * generated types name, so a new kind or state fails type checking until it has words.
 *
 * Which states allow what, as the generated request types say: `cancel_job` cancels "pending or
 * running work" (`queued`, `running`); `retry_job` takes an "existing failed job" (`failed`). The
 * wire does not say whether a failed job can be retried, so a retry is offered for every failed
 * job and a refusal is shown as one.
 */
import type { Job, JobKind, JobState } from '../../api/generated/types.gen';

export const JOB_KIND: Record<JobKind, string> = {
  import: 'Import',
  redigest: 'Conversion of a file again',
  export_workspace: 'Export of the workspace',
  backup_workspace: 'Backup of the workspace',
  restore_workspace: 'Restore of the workspace',
  rebuild_index: 'Rebuild of the search index',
  export_view: 'Export of a View',
  backup_installation: 'Backup of the installation',
  purge_workspace: 'Purge of the workspace',
  purge_item: 'Purge of an item',
};

export const JOB_STATE: Record<JobState, string> = {
  queued: 'Waiting to start',
  running: 'Running',
  succeeded: 'Finished',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

/** The name a job is called by: what it is, and enough of its identity to tell it from another. */
export function jobName(job: Job): string {
  return `${JOB_KIND[job.kind]}, job ${job.id.slice(0, 8)}`;
}

export const canCancel = (job: Job) => job.state === 'queued' || job.state === 'running';
export const canRetry = (job: Job) => job.state === 'failed';
