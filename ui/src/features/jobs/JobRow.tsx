/**
 * One job as the service last reported it: what it is, how it stands (in words, never by color),
 * its progress while it has not ended, and its outcome.
 *
 * When the job was accepted, when its current attempt started and when it ended are shown as the
 * wire gives them (ruling 2), each in the reader's time zone; a job recorded before those times
 * existed shows none. The attempt number is shown from the second attempt on. A finished job leads to the items it made: a link for each, pinned to the revision
 * the job reports, with the item's conversion state beside it when it is a source. A job that
 * reports no revision gets no links: a link without a revision would read whatever the workspace
 * shows, which may not hold the items yet.
 *
 * Cancel is offered while the job is waiting or running, and asks first, in place: "Keep it
 * running" has focus. "Try the job again" is offered for a failed job the service says it will
 * take again (`retryable`); a failed job says whether it can be. Both are offered only to a person
 * the operation metadata lets control the job (`job-control.ts`).
 *
 * Idempotency. A press of "Cancel the job" or of "Try the job again" is a new request under a new
 * idempotency key, every time. Only the failure notice's "Try again" repeats a request, the one
 * that failed, under the key it had: its answer may have been lost, and the service answers a
 * repeated key with the job as it stands instead of doing the work twice.
 *
 * A request that failed leaves it unknown what the service holds, so the job is asked again at
 * once (`jobUncertain`) and the row shows what is answered. The failure is shown in plain words
 * with its retry (`useRetry`). A `conflict` is said as what it is here: the job changed or ended
 * before the request reached it. It offers no retry: the same request would be refused again, and
 * the row, read again, shows what can be done now.
 *
 * Keyboard focus. The controls of a job leave when its state changes: by the person's own action,
 * or because the job moved on by itself while a control had focus. Focus that was on a control
 * that left goes to the question's safe answer when the question has just opened, to "Cancel"
 * when the question was dismissed, and otherwise to the job's row, which says how the job stands.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import { cancelJob, retryJob } from '../../api/generated/sdk.gen';
import type { ArtifactKind, Job, Revision } from '../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { wordsFor } from '../../lib/words';
import { itemOptions } from '../documents/DocumentView';
import { conversionWord, short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, type FailureMessage, messageFor } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { mayControl, useControlRights } from './job-control';
import { canCancel, canRetry, jobName, jobState } from './job-words';
import { jobUncertain, rememberJob, unfinished } from './jobs';

export interface JobRowProps {
  workspaceId: string;
  job: Job;
}

/** Item links shown at first, and added by each "Show more". */
export const JOB_ITEMS_STEP = 25;

const ARTIFACT: Record<ArtifactKind, string> = {
  export: 'an export of the workspace',
  workspace_backup: 'a backup of the workspace',
  installation_backup: 'a backup of the installation',
  view_export: 'an export of a View',
};

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** A `conflict` on a cancel or a retry: not "this changed since it was loaded", and no reload. */
const JOB_MOVED_ON: FailureMessage = {
  title: 'The job changed before this could be done.',
  advice:
    'It ended, was cancelled or was started again meanwhile. The job was read again: its row shows how it stands now, and what can be done with it.',
  action: { kind: 'none' },
};

/** The words of a failed cancel or retry, where the general ones would mislead. */
function jobWords(error: unknown): FailureMessage | undefined {
  const failure = classifyFailure(error);
  return failure.kind === 'api' && failure.error.code === 'conflict' ? JOB_MOVED_ON : undefined;
}

