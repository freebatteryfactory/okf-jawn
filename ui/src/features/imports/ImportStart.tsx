/**
 * Starting the import of the uploaded files, and the jobs started from this page.
 *
 * The person names the folder to import into: a path inside the workspace, sent as typed without
 * the spaces before and after it (a space left at the end of the field is not part of a folder's
 * name), empty for the top. The folders the tree has listed are offered as suggestions; the service
 * decides whether a path is acceptable, and its refusal is shown at the field's form. Only files
 * whose upload the service verified are imported: those of this page's list, and those the service
 * holds that the person included (`WaitingUploads.tsx`); the button says how many. It is always
 * there (`aria-disabled` with nothing to import, and while it works), so it keeps keyboard focus.
 * More than `MAX_IMPORT_UPLOADS` files are imported in parts, one job each, and the page says so
 * before the button is pressed. The uploads each part took leave what is included as soon as the
 * part is accepted, so after a later part failed, Import names only what no part took: the part
 * that failed again, under its key (`importAttempt`), and the parts after it.
 *
 * The answer is a job for each part. Each is shown at once below, in the state the service
 * reported, and a polite status says so; it is never worded as an import that has finished. From
 * then on its row follows the job (`features/jobs`).
 *
 * A refusal is shown in the words of its code, with the service's own beside them. A refusal of the
 * folder (its `field` is the JSON Pointer `/destination`, ruling 5) is shown at the folder's field
 * instead, which takes keyboard focus. A head that moved since the base does not refuse an import
 * (ruling 26), so nothing here speaks of one.
 *
 * What the import is based on is `importBase` (`import-start.ts`): the revision shown, or the
 * revision this page's own finished import made, when that is newer than what is shown. The page
 * says so when the two differ.
 */
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Job, Revision } from '../../api/generated/types.gen';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure } from '../errors/failure';
import { useRetry } from '../errors/retry';
import type { FolderListing } from '../explorer/folder-listing';
import { useJobsFeed } from '../jobs/feed';
import { JobRow } from '../jobs/JobRow';
import { jobName, jobStateSaid } from '../jobs/job-words';
import { jobHeldOptions, jobOptions } from '../jobs/jobs';
import {
  beginImport,
  type ImportAttempt,
  importAttempt,
  importBase,
  MAX_IMPORT_UPLOADS,
  partsOf,
  uploadsOf,
} from './import-start';
import { type UploadEntry, useUploads } from './upload-list';

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
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
  /** Uploads the service holds that the person included in the import, beside the list's own. */
  included: readonly string[];
  /** A job took these uploads: told for each part of an import, as soon as it is accepted. */
  onStarted?: (uploadIds: readonly string[]) => void;
}

/** What a started import is said to be: the jobs it was accepted as, and how they stand. */
function startedWords(attemptFiles: number, where: string, jobs: readonly Job[]): string {
  const files = count(attemptFiles, 'file', 'files');
  const first = jobs.at(0);
  if (jobs.length === 1 && first !== undefined) {
    return `Import of ${files} into ${where} was accepted as ${jobName(first)}. It is not finished: its state is “${jobStateSaid(first.state)}”.`;
  }
  return `Import of ${files} into ${where} was accepted in ${jobs.length} parts, as ${jobs.map(jobName).join(', ')}. None is finished yet: each part's state is below.`;
}

