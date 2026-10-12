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
 * A failure also changes without a retry: something else asks the operation again (a query
 * refetched when the window regains focus), or the component asks for something else. Keyboard
 * focus that was in the notice when the notice, or its retry button, left the page follows that
 * change the same way: to the content `loaded` names once no failure is shown, else to the retry
 * button of the notice now shown, or to that notice itself when its failure allows no retry.
 *
 * When `loaded` names nothing (a further page that brought nothing and left no control behind, a
 * request that has only just been made), focus goes to the page's level-one heading instead of
 * staying on the page body. The workspace list, the Explorer and the page shown before the Explorer
 * each have one that takes focus from script (`tabindex="-1"`); a page added later needs one too.
 *
 * How the hook knows: React clears an element's ref before it removes the element, so the refs
 * given to the notice and to its button are cleared while focus is still where it was.
 *
 * Focus is moved in a layout effect: in the commit that changes the page, before the browser
 * paints it. A passive effect runs after the paint, so a frame could be drawn with focus on the
 * page body.
 *
 * Call the hook where the operation is read, not inside the notice: settling re-renders that
 * component, which reads the operation's state as it is then and renders what it loaded in the
 * same commit the focus moves in. A ref `loaded` reads must be on an element that component
 * renders, itself or through its children: those refs are set before its layout effects run. The
 * retry button exists only through this hook.
 *
 * Limits. The retry is over when `run` settles. A refetch that replaces a request still in flight
 * for content already on screen (an invalidation; nothing does that yet) settles `run` early,
 * while the replacement is still in flight; the outcome followed is then the state at that moment.
 * A notice that leaves because the component calling the hook is itself unmounted is not followed:
 * the hook goes with it.
 */
import { type RefCallback, useCallback, useLayoutEffect, useRef, useState } from 'react';

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
  /** Repeat the operation; nothing happens while a retry is in flight. */
  start: () => void;
  /** For the notice's element: where keyboard focus is looked for and, with no retry left, put. */
  noticeRef: RefCallback<HTMLDivElement>;
  /** For the notice's retry button: where keyboard focus is put while a failure is shown. */
  buttonRef: RefCallback<HTMLButtonElement>;
}

/** The last retry asked for: in flight until `outcome` says where keyboard focus was at its end. */
interface Attempt {
  number: number;
  request: string;
  /** The failure being repeated, shown until the retry settles. */
  error: unknown;
  outcome: { focusWasInNotice: boolean } | null;
}

/**
 * A ref of the notice goes from `previous` to `next`. React clears a ref before it removes the
 * element, while keyboard focus is still in it: an element cleared with focus in it is held. An
 * element whose ref is set again has not left the page, and is let go: in development, StrictMode
 * clears and sets again the refs of a notice that has just mounted.
 */
function refChanged(
  held: { current: HTMLElement | null },
  previous: HTMLElement | null,
  next: HTMLElement | null,
) {
  if (previous?.contains(document.activeElement)) held.current = previous;
  if (next === held.current) held.current = null;
}

export function useRetry({ request, failed, error, run, loaded }: RetriedOperation): FailureRetry {
  const [attempt, setAttempt] = useState<Attempt | null>(null);
  const notice = useRef<HTMLDivElement | null>(null);
  const button = useRef<HTMLButtonElement | null>(null);
  const followed = useRef(0);
  const mine = attempt !== null && attempt.request === request;
  const retrying = mine && attempt.outcome === null;

  // The notice, or its button, that left the page with keyboard focus in it (see `refChanged`).
  const heldFocus = useRef<HTMLElement | null>(null);
  const noticeRef = useCallback((element: HTMLDivElement | null) => {
    refChanged(heldFocus, notice.current, element);
    notice.current = element;
  }, []);
  const buttonRef = useCallback((element: HTMLButtonElement | null) => {
    refChanged(heldFocus, button.current, element);
    button.current = element;
  }, []);

  // In each commit, before the browser paints it: `failed` is the operation's state as the commit
  // shows it, and what it loaded is on the page. Focus follows a settled retry once, and a notice
  // or button that left the page with focus in it.
  useLayoutEffect(() => {
    const left = heldFocus.current !== null;
    heldFocus.current = null;
    let settledWithFocus = false;
    if (attempt !== null && attempt.outcome !== null && followed.current !== attempt.number) {
      followed.current = attempt.number;
      settledWithFocus = attempt.outcome.focusWasInNotice && attempt.request === request;
    }
    if (!left && !settledWithFocus) return;
    if (failed) (button.current ?? notice.current)?.focus();
    else (loaded() ?? document.querySelector<HTMLElement>('h1'))?.focus();
  });

  const start = () => {
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
    void run().then(settle, settle);
  };

  return {
    shown: failed || retrying,
    error: retrying ? attempt.error : error,
    retrying,
    settled: attempt === null ? 0 : attempt.outcome === null ? attempt.number - 1 : attempt.number,
    start,
    noticeRef,
    buttonRef,
  };
}
