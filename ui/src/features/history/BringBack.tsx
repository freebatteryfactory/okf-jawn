/**
 * Bringing back an item the person deleted in this page: Rewind of that item to the last version
 * that held it (the one the delete was made on), offered from the tab that says it was deleted.
 *
 * It is Rewind (`Rewind.tsx`): a new version that puts the item back where it was, with its
 * history kept. Once the workspace shows the version it made, the item answers there and the
 * page forgets that it was deleted (`explorer/restored.ts`), so its tab reads it again. It needs
 * permission to change the workspace; without it, nothing is offered. Closing the panel gives
 * keyboard focus back to the button that opened it.
 */
import { useLayoutEffect, useRef, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { allowed } from '../explorer/rights';
import { useWorkspaceChanges } from '../tree/context';
import { RewindPanel } from './Rewind';

export interface BringBackProps {
  workspaceId: string;
  itemId: string;
  title: string;
  /** The last version that held the item. */
  at: Revision;
}

export function BringBack({ workspaceId, itemId, title, at }: BringBackProps) {
  const { rights } = useWorkspaceChanges();
  const [open, setOpen] = useState(false);
  const opener = useRef<HTMLButtonElement | null>(null);
  const giveBack = useRef(false);
  useLayoutEffect(() => {
    if (!giveBack.current || open) return;
    giveBack.current = false;
    opener.current?.focus();
  });
  if (allowed(rights, 'write') === 'no-permission') return null;
  return (
    <>
      {' '}
      <button
        type="button"
        ref={opener}
        aria-expanded={open}
        onClick={() => setOpen((now) => !now)}
      >
        Bring it back…
      </button>
      {open && (
        <RewindPanel
          workspaceId={workspaceId}
          target={at}
          item={{ id: itemId, title }}
          level={3}
          onClose={() => {
            giveBack.current = true;
            setOpen(false);
          }}
        />
      )}
    </>
  );
}
