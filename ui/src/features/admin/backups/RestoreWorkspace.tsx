/**
 * Restore from a backup (rulings C6 and C3), inside the workspace's Backups: fill this workspace,
 * which must be blank, from a workspace's backup archive.
 *
 * The steps, each the person's own:
 * 1. "Choose a backup archive…" opens the browser's file picker (the picker's input is hidden, as
 *    the import page's is; the button is the keyboard's way in). The archive chosen is said by its
 *    name and size. One larger than this page or the service takes (`boundOver`) is said so, and
 *    nothing of it is read.
 * 2. "Upload the archive" takes an upload slot of this workspace and sends the bytes through the
 *    upload transport (`archive-upload.ts`: `create_upload`, `upload_content`, `complete_upload`,
 *    with the SHA-256 computed as an import computes it). Its progress is shown; a failure is said
 *    with its retry, which goes on with the same slot under the same keys. Once the service has
 *    confirmed it, the archive is said by its name, size and SHA-256.
 * 3. "Restore this workspace from it…" opens the destructive dialog (`DestructiveDialog`, ruling
 *    C3): what will happen, and exactly what is acted on, the archive's name, size and SHA-256 and
 *    this workspace. Confirming asks `restore_workspace` under the dialog's key, kept through a
 *    retry and through the dialog closed and opened again on the same upload (`useDestructive`).
 *    The contract's restore carries no `base_revision`: the guard is the blank workspace, which the
 *    service checks, with the `admin` grant and the human session.
 * 4. The answer is a job (202), told to the cache (`rememberJob`: the Jobs page lists it) and
 *    followed here: its state, then what the restore did (`RestoreReport`) in words, or why it
 *    failed. A finished restore made new versions of the workspace: the shell's check for a newer
 *    version is run again (`checkHeadsAgain`), and the section says to refresh.
 *
 * What the service decides, and the page only says (ruling C2): the workspace must be blank (no
 * item, no draft, no history since its creation), and the archive's workspace must not still exist
 * here or have been purged here; a restore is taken only from an administrator in their own browser
 * session (`HUMAN_SESSION_ONLY`). Its refusals are said in words (`backup-words.ts`). A restore is
 * not run in an archived workspace (not in `ALLOWED_WHILE_ARCHIVED`, and neither are the upload's
 * steps): the controls stay visible, `aria-disabled`, say why, and send nothing.
 *
 * Keyboard focus. The buttons keep focus while their request is out (`aria-disabled`). The upload
 * button leaves once the archive is uploaded; focus that was on it goes to "Restore this workspace
 * from it…", which is where the dialog returns focus when it closes. After a restore is accepted
 * that button stays, `aria-disabled`, saying that the restore below has taken the upload.
 *
 * What is kept where. The chosen File and the keys of its upload are the page's own (component
 * state and a ref); the uploaded archive is the upload mutation's answer, and the restore's job is
 * read from the query cache by the identity the service answered.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { openWorkspaceOptions } from '../../../api/generated/@tanstack/react-query.gen';
import { restoreWorkspace } from '../../../api/generated/sdk.gen';
import type { Job, RestoreReport } from '../../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../../lib/focus';
import { FailureNotice } from '../../errors/FailureNotice';
import { classifyFailure, messageFor } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import { checkHeadsAgain } from '../../explorer/head';
import { allowedOperation, archivedWords, humanSessionWords, Missing } from '../../explorer/rights';
import { describeSize } from '../../imports/files';
import { sessionOptions, uploadLimitOf } from '../../imports/limits';
import { boundOver } from '../../imports/upload-list';
import { sizeWords } from '../../jobs/ArtifactDownload';
import { jobName, jobStateSaid } from '../../jobs/job-words';
import { jobOptions, rememberJob } from '../../jobs/jobs';
import { useWorkspaceChanges } from '../../tree/context';
import { DestructiveDialog, useDestructive } from '../destructive';
import {
  type ArchiveKeys,
  ArchiveUploadFailed,
  freshKeys,
  type UploadedArchive,
  uploadArchive,
} from './archive-upload';
import { archiveUploadWords, restoreWords } from './backup-words';

/** What the dialog is bound to: the uploaded archive. */
type RestoreValues = UploadedArchive;

/** The restore the service accepted: its job, and the upload it took. */
interface Accepted {
  jobId: string;
  uploadId: string;
}

