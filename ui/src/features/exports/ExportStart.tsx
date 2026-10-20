/**
 * What both export controls share: one press is one request, repeated under its key while it asks
 * the same thing (`exportAttempt`), and what came of it, said in place.
 *
 * The answer is a job, and is said as one: accepted as that job, in the state the service reported
 * when it answered, never as an export that has finished. The words stay true as the job moves on
 * (they say when that state was reported, and where the job is followed), so they are not asked
 * again. A polite status says them, and says "Starting the export…" while the request is out. A refusal is shown in the words of its code with the service's
 * own beside them (`FailureNotice`); the notice's "Try again", where the failure allows one, repeats
 * the request under the key it had (`useRetry`), so a request whose answer was lost does not start
 * a second export. Keyboard focus that was in the notice when it leaves goes to what the caller
 * names (`loaded`: its button).
 *
 * Everything said is about what is asked now (`request`): a control that stays on the page while
 * it is given another thing to export (a View's tab moved to another revision) says nothing of a
 * press made for the one before, whether it is still out, failed or was answered, and its "Try
 * again" never repeats that press. That press goes on (its job, if it makes one, is listed on the
 * Jobs page as every job is), and a press here is not held back by it. Each request keeps its own
 * last attempt, so coming back to one that was never answered asks again under the key it had.
 */
import { useMutation } from '@tanstack/react-query';
import { useRef } from 'react';
import type { Job } from '../../api/generated/types.gen';
import { FailureNotice } from '../errors/FailureNotice';
import { type FailureRetry, useRetry } from '../errors/retry';
import { jobName, jobStateSaid } from '../jobs/job-words';
import { type ExportAttempt, exportAttempt } from './exports';

/** One press as sent: the attempt, and the request it was made for. */
interface Asked<T> {
  request: string;
  attempt: ExportAttempt<T>;
}

export interface ExportStart<T> {
  /** Start the export of `values`; nothing is sent while a request is out. */
  begin: (values: T) => void;
  /** A request is out. */
  pending: boolean;
  /** The job the last request answered, once it has. */
  started: Job | null;
  /** Its failure, kept on screen through its retry. */
  failure: FailureRetry;
}

export interface ExportStartOptions<T> {
  /** What is asked, as text that changes when the request does (`useRetry`'s `request`). */
  request: string;
  /** Send one request. */
  start: (attempt: ExportAttempt<T>) => Promise<Job>;
  /** Where keyboard focus goes when the notice leaves with focus in it: the control's button. */
  loaded: () => HTMLElement | null;
}

export function useExportStart<T>({
  request,
  start,
  loaded,
}: ExportStartOptions<T>): ExportStart<T> {
  // The last attempt of each request, until a job answers it.
  const attempts = useRef(new Map<string, ExportAttempt<T>>());
  const send = useMutation({
    mutationFn: ({ attempt }: Asked<T>) => start(attempt),
    // Answered: the next press for that request is a new export.
    onSuccess: (_job, asked) => {
      if (attempts.current.get(asked.request) === asked.attempt)
        attempts.current.delete(asked.request);
    },
  });
  // The last press is shown only while it is a press for what is asked now.
  const asked = send.variables;
  const mine = asked !== undefined && asked.request === request;
  const begin = (values: T) => {
    if (mine && send.isPending) return;
    const attempt = exportAttempt(attempts.current.get(request) ?? null, values);
    attempts.current.set(request, attempt);
    send.mutate({ request, attempt });
  };
  const failure = useRetry({
    request,
    failed: mine && send.isError,
    error: mine ? send.error : null,
    // The failed request again, under its key; never a press made for another request.
    run: () => (mine ? send.mutateAsync(asked) : Promise.resolve()),
    loaded,
  });
  return {
    begin,
    pending: mine && send.isPending,
    started: mine && send.isSuccess ? send.data : null,
    failure,
  };
}

/** What an accepted export is said to be: the job it was accepted as, and how that stood then. */
export function acceptedWords(what: string, job: Job): string {
  return `${what} was accepted as ${jobName(job)}: “${jobStateSaid(job.state)}” when the service answered.`;
}

export interface ExportSaidProps<T> {
  start: ExportStart<T>;
  /** The export, as the start of a sentence: "The export". */
  what: string;
  /** Said after the job's state, where the person finds the job. */
  where?: string;
  /** What the person was doing, for the failure notice. */
  context: string;
  workspaceId: string;
}

/** The status of the last press, and its failure with its retry. */
export function ExportSaid<T>({ start, what, where, context, workspaceId }: ExportSaidProps<T>) {
  const said =
    start.pending && !start.failure.retrying
      ? 'Starting the export…'
      : start.started === null
        ? ''
        : `${acceptedWords(what, start.started)}${where === undefined ? '' : ` ${where}`}`;
  return (
    <>
      {/* Present from the start, so what pressing did is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
      {start.failure.shown && (
        <FailureNotice
          context={context}
          error={start.failure.error}
          retry={start.failure}
          workspaceId={workspaceId}
        />
      )}
    </>
  );
}
