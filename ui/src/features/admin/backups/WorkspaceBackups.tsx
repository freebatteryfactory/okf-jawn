/**
 * Backups (ruling C6), a section of the workspace's Administration: "Back up this workspace"
 * (`backup_workspace`) and, below it, a restore from a backup archive (`RestoreWorkspace`).
 *
 * A backup is one self-contained archive of the workspace: its versions, the objects it keeps and
 * its records, drafts included (SPEC section 12). It is a job: the service answers 202 with the
 * job it queued, which is said as one (`acceptedWords`), told to the cache (`rememberJob`: the
 * Jobs page lists it as it lists every job) and followed here, its row as the Jobs page shows it
 * (`JobRow`), asked again on the jobs' clock while it runs (`jobOptions`). Once the service reports
 * it finished, its row offers the archive by the artifact's own rule (`ArtifactDownload`: `admin`
 * here, in a person's own session; the session is read here so its route is known), and the
 * section says what the backup left out (`BackupReport`). Only the backup started on this visit
 * is followed here: `list_jobs` has no filter by kind, and finding the latest earlier backup would
 * mean walking every page of jobs, so earlier backups are left to the Jobs page, which this
 * section links to.
 *
 * Who may. The section is shown to a person whose grant here includes `admin` (the page's rule).
 * The service takes a backup only from an administrator in their own browser session
 * (`HUMAN_SESSION_ONLY`): the section says so (`humanSessionWords`), offers the button, and says
 * the service's `forbidden` in words (`backup-words.ts`). A backup runs in an archived workspace
 * too (`ALLOWED_WHILE_ARCHIVED`), so the button stays offered there.
 *
 * Idempotency as for an export (`useExportStart`): a request is repeated under its key until a
 * job answers it, so a request whose answer was lost does not start a second backup; the next
 * press after an answer is a new backup.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import { backupWorkspace } from '../../../api/generated/sdk.gen';
import type { Job } from '../../../api/generated/types.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { allowedOperation, humanSessionWords, Missing } from '../../explorer/rights';
import { acceptedWords, useExportStart } from '../../exports/ExportStart';
import { sessionOptions } from '../../imports/limits';
import { JobRow } from '../../jobs/JobRow';
import { jobOptions, rememberJob } from '../../jobs/jobs';
import { useWorkspaceChanges } from '../../tree/context';
import { backupWords } from './backup-words';
import { RestoreWorkspace } from './RestoreWorkspace';

export function WorkspaceBackups({ workspaceId }: { workspaceId: string }) {
  const headingId = useId();
  const aboutId = useId();
  const client = useQueryClient();
  const { rights } = useWorkspaceChanges();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  // The session's route decides where a backup may be downloaded (`jobs/download.ts`).
  useQuery(sessionOptions());
  const start = useExportStart<null>({
    request: workspaceId,
    start: async (attempt) => {
      const { data: job } = await backupWorkspace({
        body: { workspace_id: workspaceId, idempotency_key: attempt.key },
        throwOnError: true,
      });
      rememberJob(client, workspaceId, job);
      return job;
    },
    loaded: () => buttonRef.current,
  });
  const can = allowedOperation(rights, 'backup_workspace', 'admin');
  const said =
    start.pending && !start.failure.retrying
      ? 'Starting the backup…'
      : start.started === null
        ? ''
        : `${acceptedWords('The backup', start.started)} Its row below shows how it stands now; once finished, it offers the download.`;
  return (
    <section aria-labelledby={headingId} className="grid gap-2">
      <h3 id={headingId} className="text-lg font-bold">
        Backups
      </h3>
      <p id={aboutId}>
        A backup is one self-contained archive of this workspace: its versions, the files it keeps
        and its records, drafts included. It can be restored without this installation’s stores, and
        whoever holds the file can read all of it.
      </p>
      {can === 'no-permission' ? (
        <Missing action="Backing up this workspace" permission="admin" />
      ) : (
        <>
          <p className="meta">{humanSessionWords('Backing up this workspace')}</p>
          <p>
            {/* aria-disabled rather than disabled: the button keeps focus while the backup starts. */}
            <button
              type="button"
              ref={buttonRef}
              aria-disabled={start.pending}
              aria-describedby={aboutId}
              onClick={() => start.begin(null)}
            >
              {start.pending ? 'Starting the backup…' : 'Back up this workspace'}
            </button>
          </p>
          {/* Present from the start, so what pressing did is announced. */}
          <p role="status" className="meta">
            {said}
          </p>
          {start.failure.shown && (
            <FailureNotice
              context="Backing up this workspace"
              error={start.failure.error}
              retry={start.failure}
              workspaceId={workspaceId}
              words={backupWords(start.failure.error)}
            />
          )}
          {start.started !== null && (
            <StartedBackup workspaceId={workspaceId} accepted={start.started} />
          )}
        </>
      )}
      <p className="meta">
        Earlier backups are listed with this workspace’s other jobs, where a finished one offers its
        download:{' '}
        <Link className="underline" to="/w/$workspaceId/jobs" params={{ workspaceId }}>
          All jobs of this workspace
        </Link>
        .
      </p>
      <RestoreWorkspace workspaceId={workspaceId} />
    </section>
  );
}

/** The backup this visit started, as the service now reports it. */
function StartedBackup({ workspaceId, accepted }: { workspaceId: string; accepted: Job }) {
  const followed = useQuery(jobOptions(workspaceId, accepted.id, false));
  const listRef = useRef<HTMLUListElement | null>(null);
  const failure = useRetry({
    request: accepted.id,
    failed: followed.isError,
    error: followed.error,
    run: () => followed.refetch(),
    loaded: () => listRef.current?.querySelector<HTMLElement>('li') ?? null,
  });
  const job = followed.data ?? accepted;
  const leftOut = job.state === 'succeeded' ? (job.backup?.rows_left_out ?? null) : null;
  return (
    <>
      <ul ref={listRef} className="job-list" aria-label="The backup started here">
        <JobRow workspaceId={workspaceId} job={job} />
      </ul>
      {leftOut !== null && (
        <p>
          {leftOut === 0
            ? 'It left nothing out of the archive.'
            : leftOut === 1
              ? 'It left 1 record out of the archive: it named a managed backup or a written history.'
              : `It left ${leftOut} records out of the archive: they named a managed backup or a written history.`}
        </p>
      )}
      {failure.shown && (
        <FailureNotice
          context="Asking how the backup stands"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
    </>
  );
}
