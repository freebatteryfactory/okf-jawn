/**
 * "Purge this item…" (rulings C3 and C7), offered among the item's own actions in its pane
 * (`tree/ItemActions.tsx`), beside Rename or move and Delete: where a person looks for what can be
 * done to the item. It is offered to a person whose grant here includes `admin`; to anyone else it
 * is not there (the Administration page, which they are not offered either, is where they would
 * look for administration). It stays offered in an archived workspace, where the service runs it
 * (`ALLOWED_WHILE_ARCHIVED`).
 *
 * It asks through the same dialog as a workspace's purge: SPEC section 8's words for an item, the
 * item it acts on (its title, path and identity) and the head the pane shows, to which it is bound
 * when it opens; the human-session rule in words. `purge_item` is not built on main: the service
 * answers `not_implemented`, said in the dialog in the general words ("Not available in this
 * build.", and that nothing was done), with no retry, and nothing is removed. Were it to answer a
 * purge, the page would go to its record, as for a workspace.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import type { ItemSummary, Purge } from '../../../api/generated/types.gen';
import { short } from '../../documents/describe';
import { allowedOperation, archivedWords, humanSessionWords } from '../../explorer/rights';
import { useWorkspaceChanges } from '../../tree/context';
import { DestructiveDialog, useDestructive } from '../destructive';
import { PurgeWords } from './PurgeWords';
import { type ItemPurge, purgeRefusedWords, rememberPurge, startItemPurge } from './purges';

const RULE = humanSessionWords('Purging an item');

export interface PurgeItemProps {
  /** The item as the pane last read it, at the head the workspace shows. */
  item: ItemSummary;
  /** The pane is reading the item again: nothing is asked meanwhile. */
  busy: boolean;
}

export function PurgeItem({ item, busy }: PurgeItemProps) {
  const { workspaceId, head, rights } = useWorkspaceChanges();
  const client = useQueryClient();
  const navigate = useNavigate();
  const archivedId = useId();
  const opener = useRef<HTMLButtonElement | null>(null);
  const purge = useDestructive<ItemPurge, Purge>({
    act: startItemPurge,
    returnFocus: opener,
    onDone: (answered) => {
      rememberPurge(client, answered);
      void navigate({ to: '/purges/$purgeId', params: { purgeId: answered.id } });
    },
  });
  const can = allowedOperation(rights, 'purge_item', 'admin');
  if (can === 'no-permission') return null;
  const stopped = can === 'archived';
  const bound = purge.opened;
  const title = item.title || item.path;
  const words = purge.failure.shown ? purgeRefusedWords(purge.failure.error) : undefined;
  return (
    <>
      <p className="document-actions">
        <button
          type="button"
          ref={opener}
          aria-disabled={busy || stopped}
          {...(stopped ? { 'aria-describedby': archivedId } : {})}
          onClick={() => {
            if (busy || stopped) return;
            purge.open({ workspaceId, itemId: item.id, baseRevision: head });
          }}
        >
          Purge this item…
        </button>
      </p>
      {stopped && rights.archivedAt !== null && (
        <p id={archivedId} className="meta">
          {archivedWords(rights.archivedAt)}
        </p>
      )}
      <DestructiveDialog
        action={purge}
        title="Purge this item?"
        description={<PurgeWords of="item" />}
        confirmLabel="Purge the item"
        busyLabel="Purging…"
        context="Purging this item"
        workspaceId={workspaceId}
        {...(words === undefined ? {} : { words })}
      >
        {bound !== null && (
          <>
            <p>
              The item “{title}” ({item.path}, <code>{bound.itemId}</code>), as of revision{' '}
              <code title={bound.baseRevision}>{short(bound.baseRevision)}</code>: the version this
              page showed when you asked. If the workspace has changed since, nothing is removed and
              the service says so.
            </p>
            <p>{RULE}</p>
          </>
        )}
      </DestructiveDialog>
    </>
  );
}
