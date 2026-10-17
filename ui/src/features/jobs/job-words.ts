/**
 * Plain words for a job: what it is, and how it stands. Both tables are keyed by every value the
 * generated types name, so a new kind or state fails type checking until it has words.
 *
 * Which states allow what, as the generated request types say: `cancel_job` cancels "pending or
 * running work" (`queued`, `running`); `retry_job` takes an "existing failed job" (`failed`) that
 * the service says it will accept again (`retryable`, ruling 2: present exactly when the job
 * failed). A failed job that does not say so (one recorded before the field existed) is not
 * offered a retry. Who may do either is `job-control.ts`.
 *
 * A kind or a state this build does not know (a newer service) is said to be unknown, with the
 * service's own word: never a blank, never another state's words. Such a job is offered neither
 * a cancel nor a retry, and is not taken to have ended (`jobs.ts` goes on asking about it).
 */
import type { Job, JobKind, JobState } from '../../api/generated/types.gen';
import { wordsFor } from '../../lib/words';

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

/** How a job stands, as the start of a sentence. */
export function jobState(state: string): string {
  return wordsFor(JOB_STATE, state) ?? `In a state this page does not know (“${state}”)`;
}

/** How a job stands, inside a sentence: the same words without their capital. */
export function jobStateSaid(state: string): string {
  return (
    wordsFor(JOB_STATE, state)?.toLowerCase() ?? `in a state this page does not know (“${state}”)`
  );
}

/** The name a job is called by: what it is, and enough of its identity to tell it from another. */
export function jobName(job: Job): string {
  const kind =
    wordsFor(JOB_KIND, job.kind) ?? `A job of a kind this page does not know (“${job.kind}”)`;
  return `${kind}, job ${job.id.slice(0, 8)}`;
}

export const canCancel = (job: Job) => job.state === 'queued' || job.state === 'running';
export const canRetry = (job: Job) => job.state === 'failed' && job.retryable === true;
