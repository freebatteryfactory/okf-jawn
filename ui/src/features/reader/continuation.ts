/**
 * "Continue reading" (ruling A1): a partial read's further part, asked with the cursor the last
 * part gave, and where keyboard focus goes when it has settled.
 *
 * The rule, found against the carried defect (`tests/unit/m4a-reader-continue.test.tsx`): a
 * continuation moves keyboard focus only from where it left it. That is the button, while it is
 * there; the page body, when a pointer press focused nothing or the button left the page with
 * focus on it (the last part arrived, and with it the end of the partial reading); and nothing
 * else. Focus the person moved on while the part was read (to the properties, the search box,
 * another pane) stays where they put it.
 *
 * - The part arrived: focus goes to it, so reading goes on there.
 * - The continuation failed: focus stays on the button, which is still there, and the failure's
 *   notice offers the retry (`useRetry`, which follows its own notice).
 * - What was asked belongs to one reading (`reading`: the request as text). Another view,
 *   selection or revision is another reading, and what was asked of the first is dropped.
 *
 * Focus moves in a layout effect, in the commit that shows the part: never a painted frame with
 * focus on the body. The button is watched as it leaves (`useWatchedPart`): a button that leaves
 * with focus on it for any other reason within the same reading (the reading was read again and
 * is whole now) gives focus to the last part on the page. A replacement reading never receives it.
 */
import { type RefCallback, type RefObject, useLayoutEffect, useRef } from 'react';
import { takeHeld, useWatchedPart } from '../../lib/focus';

/** The parts of the reading on screen: the `[data-part]` children of its body. */
function partsIn(body: HTMLElement | null): HTMLElement[] {
  return body === null
    ? []
    : Array.from(body.children).filter(
        (child): child is HTMLElement =>
          child instanceof HTMLElement && child.hasAttribute('data-part'),
      );
}

/** A part of the reading on screen, as an element keyboard focus can go to. */
export function partOf(
  body: HTMLElement | null,
  which: 'first' | 'last' | number,
): HTMLElement | null {
  const parts = partsIn(body);
  const index = which === 'first' ? 0 : which === 'last' ? parts.length - 1 : which;
  return parts[index] ?? null;
}

/** Keyboard focus is nowhere the person put it: on the page body, or on nothing. */
function focusOnBody(): boolean {
  return document.activeElement === null || document.activeElement === document.body;
}

export interface ContinuedRead {
  /** Parts on screen. */
  parts: number;
  /** A further part is being read. */
  continuing: boolean;
  /** The last further part asked failed and has not answered since. */
  failed: boolean;
  /** Ask the further part. */
  fetchNextPage: () => Promise<unknown>;
}

export interface Continuation {
  /** For the element holding the parts, each a `[data-part]` child. */
  bodyRef: RefObject<HTMLDivElement | null>;
  /** For "Continue reading". */
  buttonRef: RefCallback<HTMLButtonElement>;
  /** "Continue reading" was pressed. */
  continueReading: () => void;
}

export function useContinuation(read: ContinuedRead, reading: string): Continuation {
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const held = useRef<HTMLElement | null>(null);
  const [button, buttonRef] = useWatchedPart<HTMLButtonElement>(held);
  // The part asked for, in which reading.
  const asked = useRef<{ reading: string; part: number } | null>(null);
  const previousReading = useRef(reading);
  const { parts, continuing, failed } = read;

  useLayoutEffect(() => {
    const changed = previousReading.current !== reading;
    previousReading.current = reading;
    const left = takeHeld(held);
    const wanted = asked.current;
    if (wanted !== null && wanted.reading !== reading) asked.current = null;
    const mine = asked.current;
    const free = left || focusOnBody() || document.activeElement === button.current;
    if (mine !== null && parts > mine.part) {
      asked.current = null;
      if (free) partOf(bodyRef.current, mine.part)?.focus();
    } else if (mine !== null && failed && !continuing) {
      asked.current = null;
      if (free) (button.current ?? partOf(bodyRef.current, 'last'))?.focus();
    } else if (left && !changed) {
      // The button left with focus on it, though nothing asked of it settled now.
      // A different cached selection/revision must not inherit that focus transfer.
      partOf(bodyRef.current, 'last')?.focus();
    }
  });

  const continueReading = () => {
    if (continuing) return;
    asked.current = { reading, part: parts };
    void read.fetchNextPage();
  };

  return { bodyRef, buttonRef, continueReading };
}
