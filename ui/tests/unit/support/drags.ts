/**
 * Drags onto a control, and what the control is handed of them.
 *
 * happy-dom delivers the events and has no default action for a drop, so "inserted" cannot be
 * seen here: what is seen is whether the drag was cancelled before the control got it. A browser
 * inserts dropped text where the drop was not cancelled, and neither inserts a dropped file nor
 * opens it where it was (the journeys in `explorer-editing.spec.ts` and `explorer-changes.spec.ts`
 * show the insertion itself, in a browser).
 */
import { createEvent, fireEvent } from '@testing-library/react';
import { dropOf, droppedFile, fileOf } from './imports';

export interface Seen {
  type: string;
  cancelled: boolean;
}

/**
 * A record of the drags `box` is handed: a capturing listener on the box itself, which runs after
 * the window's capturing listeners (the root guard) and before any handler of the control's own.
 * The control's own handlers are not run.
 */
export function seenBy(box: Element): Seen[] {
  const seen: Seen[] = [];
  for (const type of ['dragover', 'drop']) {
    box.addEventListener(
      type,
      (event) => {
        seen.push({ type, cancelled: event.defaultPrevented });
        event.stopImmediatePropagation();
      },
      { capture: true },
    );
  }
  return seen;
}

/** A whole drag onto `target`, as a browser delivers one. */
export function dragOnto(target: Element, carried: unknown) {
  fireEvent.dragEnter(target, { dataTransfer: carried });
  fireEvent(target, createEvent.dragOver(target, { dataTransfer: carried }));
  fireEvent(target, createEvent.drop(target, { dataTransfer: carried }));
}

/** A drag of one file from the computer. */
export const fileDrag = () => dropOf([droppedFile(fileOf('private.txt', 'DROPPED-FROM-DISK'))]);

/** A drag of text: a selection, or text from another application. */
export const textDrag = () => ({
  types: ['text/plain'],
  items: [],
  files: [],
  dropEffect: 'none',
  effectAllowed: 'all',
});

export const TAKEN: Seen[] = [
  { type: 'dragover', cancelled: false },
  { type: 'drop', cancelled: false },
];

export const CANCELLED: Seen[] = [
  { type: 'dragover', cancelled: true },
  { type: 'drop', cancelled: true },
];

/**
 * What `box` is handed of a drag of text and then of a drag of files. Text first, as the test of
 * the draft editor has it (`drop-editor.test.tsx`): nothing a drag of files leaves behind on the
 * page can then be what lets the text through or holds it back.
 */
export function handed(box: Element): { text: Seen[]; files: Seen[] } {
  const seen = seenBy(box);
  dragOnto(box, textDrag());
  const text = seen.splice(0);
  dragOnto(box, fileDrag());
  return { text, files: seen.splice(0) };
}
