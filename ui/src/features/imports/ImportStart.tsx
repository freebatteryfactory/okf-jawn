/**
 * Starting the import of the uploaded files, and the jobs started from this page.
 *
 * The person names the folder to import into: a path inside the workspace, sent exactly as typed,
 * empty for the top. The folders the tree has listed are offered as suggestions; the service
 * decides whether a path is acceptable, and its refusal is shown at the field's form. Only files
 * whose upload the service verified are imported; the button says how many. It is always there
 * (`aria-disabled` with nothing to import, and while it works), so it keeps keyboard focus.
 *
 * The answer is a job. It is shown at once below, in the state the service reported, and a polite
 * status says so; it is never worded as an import that has finished. From then on its row follows
 * the job (`features/jobs`).
 *
 * A `conflict` here most often means the workspace has moved on from the version on screen, which
 * the import is based on: the notice says to refresh and start again, and shows the service's own
 * words. It offers no retry of its own, because the same request would be refused again.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useRef, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, type FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import type { FolderListing } from '../explorer/folder-listing';
import { useJobsFeed } from '../jobs/feed';
import { JobRow } from '../jobs/JobRow';
import { JOB_STATE, jobName } from '../jobs/job-words';
import { jobOptions } from '../jobs/jobs';
import { beginImport, type ImportAttempt, importAttempt } from './import-start';
import { type UploadEntry, useUploads } from './upload-list';

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

const STALE_BASE: FailureMessage = {
  title: 'The import could not be started on the version shown.',
  advice:
    'An import is based on the version of the workspace on screen. If the workspace has changed since, press Refresh in the workspace’s bar, then start the import again.',
  action: { kind: 'none' },
};

/** A refusal a repeat of the same request cannot cure: the base, or the uploads, are not as sent. */
function isStale(error: unknown): boolean {
  const failure = classifyFailure(error);
  return failure.kind === 'api' && failure.error.code === 'conflict';
}

/** The folders the tree has listed at this revision, for suggestions; read as the cache holds them. */
function useListedFolders(workspaceId: string, head: Revision): string[] {
  const client = useQueryClient();
  const folders = new Set<string>();
  for (const [, listing] of client.getQueriesData<FolderListing>({
    queryKey: ['explorer-folder', workspaceId, head],
  })) {
    for (const page of listing?.pages ?? []) for (const folder of page.folders) folders.add(folder);
  }
  return [...folders].sort();
}

export interface ImportStartProps {
  workspaceId: string;
  /** The revision the workspace shows: the import's base. */
  head: Revision;
  entries: readonly UploadEntry[];
}

