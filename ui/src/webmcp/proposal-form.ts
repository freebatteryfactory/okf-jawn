/**
 * The Propose change forms on the page that the page tool `prepare_proposal` may fill for the
 * person (`tools.ts`), by workspace and item. A form takes part only while it can be used: its
 * item is a Note shown at the version the workspace shows, and the person may propose there
 * (`proposals/ProposeChange.tsx`). It is filled, never sent: only the person presses Propose.
 *
 * A form answers each fill itself, from its state when it is asked: `filled`; `busy` when the
 * person is writing in it (what it holds is not what it was opened or last filled with), so their
 * words are never written over; or `stale` when it was asked for a base revision other than the
 * one the workspace shows. With no such form on the page the answer is `absent`. Two panes may show
 * the same note: the form that took part last is filled.
 */
import { useEffect, useLayoutEffect, useRef } from 'react';
import type { Revision } from '../api/generated/types.gen';

/** What `prepare_proposal` puts in the form: its title, its reason and the note's new text. */
export interface PreparedProposal {
  title: string;
  description: string;
  body: string;
  /** The revision the tool found on screen: the version the workspace shows. */
  base: Revision;
}

export type FillOutcome = 'filled' | 'busy' | 'stale';

type Fill = (prepared: PreparedProposal) => FillOutcome;

/** The forms taking part, oldest first, by workspace and item. */
const forms = new Map<string, Fill[]>();

const keyOf = (workspaceId: string, itemId: string) => `${workspaceId}/${itemId}`;

/** Fill the Propose change form of an item, when the page shows one that takes part. */
export function fillProposalForm(
  workspaceId: string,
  itemId: string,
  prepared: PreparedProposal,
): FillOutcome | 'absent' {
  const fill = forms.get(keyOf(workspaceId, itemId))?.at(-1);
  return fill === undefined ? 'absent' : fill(prepared);
}

/** The form of `itemId` takes part while `active`; `fill` answers with its state at that moment. */
export function useFillableProposalForm(
  workspaceId: string,
  itemId: string,
  active: boolean,
  fill: Fill,
) {
  const latest = useRef(fill);
  useLayoutEffect(() => {
    latest.current = fill;
  });
  useEffect(() => {
    if (!active) return;
    const key = keyOf(workspaceId, itemId);
    const entry: Fill = (prepared) => latest.current(prepared);
    forms.set(key, [...(forms.get(key) ?? []), entry]);
    return () => {
      const left = (forms.get(key) ?? []).filter((taking) => taking !== entry);
      if (left.length === 0) forms.delete(key);
      else forms.set(key, left);
    };
  }, [workspaceId, itemId, active]);
}
