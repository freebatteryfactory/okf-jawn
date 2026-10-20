/**
 * The reader's choice of view (ruling A1): one radio per view the item has, in a group the
 * keyboard reaches with one Tab and moves through with the arrow keys, as a radio group does. The
 * view shown follows the choice at once; keyboard focus stays on the choice. Under it, the views
 * the item cannot answer are named, each with why (`views.ts`): none is offered and then refused.
 *
 * A view a newer answer about the item no longer offers leaves the group; keyboard focus that was
 * on it goes to the view shown in its place.
 */
import { type ReactNode, useId, useLayoutEffect, useRef } from 'react';
import type { ReadView } from '../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../lib/focus';
import { type ItemViews, refusedWords, VIEW_NAMES } from './views';

export function ViewSwitch({
  views,
  view,
  onView,
  unknown,
}: {
  views: ItemViews;
  /** The view shown. */
  view: ReadView;
  onView: (view: ReadView) => void;
  /** Words for the views that cannot be known yet (the item's properties did not read). */
  unknown?: ReactNode;
}) {
  const name = useId();
  const label = useId();
  const refused = refusedWords(views.refused);
  const groupRef = useRef<HTMLDivElement | null>(null);
  const held = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (takeHeld(held)) {
      groupRef.current?.querySelector<HTMLInputElement>('input:checked')?.focus();
    }
  });
  return (
    <div className="reader-views grid gap-1">
      <div
        ref={groupRef}
        role="radiogroup"
        aria-labelledby={label}
        className="flex flex-wrap items-center gap-x-4 gap-y-1"
      >
        <span id={label} className="font-bold">
          Read as
        </span>
        {views.offered.map((offered) => (
          <Choice
            key={offered}
            name={name}
            view={offered}
            checked={offered === view}
            onView={onView}
            held={held}
          />
        ))}
      </div>
      {refused !== null && <p className="meta m-0">{refused}</p>}
      {unknown}
    </div>
  );
}

function Choice({
  name,
  view,
  checked,
  onView,
  held,
}: {
  name: string;
  view: ReadView;
  checked: boolean;
  onView: (view: ReadView) => void;
  /** Where a choice that leaves with keyboard focus in it is noted. */
  held: HeldFocus;
}) {
  const [, ref] = useWatchedPart<HTMLLabelElement>(held);
  return (
    <label ref={ref} className="reader-view inline-flex items-center gap-1">
      <input
        type="radio"
        name={name}
        value={view}
        checked={checked}
        onChange={() => onView(view)}
      />
      {VIEW_NAMES[view]}
    </label>
  );
}