export function ImportStart({ workspaceId, head, entries }: ImportStartProps) {
  const client = useQueryClient();
  const folderId = useId();
  const hintId = useId();
  const suggestionsId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [destination, setDestination] = useState('');
  const [said, setSaid] = useState('');
  const folders = useListedFolders(workspaceId, head);
  const ready = entries.flatMap((entry) =>
    entry.state === 'uploaded' && entry.uploadId !== null ? [entry.uploadId] : [],
  );
  const last = useRef<ImportAttempt | null>(null);
  const start = useMutation({
    mutationFn: (attempt: ImportAttempt) => beginImport(client, workspaceId, attempt),
    onSuccess: (job, attempt) => {
      last.current = null;
      const where =
        attempt.destination === '' ? 'the top of the workspace' : `“${attempt.destination}”`;
      setSaid(
        `Import of ${count(attempt.uploadIds.length, 'file', 'files')} into ${where} was accepted as ${jobName(job)}. It is not finished: its state is “${JOB_STATE[job.state].toLowerCase()}”.`,
      );
    },
  });
  const begin = () => {
    if (start.isPending || ready.length === 0) return;
    const attempt = importAttempt(last.current, ready, destination, head);
    last.current = attempt;
    start.mutate(attempt);
  };
  const stale = start.isError && isStale(start.error);
  const failure = useRetry({
    request: workspaceId,
    failed: start.isError,
    error: start.error,
    ...(stale
      ? {}
      : {
          run: () => (last.current === null ? Promise.resolve() : start.mutateAsync(last.current)),
        }),
    loaded: () => buttonRef.current,
  });

  return (
    <>
      <form
        className="import-form"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          begin();
        }}
      >
        <label htmlFor={folderId}>Folder to import into</label>
        <input
          id={folderId}
          type="text"
          value={destination}
          list={suggestionsId}
          aria-describedby={hintId}
          autoComplete="off"
          onChange={(event) => setDestination(event.target.value)}
        />
        <datalist id={suggestionsId}>
          {folders.map((folder) => (
            <option key={folder} value={folder} />
          ))}
        </datalist>
        <p id={hintId} className="meta">
          A path inside the workspace, such as notes/2025. Leave it empty for the top of the
          workspace. Each file keeps its own path under that folder.
        </p>
        {/* aria-disabled rather than disabled: the button keeps focus while the import starts,
            and when the files it took have left the list. */}
        <button type="submit" ref={buttonRef} aria-disabled={start.isPending || ready.length === 0}>
          {start.isPending
            ? 'Starting the import…'
            : `Import ${count(ready.length, 'uploaded file', 'uploaded files')}`}
        </button>
      </form>
      {/* Present from the start, so what starting did is announced. */}
      <p role="status" className="meta">
        {start.isPending && !failure.retrying ? 'Starting the import…' : said}
      </p>
      {failure.shown && (
        <FailureNotice
          context="Starting the import"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
          words={stale ? STALE_BASE : undefined}
        />
      )}
    </>
  );
}

export interface StartedJobsProps {
  workspaceId: string;
  /** The person may write: only then are Cancel and a retry offered on a job. */
  canWrite: boolean;
}

/** The jobs started from this page, newest first, each as the service now reports it. */
export function StartedJobs({ workspaceId, canWrite }: StartedJobsProps) {
  const heading = useId();
  const started = useUploads((state) => state.started[workspaceId]);
  if (started === undefined || started.length === 0) return null;
  return (
    <section aria-labelledby={heading} className="started-jobs">
      <h3 id={heading} className="panel-heading">
        Imports started here
      </h3>
      <StartedList workspaceId={workspaceId} jobIds={started} canWrite={canWrite} />
      <p>
        <Link to="/w/$workspaceId/jobs" params={{ workspaceId }}>
          All jobs of this workspace
        </Link>
      </p>
    </section>
  );
}

function StartedList({
  workspaceId,
  jobIds,
  canWrite,
}: {
  workspaceId: string;
  jobIds: readonly string[];
  canWrite: boolean;
}) {
  const feed = useJobsFeed(workspaceId);
  return (
    <ul className="job-list">
      {jobIds.map((jobId) => (
        <StartedJob
          key={jobId}
          workspaceId={workspaceId}
          jobId={jobId}
          live={feed.live}
          canWrite={canWrite}
        />
      ))}
    </ul>
  );
}

function StartedJob({
  workspaceId,
  jobId,
  live,
  canWrite,
}: {
  workspaceId: string;
  jobId: string;
  live: boolean;
  canWrite: boolean;
}) {
  const job = useQuery(jobOptions(workspaceId, jobId, live));
  // A failed reading keeps the job as last reported, with its notice beside it.
  const failure = useRetry({
    request: jobId,
    failed: job.isError,
    error: job.error,
    run: () => job.refetch(),
    loaded: () => document.querySelector<HTMLElement>(`li[data-job="${jobId}"]`),
  });
  return (
    <>
      {job.data !== undefined && (
        <JobRow workspaceId={workspaceId} job={job.data} canWrite={canWrite} />
      )}
      {(failure.shown || job.data === undefined) && (
        <li className="job">
          {failure.shown ? (
            <FailureNotice
              context="Reading this job"
              error={failure.error}
              retry={failure}
              workspaceId={workspaceId}
            />
          ) : (
            <p role="status">Reading the job…</p>
          )}
        </li>
      )}
    </>
  );
}
