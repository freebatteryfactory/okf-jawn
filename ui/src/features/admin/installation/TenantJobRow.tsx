/**
 * One job of the installation (a backup of it, a purge of a workspace) as `list_tenant_jobs` last
 * reported it: what it is, how it stands (in words), its progress while it has not ended, when it
 * was accepted, started and ended, and its outcome. Said as a workspace's job row says it
 * (`jobs/JobRow.tsx`), with the same words (`job-words.ts`, `JobTimes`).
 *
 * What differs from a workspace's job row. A job of the installation belongs to no workspace: it
 * has no items to link, and the service's `cancel_job` and `retry_job` take a workspace, so no
 * control is offered here (a contract gap, reported). A finished backup of the installation offers
 * its archive by the artifact's own rule, at the installation's own download path
 * (`TenantArtifactDownload`); a failed job says why, in the words of its error and the service's.
 */
import { useId } from 'react';
import type { Job } from '../../../api/generated/types.gen';
import { keyed } from '../../../lib/keys';
import { classifyFailure, messageFor } from '../../errors/failure';
import { TenantArtifactDownload } from '../../jobs/ArtifactDownload';
import { JobTimes } from '../../jobs/JobRow';
import { jobName, jobState } from '../../jobs/job-words';
import { unfinished } from '../../jobs/jobs';

export function TenantJobRow({ job }: { job: Job }) {
  const titleId = useId();
  const name = jobName(job);
  const percent = Math.min(100, job.progress);
  return (
    <li tabIndex={-1} className="job" data-job={job.id} aria-labelledby={titleId}>
      <p id={titleId} className="job-title">
        {name}
      </p>
      <p>
        {jobState(job.state)}
        {job.attempt > 1 ? `, attempt ${job.attempt}` : ''}.
        {unfinished(job) && (
          <>
            {' '}
            <progress max={100} value={percent} aria-label={`${name}: progress`} /> {percent}%
          </>
        )}
      </p>
      <JobTimes job={job} />
      <TenantOutcome job={job} />
      {keyed(job.warnings, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}
    </li>
  );
}

/** What the job came to, as far as the wire says. */
function TenantOutcome({ job }: { job: Job }) {
  if (job.state === 'cancelled') return <p>It was cancelled.</p>;
  if (job.state === 'failed') {
    const error = job.error ?? null;
    if (error === null) return <p>The service recorded no reason.</p>;
    return (
      <p>
        Why: {messageFor(classifyFailure(error)).title}
        {error.message.length > 0 ? (
          <>
            {' '}
            The service said: <q>{error.message}</q>
          </>
        ) : null}
        {error.request_id ? (
          <>
            {' '}
            Reference: <code>{error.request_id}</code>
          </>
        ) : null}
      </p>
    );
  }
  if (job.state !== 'succeeded') return null;
  const artifact = job.artifact ?? null;
  return artifact === null ? null : <TenantArtifactDownload artifact={artifact} />;
}