export function JobRow({ workspaceId, job }: JobRowProps) {
  const client = useQueryClient();
  const titleId = useId();
  const name = jobName(job);
  const rowRef = useRef<HTMLLIElement | null>(null);
  const body = { workspace_id: workspaceId, job_id: job.id };

  const [asking, setAsking] = useState(false);
  // Each mutation is given its idempotency key: a new one at a press, the failed one's at a retry.
  const cancel = useMutation({
    mutationFn: async (key: string) => {
      const { data } = await cancelJob({
        body: { ...body, idempotency_key: key },
        throwOnError: true,
      });
      return data;
    },
    onSuccess: (answered) => {
      setAsking(false);
      rememberJob(client, workspaceId, answered);
    },
    onError: () => jobUncertain(client, workspaceId, job.id),
  });
  const cancelFailure = useRetry({
    request: job.id,
    failed: cancel.isError,
    error: cancel.error,
    // The failed request's own key (for the type: a request that failed always had one).
    run: () => cancel.mutateAsync(cancel.variables ?? crypto.randomUUID()),
    loaded: () => rowRef.current,
  });

  const again = useMutation({
    mutationFn: async (key: string) => {
      const { data } = await retryJob({
        body: { ...body, idempotency_key: key },
        throwOnError: true,
      });
      return data;
    },
    onSuccess: (answered) => rememberJob(client, workspaceId, answered),
    onError: () => jobUncertain(client, workspaceId, job.id),
  });
  const againFailure = useRetry({
    request: job.id,
    failed: again.isError,
    error: again.error,
    // The failed request's own key, as for the cancel.
    run: () => again.mutateAsync(again.variables ?? crypto.randomUUID()),
    loaded: () => rowRef.current,
  });

  // Who may control this job: the permission of the operation that started it (ruling 8).
  const rights = useControlRights(workspaceId);
  const control = mayControl(job, rights.permissions, rights.route);
  const cancellable = control && canCancel(job);
  const retryable = control && canRetry(job);
  // The question is on screen only while the job can still be cancelled.
  const question = asking && cancellable;

  // A control that left with keyboard focus on it (see the header).
  const heldCancel: HeldFocus = useRef<HTMLElement | null>(null);
  const heldAgain: HeldFocus = useRef<HTMLElement | null>(null);
  const [cancelButton, cancelRef] = useWatchedPart<HTMLButtonElement>(heldCancel);
  const [, questionRef] = useWatchedPart<HTMLFieldSetElement>(heldCancel);
  const [, againRef] = useWatchedPart<HTMLButtonElement>(heldAgain);
  const keepRef = useRef<HTMLButtonElement | null>(null);
  useLayoutEffect(() => {
    if (takeHeld(heldCancel)) {
      (keepRef.current ?? cancelButton.current ?? rowRef.current)?.focus();
    }
    if (takeHeld(heldAgain)) rowRef.current?.focus();
  });

  const percent = Math.min(100, job.progress);
  return (
    <li ref={rowRef} tabIndex={-1} className="job" data-job={job.id} aria-labelledby={titleId}>
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
      <Outcome workspaceId={workspaceId} job={job} />
      {keyed(job.warnings, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}

      {question ? (
        <fieldset ref={questionRef} className="confirm">
          <legend>Cancel this job?</legend>
          <p>
            The work stops where it is. Files that were uploaded are kept by the service, and
            nothing that is already in the workspace is removed.
          </p>
          <button type="button" ref={keepRef} onClick={() => setAsking(false)}>
            Keep it running
          </button>{' '}
          {/* aria-disabled rather than disabled: the button keeps focus while it works. */}
          <button
            type="button"
            aria-disabled={cancel.isPending}
            onClick={() => {
              if (!cancel.isPending) cancel.mutate(crypto.randomUUID());
            }}
          >
            {cancel.isPending ? 'Cancelling…' : 'Cancel the job'}
          </button>
        </fieldset>
      ) : cancellable ? (
        <p className="job-actions">
          <button type="button" ref={cancelRef} onClick={() => setAsking(true)}>
            Cancel
          </button>
        </p>
      ) : null}
      {retryable && (
        <p className="job-actions">
          {/* aria-disabled rather than disabled: the button keeps focus while it works. */}
          <button
            type="button"
            ref={againRef}
            aria-disabled={again.isPending}
            onClick={() => {
              if (!again.isPending) again.mutate(crypto.randomUUID());
            }}
          >
            {again.isPending ? 'Asking for another try…' : 'Try the job again'}
          </button>
        </p>
      )}
      {cancelFailure.shown && (
        <FailureNotice
          context="Cancelling this job"
          error={cancelFailure.error}
          retry={cancelFailure}
          workspaceId={workspaceId}
          words={jobWords(cancelFailure.error)}
        />
      )}
      {againFailure.shown && (
        <FailureNotice
          context="Asking for another try of this job"
          error={againFailure.error}
          retry={againFailure}
          workspaceId={workspaceId}
          words={jobWords(againFailure.error)}
        />
      )}
    </li>
  );
}

/** When the job was accepted, started and ended, as far as the wire says. */
function JobTimes({ job }: { job: Job }) {
  const times = (
    [
      ['Accepted', job.created_at],
      ['Started', job.started_at],
      ['Ended', job.finished_at],
    ] as const
  ).flatMap(([label, at]) => (at === undefined || at === null ? [] : [{ label, at }]));
  if (times.length === 0) return null;
  return (
    <p className="meta">
      {times.map(({ label, at }, index) => (
        <span key={label}>
          {label} <Instant value={at} />
          {index < times.length - 1 ? '; ' : '.'}
        </span>
      ))}
    </p>
  );
}

/** What the job came to, as far as the wire says. */
function Outcome({ workspaceId, job }: { workspaceId: string; job: Job }) {
  if (job.state === 'cancelled') {
    return <p>It was cancelled. Files that were uploaded are kept by the service.</p>;
  }
  if (job.state === 'failed') {
    const error = job.error ?? null;
    const final = job.retryable === false ? <p>The service will not take this job again.</p> : null;
    if (error === null) {
      return (
        <>
          <p>The service recorded no reason.</p>
          {final}
        </>
      );
    }
    return (
      <>
        {final}
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
      </>
    );
  }
  if (job.state !== 'succeeded') return null;
  const revision = job.revision ?? null;
  const artifact = job.artifact ?? null;
  const restore = job.restore ?? null;
  return (
    <>
      {revision !== null && (
        <p>
          It made revision <code title={revision}>{short(revision)}</code> of the workspace.
        </p>
      )}
      {artifact !== null && (
        <p>
          It produced{' '}
          {wordsFor(ARTIFACT, artifact.kind) ??
            `something of a kind this page does not know (“${artifact.kind}”)`}
          : {artifact.size} bytes.
        </p>
      )}
      {restore !== null && (
        <p>
          It restored {count(restore.items, 'item', 'items')} and{' '}
          {count(restore.drafts_restored, 'draft', 'drafts')};{' '}
          {count(restore.drafts_unassigned, 'draft was', 'drafts were')} left unassigned.
        </p>
      )}
      {job.item_ids.length > 0 &&
        (revision === null ? (
          <p>
            It made {count(job.item_ids.length, 'item', 'items')}. The job reports no revision, so
            they are not linked from here: refresh the workspace to find them in its folders.
          </p>
        ) : (
          <JobItems
            workspaceId={workspaceId}
            itemIds={job.item_ids}
            revision={revision}
            label={`Items made by ${jobName(job)}`}
          />
        ))}
    </>
  );
}

function JobItems({
  workspaceId,
  itemIds,
  revision,
  label,
}: {
  workspaceId: string;
  itemIds: readonly string[];
  revision: Revision;
  label: string;
}) {
  const [shown, setShown] = useState(JOB_ITEMS_STEP);
  const listRef = useRef<HTMLUListElement | null>(null);
  // "Show more" moves keyboard focus to the first link it brought; the button may be gone.
  const focusFrom = useRef<number | null>(null);
  useLayoutEffect(() => {
    const from = focusFrom.current;
    if (from === null) return;
    focusFrom.current = null;
    listRef.current?.querySelectorAll<HTMLElement>('a').item(from)?.focus();
  });
  const rest = itemIds.length - shown;
  return (
    <>
      <ul ref={listRef} className="job-items" aria-label={label}>
        {itemIds.slice(0, shown).map((itemId) => (
          <JobItem key={itemId} workspaceId={workspaceId} itemId={itemId} revision={revision} />
        ))}
      </ul>
      {rest > 0 && (
        <button
          type="button"
          onClick={() => {
            focusFrom.current = shown;
            setShown(shown + JOB_ITEMS_STEP);
          }}
        >
          Show {count(Math.min(rest, JOB_ITEMS_STEP), 'more item', 'more items')} of {rest}
        </button>
      )}
    </>
  );
}

/** A link to one item the job made, read at the revision the job reports. */
function JobItem({
  workspaceId,
  itemId,
  revision,
}: {
  workspaceId: string;
  itemId: string;
  revision: Revision;
}) {
  const item = useQuery(itemOptions(workspaceId, itemId, revision));
  const summary = item.data?.summary;
  const state = summary === undefined ? null : conversionWord(summary.extraction);
  return (
    <li>
      <Link
        to="/w/$workspaceId/i/$itemId"
        params={{ workspaceId, itemId }}
        search={{ rev: revision }}
      >
        {summary ? summary.title || summary.path : `Item ${itemId.slice(0, 8)}`}
      </Link>
      {summary && summary.title.length > 0 ? <span className="meta"> {summary.path}</span> : null}
      {state === null ? null : <span className="meta"> ({state})</span>}
    </li>
  );
}