export function RestoreWorkspace({ workspaceId }: { workspaceId: string }) {
  const headingId = useId();
  const archivedId = useId();
  const takenId = useId();
  const client = useQueryClient();
  const { rights } = useWorkspaceChanges();
  const can = allowedOperation(rights, 'restore_workspace', 'admin');
  const stopped = can === 'archived';
  const workspace = useQuery({
    ...openWorkspaceOptions({ body: { workspace_id: workspaceId } }),
    enabled: false,
  });
  const session = useQuery(sessionOptions());

  const inputRef = useRef<HTMLInputElement | null>(null);
  const chooseRef = useRef<HTMLButtonElement | null>(null);
  const restoreRef = useRef<HTMLButtonElement | null>(null);
  const heldUpload: HeldFocus = useRef<HTMLElement | null>(null);
  const [uploadButton, uploadRef] = useWatchedPart<HTMLButtonElement>(heldUpload);

  // The archive chosen, and how many times one was: each choice is another request.
  const [chosen, setChosen] = useState<File | null>(null);
  const [choice, setChoice] = useState(0);
  const keys = useRef<ArchiveKeys>(freshKeys());
  const running = useRef<AbortController | null>(null);
  const [percent, setPercent] = useState(0);
  const upload = useMutation({
    mutationFn: (file: File) => {
      running.current?.abort();
      const controller = new AbortController();
      running.current = controller;
      return uploadArchive({
        workspaceId,
        file,
        keys,
        // A whole percent at a time: the same value does not draw the section again.
        onSent: (sent) => setPercent(file.size === 0 ? 100 : Math.floor((100 * sent) / file.size)),
        signal: controller.signal,
      });
    },
  });
  // Nothing goes on once the section is gone.
  useEffect(() => () => running.current?.abort(), []);

  // What is said is about the archive chosen now.
  const mine = chosen !== null && upload.variables === chosen;
  const uploading = mine && upload.isPending;
  const uploaded = mine && upload.isSuccess ? upload.data : null;
  const uploadFailure = useRetry({
    request: String(choice),
    failed: mine && upload.isError,
    error: mine ? upload.error : null,
    run: () => (mine && chosen !== null ? upload.mutateAsync(chosen) : Promise.resolve()),
    loaded: () => restoreRef.current ?? uploadButton.current ?? chooseRef.current,
  });
  const failedTry =
    uploadFailure.error instanceof ArchiveUploadFailed ? uploadFailure.error.failure : null;
  const over = chosen === null ? null : boundOver(chosen.size, uploadLimitOf(session.data));

  const [accepted, setAccepted] = useState<Accepted | null>(null);
  const restore = useDestructive<RestoreValues, Job>({
    act: async ({ values, key }) => {
      const { data: job } = await restoreWorkspace({
        body: {
          workspace_id: workspaceId,
          upload_id: values.uploadId,
          sha256: values.sha256,
          idempotency_key: key,
        },
        throwOnError: true,
      });
      rememberJob(client, workspaceId, job);
      return job;
    },
    onDone: (job, values) => setAccepted({ jobId: job.id, uploadId: values.uploadId }),
    returnFocus: restoreRef,
  });
  const taken = uploaded !== null && accepted?.uploadId === uploaded.uploadId;
  // The dialog's own words for a refusal of the restore, where it has some.
  const refusalWords = restoreWords(restore.failure.error);
  const restoreSaid = refusalWords === undefined ? {} : { words: refusalWords };

  // The upload button left with focus on it (the archive is uploaded): focus goes to what follows.
  useLayoutEffect(() => {
    if (takeHeld(heldUpload)) (restoreRef.current ?? chooseRef.current)?.focus();
  });

  if (can === 'no-permission') {
    return <Missing action="Restoring this workspace" permission="admin" />;
  }
  const blocked = stopped ? { 'aria-describedby': archivedId } : {};
  const choose = (file: File) => {
    running.current?.abort();
    upload.reset();
    keys.current = freshKeys();
    setPercent(0);
    setChosen(file);
    setChoice((n) => n + 1);
  };
  return (
    <section aria-labelledby={headingId}>
      <h4 id={headingId}>Restore from a backup</h4>
      <p>
        A restore fills this workspace from a workspace’s backup archive, keeping its items’
        identities. Each draft goes back to its editor when that editor is known here; the others
        stay in the archive, unassigned, and are counted.
      </p>
      <p>
        This workspace must be blank: no items, no drafts and no history since it was created. The
        service refuses a restore into a workspace that is not, and the archive of a workspace of
        this installation that still exists or was purged.
      </p>
      <p className="meta">{humanSessionWords('Restoring a workspace')}</p>
      {stopped && rights.archivedAt !== null && (
        <p id={archivedId} className="meta">
          {archivedWords(rights.archivedAt)} A restore cannot run in an archived workspace:
          unarchive it first.
        </p>
      )}
      <p>
        {/* aria-disabled rather than disabled: the button keeps focus, and says why it cannot run. */}
        <button
          type="button"
          ref={chooseRef}
          aria-disabled={stopped || uploading}
          {...blocked}
          onClick={() => {
            if (!stopped && !uploading) inputRef.current?.click();
          }}
        >
          Choose a backup archive…
        </button>
        <input
          ref={inputRef}
          type="file"
          hidden
          accept=".zip,application/zip"
          aria-label="Backup archive to restore"
          onChange={(event) => {
            const file = Array.from(event.target.files ?? []).at(0) ?? null;
            // The same file can be chosen again.
            event.target.value = '';
            if (file !== null) choose(file);
          }}
        />
      </p>
      {/* Present from the start, so the choice is announced. */}
      <p role="status">
        {chosen === null ? '' : `Chosen: ${chosen.name}, ${sizeWords(String(chosen.size))}.`}
      </p>
      {over !== null && (
        <p className="field-error">
          {over.whose === 'service'
            ? `This archive is larger than the service takes: files of up to ${describeSize(over.bytes)}. Nothing of it will be sent.`
            : `This archive is larger than this page uploads: files of up to ${describeSize(over.bytes)}. Nothing of it will be sent.`}
        </p>
      )}
      {chosen !== null && over === null && uploaded === null && (
        <p>
          {/* aria-disabled rather than disabled: the button keeps focus while the archive goes. */}
          <button
            type="button"
            ref={uploadRef}
            aria-disabled={stopped || uploading}
            {...blocked}
            onClick={() => {
              if (!stopped && !uploading) upload.mutate(chosen);
            }}
          >
            {uploading ? 'Uploading the archive…' : 'Upload the archive'}
          </button>
          {uploading && (
            <>
              {' '}
              <progress max={100} value={percent} aria-label="Uploading the archive: progress" />{' '}
              {percent}%
            </>
          )}
        </p>
      )}
      {/* Present from the start, so the upload is announced when it starts. */}
      <p role="status" className="sr-only">
        {uploading && !uploadFailure.retrying ? 'Uploading the archive…' : ''}
      </p>
      {uploadFailure.shown && (
        <FailureNotice
          context="Uploading the archive"
          error={failedTry?.error ?? uploadFailure.error}
          retry={uploadFailure}
          workspaceId={workspaceId}
          words={failedTry === null ? undefined : archiveUploadWords(failedTry)}
        />
      )}
      {uploaded !== null && (
        <>
          <p>
            Uploaded, and the service checked its length and SHA-256:{' '}
            <strong>{uploaded.filename}</strong>, {sizeWords(String(uploaded.size))}. SHA-256:{' '}
            <code>{uploaded.sha256}</code>
          </p>
          <p>
            {/* aria-disabled rather than disabled: the button keeps focus, and says why it cannot run. */}
            <button
              type="button"
              ref={restoreRef}
              aria-disabled={stopped || taken}
              {...(stopped
                ? { 'aria-describedby': archivedId }
                : taken
                  ? { 'aria-describedby': takenId }
                  : {})}
              onClick={() => {
                if (!stopped && !taken) restore.open(uploaded);
              }}
            >
              Restore this workspace from it…
            </button>
          </p>
          {taken ? (
            <p id={takenId} className="meta">
              The restore below has taken this upload. To restore another archive, choose it.
            </p>
          ) : (
            <p className="meta">
              Until a restore takes it, the uploaded archive waits among this workspace’s uploads,
              on its Import page, where it can be discarded.
            </p>
          )}
        </>
      )}
      <DestructiveDialog
        action={restore}
        title="Restore this workspace from this archive?"
        description={
          <p>
            The archive’s items, their history and the drafts it holds are put into this workspace.
            The service does it only while this workspace is blank, and the upload is used up when
            the restore starts.
          </p>
        }
        confirmLabel="Restore the workspace"
        busyLabel="Starting the restore…"
        context="Restoring this workspace"
        workspaceId={workspaceId}
        {...restoreSaid}
      >
        {restore.opened !== null && (
          <>
            <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
              <dt>Archive</dt>
              <dd>{restore.opened.filename}</dd>
              <dt>Size</dt>
              <dd>{sizeWords(String(restore.opened.size))}</dd>
              <dt>SHA-256</dt>
              <dd>
                <code className="break-all">{restore.opened.sha256}</code>
              </dd>
              <dt>Into</dt>
              <dd>this workspace, {workspace.data?.name ?? workspaceId}</dd>
            </dl>
            <p className="meta">
              If the request fails, “Try again” repeats it under the same key, so the restore is not
              started twice.
            </p>
          </>
        )}
      </DestructiveDialog>
      {accepted !== null && <RestoreOutcome workspaceId={workspaceId} jobId={accepted.jobId} />}
    </section>
  );
}

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The restore the service accepted, as it now reports it. */
function RestoreOutcome({ workspaceId, jobId }: { workspaceId: string; jobId: string }) {
  const client = useQueryClient();
  const followed = useQuery(jobOptions(workspaceId, jobId, false));
  const acceptedRef = useRef<HTMLParagraphElement | null>(null);
  const failure = useRetry({
    request: jobId,
    failed: followed.isError,
    error: followed.error,
    run: () => followed.refetch(),
    loaded: () => acceptedRef.current,
  });
  const job = followed.data ?? null;
  // A finished restore made new versions: the shell's check for a newer one runs again, once.
  const finished = job?.state === 'succeeded';
  useEffect(() => {
    if (finished) checkHeadsAgain(client, workspaceId);
  }, [finished, client, workspaceId]);
  return (
    <div className="grid gap-1">
      <p ref={acceptedRef} tabIndex={-1}>
        The restore was accepted as {job === null ? `job ${jobId.slice(0, 8)}` : jobName(job)}.
      </p>
      {/* Present from the start, so each state it moves to is announced. */}
      <p role="status">{job === null ? '' : `${jobName(job)}: ${jobStateSaid(job.state)}.`}</p>
      {job !== null && <RestoreOutcomeWords job={job} />}
      {failure.shown && (
        <FailureNotice
          context="Asking how the restore stands"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
    </div>
  );
}

function RestoreOutcomeWords({ job }: { job: Job }) {
  if (job.state === 'cancelled') return <p>It was cancelled before it restored anything.</p>;
  if (job.state === 'failed') {
    const error = job.error ?? null;
    return (
      <>
        {error === null ? (
          <p>The service recorded no reason.</p>
        ) : (
          <p>
            Why: {messageFor(classifyFailure(error)).title}
            {error.message.length > 0 ? (
              <>
                {' '}
                The service said: <q>{error.message}</q>
              </>
            ) : null}
          </p>
        )}
        {job.retryable === true ? (
          <p>It can be tried again from its row on the Jobs page.</p>
        ) : job.retryable === false ? (
          <p>The service will not take this restore again.</p>
        ) : null}
      </>
    );
  }
  if (job.state !== 'succeeded') return null;
  const report = job.restore ?? null;
  return (
    <>
      {report === null ? (
        <p>The service reported no account of what it restored.</p>
      ) : (
        <ReportWords report={report} />
      )}
      <p>
        The workspace shows the restored content once it is refreshed: press Refresh at the top of
        the page.
      </p>
    </>
  );
}

/** What a restore did, in words. */
function ReportWords({ report }: { report: RestoreReport }) {
  const unassigned = report.drafts_unassigned;
  const leftOut = report.rows_left_out ?? null;
  return (
    <ul aria-label="What the restore did" className="list-disc pl-6">
      <li>It restored {count(report.items, 'item', 'items')}.</li>
      <li>
        {report.drafts_restored === 0
          ? 'No draft was given back to an editor.'
          : report.drafts_restored === 1
            ? '1 draft was given back to its editor.'
            : `${report.drafts_restored} drafts were given back to their editors.`}
      </li>
      <li>
        {unassigned === 0
          ? 'No draft stays in the archive unassigned.'
          : unassigned === 1
            ? '1 draft stays in the archive, unassigned: its editor is not a known identity here.'
            : `${unassigned} drafts stay in the archive, unassigned: their editors are not known identities here.`}
      </li>
      {leftOut !== null && leftOut > 0 && (
        <li>
          {leftOut === 1
            ? '1 record was not restored: the archive’s own backup had left it out.'
            : `${leftOut} records were not restored: the archive’s own backup had left them out.`}
        </li>
      )}
    </ul>
  );
}
