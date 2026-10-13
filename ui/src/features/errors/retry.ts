/**
 * One operation's failure, kept on screen through its retry, with keyboard focus kept off the page
 * body when the notice of that failure changes or leaves.
 *
 * A query that failed with nothing loaded leaves its error state the moment it is asked again. A
 * notice rendered only while the query is failed would go with it, and take its focused button
 * along: focus would fall to the page body. So the failure shown here is the operation's own or,
 * while a retry is in flight, the one being repeated. The notice stays, its button reads "Trying
 * again…" with `aria-disabled`, and a polite status says so (see `FailureNotice`).
 *
 * When the hook moves focus. In the commit that shows either of these:
 * - a retry of what is asked now has settled, and focus was in the notice when it settled (once
 *   for each retry);
 * - this hook's notice, its retry button or what holds its way out has left the page with focus in
 *   it. A retry does that, and so does a change without one: something else asks the operation
 *   again (a query refetched when the window regains focus or the network returns), the component
 *   asks for something else, another failure takes the button or the link away, or the caller
 *   shows something else in the notice's place.
 * Focus that is not in this hook's notice is never moved: not focus the person has moved
 * elsewhere, and not focus in the notice of another operation of the same caller.
 *
 * Where it moves focus: to the first of these that is there.
 * 1. While the operation is failed (`failed`), this hook's notice as it is on the page now: its
 *    retry button, else the notice itself (its failure allows no retry, or the caller gave no way
 *    to repeat the operation).
 * 2. What `loaded` names: the content now in the failure's place (its first focusable element, or
 *    its heading).
 * 3. The page's level-one heading (`pageHeading` in `lib/focus.ts`). The workspace list, the
 *    Explorer and the page shown before the Explorer each have one that takes focus from script;
 *    a page added later needs one too, or focus stays on the body there.
 * So a retry that loaded content ends on what `loaded` names, and one that failed again on the
 * retry button. Step 1 asks whether the operation is failed, not whether a failure is shown: while
 * a retry is in flight the operation is not failed, so a notice that leaves then is followed to
 * steps 2 and 3. And when the operation is failed but the caller no longer shows this hook's
 * notice (another failure of its own has the place), nothing is there for step 1, and focus goes
 * to steps 2 and 3 as well.
 *
 * How the hook knows that a part left with focus in it: React clears an element's ref before it
 * removes the element, so the refs given to the notice, its button and its way out are cleared
 * while focus is still where it was (`useWatchedPart` in `lib/focus.ts`).
 *
 * Focus is moved in a layout effect: in the commit that changes the page, before the browser
 * paints it. A passive effect runs after the paint, so a frame could be drawn with focus on the
 * page body.
 *
 * Call the hook where the operation is read, not inside the notice: settling re-renders that
 * component, which reads the operation's state as it is then and renders what it loaded in the
 * same commit the focus moves in. A ref `loaded` reads must be on an element that component
 * renders, itself or through its children: those refs are set before its layout effects run. The
 * notice must come and go with that component's own renders (it renders the notice from this
 * hook's result): the effect that moves focus runs when the component renders, so a notice taken
 * away by a component in between, on its own state, would be followed only at the next render.
 * The retry button exists only through this hook, and only where the caller gives it `run`.
 *
 * A retry replaced in flight. The retry is over when `run` settles. An invalidation of a query
 * with data on screen (the drafts list after a save, a discard or a Snapshot) cancels its request
 * in flight and asks again; TanStack Query then settles the cancelled request's promise with the
 * replacement's, so `run` settles when the replacement has answered, not before. The outcome
 * followed is the operation's state then: for a further page whose list is listed again from its
 * first page, that listing's (the further page is a new request after it, with its own outcome).
 *
 * Limit. A notice that leaves because the component calling the hook is itself unmounted is not
 * followed by the hook, which goes with it. When a navigation does that, the router's rule moves
 * the focus that fell to the page body to the new page's level-one heading (`app/router.tsx`).
 */
import { type RefCallback, useLayoutEffect, useRef, useState } from 'react';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';

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
  /**
   * Repeat it. Settles once the operation has answered or failed again; a rejection is a failure.
   * Left out where another control of the page repeats the operation (the shell's Refresh): the
   * notice then offers no retry of its own, and keyboard focus in it is followed all the same.
   */
  run?: () => Promise<unknown>;
  /**
   * The content now shown in the failure's place: its first focusable element, or its heading.
   * When it names nothing, focus goes to the page's level-one heading.
   */
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
  /**
   * Repeat the operation; nothing happens while a retry is in flight. Undefined when the caller
   * gave no way to repeat it: the notice then offers no retry button.
   */
  start: (() => void) | undefined;
  /** For the notice's element: where keyboard focus is looked for and, with no retry left, put. */
  noticeRef: RefCallback<HTMLDivElement>;
  /** For the notice's retry button: where keyboard focus is put while a failure is shown. */
  buttonRef: RefCallback<HTMLButtonElement>;
  /** For what holds the notice's way out, the link a `not_found` offers: focus on it is followed too. */
  wayOutRef: RefCallback<HTMLElement>;
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
  const followed = useRef(0);
  const mine = attempt !== null && attempt.request === request;
  const retrying = mine && attempt.outcome === null;

  // The notice, its retry button or its way out, when one left the page with keyboard focus in it.
  const heldFocus = useRef<HTMLElement | null>(null);
  const [notice, noticeRef] = useWatchedPart<HTMLDivElement>(heldFocus);
  const [button, buttonRef] = useWatchedPart<HTMLButtonElement>(heldFocus);
  const [, wayOutRef] = useWatchedPart<HTMLElement>(heldFocus);

  // In each commit, before the browser paints it: `failed` is the operation's state as the commit
  // shows it, and what it loaded is on the page. Focus follows a settled retry once, and a notice,
  // a button or a way out that left the page with focus in it.
  useLayoutEffect(() => {
    const left = takeHeld(heldFocus);
    let settledWithFocus = false;
    if (attempt !== null && attempt.outcome !== null && followed.current !== attempt.number) {
      followed.current = attempt.number;
      settledWithFocus = attempt.outcome.focusWasInNotice && attempt.request === request;
    }
    if (!left && !settledWithFocus) return;
    // The notice of a failure still shown, when this hook's notice is on the page. The caller may
    // show the failure otherwise, or not at all (another failure of its own takes the place).
    const shownNow = failed ? (button.current ?? notice.current) : null;
    (shownNow ?? loaded() ?? pageHeading())?.focus();
  });

  const start = (repeat: () => Promise<unknown>) => {
    if (retrying) return;
    const number = (attempt?.number ?? 0) + 1;
    setAttempt({ number, request, error, outcome: null });
    const settle = () => {
      const focusWasInNotice = notice.current?.contains(document.activeElement) ?? false;
      // A later retry (for another request) has replaced this one: its outcome is not followed.
      setAttempt((current) =>
        current !== null && current.number === number
          ? { ...current, outcome: { focusWasInNotice } }
          : current,
      );
    };
    void repeat().then(settle, settle);
  };

  return {
    shown: failed || retrying,
    error: retrying ? attempt.error : error,
    retrying,
    settled: attempt === null ? 0 : attempt.outcome === null ? attempt.number - 1 : attempt.number,
    start: run === undefined ? undefined : () => start(run),
    noticeRef,
    buttonRef,
    wayOutRef,
  };
}
