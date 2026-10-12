/**
 * One operation's failure, kept on screen through its retry, so that a retry never unmounts the
 * control that has keyboard focus.
 *
 * A query that failed with nothing loaded leaves its error state the moment it is asked again. A
 * notice rendered only while the query is failed would go with it, and take its focused button
 * along: focus would fall to the page body. So the failure shown here is the operation's own or,
 * while a retry is in flight, the one being repeated. The notice stays, its button reads "Trying
 * again…" with `aria-disabled`, and a polite status says so (see `FailureNotice`).
 *
 * When the retry settles, keyboard focus that was in the notice follows the outcome: to the
 * content the retry loaded (the caller says where: its first focusable element, or its heading), or
 * it stays on the notice's retry button when the operation failed again (on the notice itself when
 * the new failure allows no retry). Focus the person moved elsewhere meanwhile is left alone.
 *
 * Call the hook where the operation is read, not inside the notice: settling re-renders that
 * component, which reads the operation's state as it is then and renders what it loaded in the
 * same commit the focus moves in. The retry button exists only through this hook.
 *
 * Limit: the retry is over when `run` settles. A refetch that replaces a request still in flight
 * for content already on screen (an invalidation; nothing does that yet) settles `run` early,
 * while the replacement is still in flight; the outcome followed is then the state at that moment.
 */
import { type RefObject, useEffect, useRef, useState } from 'react';

export interface RetriedOperation {
  /**
   * What is asked, as text that changes when the request does (another item, another revision).
   * A retry in flight for another request is not this one's; a component that only ever asks one
   * thing passes a constant. A query whose request is abandoned when its component asks something
   * else (its last observer leaves and its fetch is aborted) settles its retry at once anyway; the
   * text is what covers an operation that keeps running: a listing made of several requests, a
   * mutation.
   */
  request: string;
  /** The operation has failed and has not answered since. */
  failed: boolean;
  /** Whatever it threw. */
  error: unknown;
  /** Repeat it. Settles once the operation has answered or failed again; a rejection is a failure. */
  run: () => Promise<unknown>;
  /** The content a successful retry loaded: its first focusable element, or its heading. */
  loaded: () => HTMLElement | null | undefined;
}

export interface FailureRetry {
  /** A failure is on screen: the operation's own or, while a retry is in flight, the one repeated. */
  shown: boolean;
  /** That failure. */
  error: unknown;
  /** A retry is in flight. */
  retrying: boolean;
  /** How many retries have settled: the words shown after each are a new alert. */
  settled: number;
  /** Repeat the operation; nothing happens while a retry is in flight. */
  start: () => void;
  /** The notice's element: where keyboard focus is looked for and, with no retry left, put. */
  noticeRef: RefObject<HTMLDivElement | null>;
}

/** The last retry asked for: in flight until `outcome` says where keyboard focus was at its end. */
interface Attempt {
  number: number;
  request: string;
  /** The failure being repeated, shown until the retry settles. */
  error: unknown;
  outcome: { focusWasInNotice: boolean } | null;
}

export function useRetry({ request, failed, error, run, loaded }: RetriedOperation): FailureRetry {
  const [attempt, setAttempt] = useState<Attempt | null>(null);
  const noticeRef = useRef<HTMLDivElement | null>(null);
  const followed = useRef(0);
  const mine = attempt !== null && attempt.request === request;
  const retrying = mine && attempt.outcome === null;

  // Once per settled retry, in the commit that shows its outcome: `failed` is the operation's
  // state after the retry, and what it loaded is on screen.
  useEffect(() => {
    if (attempt === null || attempt.outcome === null || followed.current === attempt.number) return;
    followed.current = attempt.number;
    if (!attempt.outcome.focusWasInNotice || attempt.request !== request) return;
    if (failed) {
      const notice = noticeRef.current;
      (notice?.querySelector<HTMLElement>('button') ?? notice)?.focus();
    } else loaded()?.focus();
  });

  const start = () => {
    if (retrying) return;
    const number = (attempt?.number ?? 0) + 1;
    setAttempt({ number, request, error, outcome: null });
    const settle = () => {
      const notice = noticeRef.current;
      const focusWasInNotice = notice?.contains(document.activeElement) ?? false;
      // A later retry (for another request) has replaced this one: its outcome is not followed.
      setAttempt((current) =>
        current !== null && current.number === number
          ? { ...current, outcome: { focusWasInNotice } }
          : current,
      );
    };
    void run().then(settle, settle);
  };

  return {
    shown: failed || retrying,
    error: retrying ? attempt.error : error,
    retrying,
    settled: attempt === null ? 0 : attempt.outcome === null ? attempt.number - 1 : attempt.number,
    start,
    noticeRef,
  };
}
