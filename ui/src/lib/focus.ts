/**
 * Keeping keyboard focus off the page body: the page's level-one heading, where focus goes when
 * nothing nearer can take it, and a way to see that a part of the page left with focus in it.
 *
 * Each page of the application has a level-one heading that takes focus from script
 * (`tabindex="-1"`): the workspace list's, the Explorer's (the workspace's name) and the one of
 * the page shown before the Explorer. The reader nests a document's headings beneath its item
 * title. Unrelated content can still contain level-one headings; only the application's heading
 * carries this focus target's tabindex.
 *
 * Seeing a part leave: React clears an element's ref before it removes the element, so a ref
 * callback is called while focus is still where it was (react-dom 19: `safelyDetachRef`, then
 * `removeChild`). Nothing else says so in time: once the element is gone, focus is on the body.
 */
import { type RefCallback, type RefObject, useCallback, useRef } from 'react';

/** The page's level-one heading, when the page has one that takes focus from script. */
export function pageHeading(): HTMLElement | null {
  return document.querySelector<HTMLElement>('h1[tabindex="-1"]');
}

/** The part that left the page with keyboard focus in it, for the parts watched together. */
export type HeldFocus = RefObject<HTMLElement | null>;

/**
 * A ref of a watched part goes from `previous` to `next`. A part cleared with focus in it is
 * held. A part whose ref is set again has not left the page, and is let go: in development,
 * StrictMode clears and sets again the refs of what has just mounted.
 */
function refChanged(held: HeldFocus, previous: HTMLElement | null, next: HTMLElement | null) {
  if (previous?.contains(document.activeElement)) held.current = previous;
  if (next === held.current) held.current = null;
}

/**
 * One part of the page watched for leaving with keyboard focus in it: the part as it is on the
 * page now, and the ref to give its element. Parts watched together share `held`.
 */
export function useWatchedPart<T extends HTMLElement>(
  held: HeldFocus,
): [RefObject<T | null>, RefCallback<T>] {
  const part = useRef<T | null>(null);
  const ref = useCallback(
    (element: T | null) => {
      refChanged(held, part.current, element);
      part.current = element;
    },
    [held],
  );
  return [part, ref];
}

/**
 * Whether a watched part left the page with focus in it since this was last asked. Ask in a
 * layout effect of the component that renders the parts, in every commit: what is held is then
 * what that commit removed.
 */
export function takeHeld(held: HeldFocus): boolean {
  const left = held.current !== null;
  held.current = null;
  return left;
}
