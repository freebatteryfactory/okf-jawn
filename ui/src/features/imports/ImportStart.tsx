/**
 * Starting the import of the uploaded files, and the jobs started from this page.
 *
 * The person names the folder to import into: a path inside the workspace, sent as typed without
 * the spaces before and after it (a space left at the end of the field is not part of a folder's
 * name), empty for the top. The folders the tree has listed are offered as suggestions; the service
 * decides whether a path is acceptable, and its refusal is shown at the field's form. Only files
 * whose upload the service verified are imported; the button says how many. It is always there
 * (`aria-disabled` with nothing to import, and while it works), so it keeps keyboard focus.
 *
 * The answer is a job. It is shown at once below, in the state the service reported, and a polite
 * status says so; it is never worded as an import that has finished. From then on its row follows
 * the job (`features/jobs`).
 *
 * A `conflict` is said to be a stale base only when the answer says so: it names the request's
 * `base_revision` as the field it is about (the wire has no typed detail for it). The notice then
 * says to refresh and start again, and offers no retry: the same request would be refused again.
 * Any other `conflict` (an upload already taken by another job, a key used for another request)
 * keeps the general words, with the service's own beside them.
 *
 * What the import is based on is `importBase` (`import-start.ts`): the revision shown, or the
 * revision this page's own finished import made, when that is newer than what is shown. The page
 * says so when the two differ.
 */
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useRef, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, type FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import type { FolderListing } from '../explorer/folder-listing';
import { useJobsFeed } from '../jobs/feed';
import { JobRow } from '../jobs/JobRow';
import { jobName, jobStateSaid } from '../jobs/job-words';
import { jobHeldOptions, jobOptions } from '../jobs/jobs';
import { beginImport, type ImportAttempt, importAttempt, importBase } from './import-start';
import { type UploadEntry, useUploads } from './upload-list';

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

const STALE_BASE: FailureMessage = {
  title: 'The workspace has changed since the version this import was based on.',
  advice:
    'Press Refresh in the workspace’s bar to see the workspace as it is now, then start the import again.',
  action: { kind: 'none' },
};

/** The field of `start_import` that names the import's base. */
const BASE_FIELD = 'base_revision';

/** The answer says the base is stale: a `conflict` about the request's base revision. */
function isStale(error: unknown): boolean {
  const failure = classifyFailure(error);
  return (
    failure.kind === 'api' &&
    failure.error.code === 'conflict' &&
    failure.error.field === BASE_FIELD
  );
}

const NO_JOBS: readonly string[] = [];

/**
 * The revision the next import is based on: the one shown, or the one made by an import this page
 * started under it and that has finished (`importBase`). Read from the cache, where the rows
 * under "Imports started here" keep those jobs current; nothing is asked here.
 */
function useImportBase(workspaceId: string, shown: Revision): Revision {
  const started = useUploads((state) => state.started[workspaceId]) ?? NO_JOBS;
  const under = useUploads((state) => state.startedUnder);
  const mine = useQueries({
    queries: started
      .filter((jobId) => under[jobId] === shown)
      .map((jobId) => jobHeldOptions(workspaceId, jobId)),
  });
  return importBase(
    shown,
    mine.map((job) => job.data),
  );
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
  const base = useImportBase(workspaceId, head);
  const ready = entries.flatMap((entry) =>
    entry.state === 'uploaded' && entry.uploadId !== null ? [entry.uploadId] : [],
  );
  const last = useRef<ImportAttempt | null>(null);
  const start = useMutation({
    mutationFn: (attempt: ImportAttempt) => beginImport(client, workspaceId, attempt, head),
    onSuccess: (job, attempt) => {
      last.current = null;
      const where =
        attempt.destination === '' ? 'the top of the workspace' : `“${attempt.destination}”`;
      setSaid(
        `Import of ${count(attempt.uploadIds.length, 'file', 'files')} into ${where} was accepted as ${jobName(job)}. It is not finished: its state is “${jobStateSaid(job.state)}”.`,
      );
    },
  });
  const begin = () => {
    if (start.isPending || ready.length === 0) return;
    const attempt = importAttempt(last.current, ready, destination.trim(), base);
    last.current = attempt;
    start.mutate(attempt);
  };
  const stale = start.isError && isStale(start.error);
  const failure = useRetry({
    request: workspaceId,
    failed: start.isError,
    error: start.error,
    run: () => (last.current === null ? Promise.resolve() : start.mutateAsync(last.current)),
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
        {base !== head && (
          <p role="note" className="notice">
            An import started here made revision <code title={base}>{short(base)}</code> of the
            workspace. The next import is based on it. The workspace above still shows the version
            from before: press Refresh to see what was imported.
          </p>
        )}
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
  // A job was answered: a stream that had stopped for a session that ended is asked for again.
  const { wake } = feed;
  const held = useQueries({ queries: jobIds.map((jobId) => jobHeldOptions(workspaceId, jobId)) });
  const answeredAt = Math.max(0, ...held.map((job) => job.dataUpdatedAt));
  const answered = useRef(answeredAt);
  useEffect(() => {
    // Not at the first render: the jobs are then shown from the answers that started them.
    if (answeredAt > answered.current) wake();
    answered.current = answeredAt;
  }, [answeredAt, wake]);
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