export function ImportStart({ workspaceId, head, entries, included, onStarted }: ImportStartProps) {
  const client = useQueryClient();
  const folderId = useId();
  const folderRef = useRef<HTMLInputElement | null>(null);
  const folderErrorId = useId();
  const hintId = useId();
  const suggestionsId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [destination, setDestination] = useState('');
  const [said, setSaid] = useState('');
  const folders = useListedFolders(workspaceId, head);
  const base = useImportBase(workspaceId, head);
  const ready = [
    ...entries.flatMap((entry) =>
      entry.state === 'uploaded' && entry.uploadId !== null ? [entry.uploadId] : [],
    ),
    ...included,
  ].filter((uploadId, index, all) => all.indexOf(uploadId) === index);
  const parts = partsOf(ready).length;
  const last = useRef<ImportAttempt | null>(null);
  const start = useMutation({
    // The uploads of each part are taken as soon as its job is: a part that fails after it does
    // not leave them included, to be named again.
    mutationFn: (attempt: ImportAttempt) =>
      beginImport(client, workspaceId, attempt, head, (taken) => onStarted?.(taken)),
    onSuccess: (jobs, attempt) => {
      last.current = null;
      const where =
        attempt.destination === '' ? 'the top of the workspace' : `“${attempt.destination}”`;
      setSaid(startedWords(uploadsOf(attempt).length, where, jobs));
    },
  });
  const begin = () => {
    if (start.isPending || ready.length === 0) return;
    const attempt = importAttempt(last.current, ready, destination.trim(), base);
    last.current = attempt;
    start.mutate(attempt);
  };
  const failure = useRetry({
    request: workspaceId,
    failed: start.isError,
    error: start.error,
    run: () => (last.current === null ? Promise.resolve() : start.mutateAsync(last.current)),
    loaded: () => buttonRef.current,
  });
  // A refusal of the folder: shown at its field, which takes keyboard focus when it arrives.
  const refusal = start.isError ? classifyFailure(start.error) : null;
  const folderRefused =
    refusal?.kind === 'api' && refusal.error.field === '/destination'
      ? refusal.error.message
      : null;
  const focusedFor = useRef<unknown>(null);
  useLayoutEffect(() => {
    if (folderRefused === null || focusedFor.current === start.error) return;
    focusedFor.current = start.error;
    folderRef.current?.focus();
  });
  // The parts already accepted, when a later one failed.
  const startedParts = last.current?.parts.filter((part) => part.job !== null).length ?? 0;

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
          ref={folderRef}
          type="text"
          value={destination}
          list={suggestionsId}
          aria-describedby={folderRefused === null ? hintId : `${folderErrorId} ${hintId}`}
          aria-invalid={folderRefused !== null}
          autoComplete="off"
          onChange={(event) => {
            setDestination(event.target.value);
            // The refusal was of the folder typed before.
            if (folderRefused !== null) start.reset();
          }}
        />
        {folderRefused !== null && (
          <p id={folderErrorId} className="field-error">
            The service refused this folder: {folderRefused}
          </p>
        )}
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
        {parts > 1 && (
          <p role="note" className="notice">
            One import takes at most {MAX_IMPORT_UPLOADS.toLocaleString('en')} files, so these{' '}
            {ready.length.toLocaleString('en')} are imported in {parts} parts, one after another.
            Each part is a job of its own.
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
      {failure.shown && startedParts > 0 && (
        <p role="note" className="notice">
          {count(startedParts, 'part was', 'parts were')} accepted before this failure:{' '}
          {startedParts === 1 ? 'its job is' : 'their jobs are'} below. Trying again starts the part
          that failed, then the rest.
        </p>
      )}
      {failure.shown && folderRefused === null && (
        <FailureNotice
          context="Starting the import"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
    </>
  );
}

export interface StartedJobsProps {
  workspaceId: string;
}

/** The jobs started from this page, newest first, each as the service now reports it. */
export function StartedJobs({ workspaceId }: StartedJobsProps) {
  const heading = useId();
  const started = useUploads((state) => state.started[workspaceId]);
  if (started === undefined || started.length === 0) return null;
  return (
    <section aria-labelledby={heading} className="started-jobs">
      <h3 id={heading} className="panel-heading">
        Imports started here
      </h3>
      <StartedList workspaceId={workspaceId} jobIds={started} />
      <p>
        <Link to="/w/$workspaceId/jobs" params={{ workspaceId }}>
          All jobs of this workspace
        </Link>
      </p>
    </section>
  );
}

function StartedList({ workspaceId, jobIds }: { workspaceId: string; jobIds: readonly string[] }) {
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
        <StartedJob key={jobId} workspaceId={workspaceId} jobId={jobId} live={feed.live} />
      ))}
    </ul>
  );
}

function StartedJob({
  workspaceId,
  jobId,
  live,
}: {
  workspaceId: string;
  jobId: string;
  live: boolean;
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
      {job.data !== undefined && <JobRow workspaceId={workspaceId} job={job.data} />}
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
