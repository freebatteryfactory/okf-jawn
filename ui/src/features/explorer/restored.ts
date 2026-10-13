/**
 * Forgetting that an item was deleted, once it is there again.
 *
 * The page remembers the items the person deleted in it (`store.ts`), so their tabs can say so
 * and nothing is asked about them at the head the delete made. That memory must not outlive its
 * truth: a later head can hold the item again (someone brought it back; a Rewind). So when the
 * workspace shows another head than the one a delete made (the person refreshed, or a change of
 * their own was adopted), each remembered item is asked for once at that head with `get_item`:
 * an answer means it is there, and it is forgotten as deleted; `not_found` means it is still
 * gone, and nothing changes. A read that fails otherwise changes nothing either.
 */
import { useQueries } from '@tanstack/react-query';
import { useEffect } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { itemOptions } from '../documents/DocumentView';
import { useExplorerStore } from './store';

export function useForgetRestored(workspaceId: string, head: Revision) {
  const deleted = useExplorerStore((state) => state.deleted);
  const forget = useExplorerStore((state) => state.forgetDeleted);
  const prefix = `${workspaceId}/`;
  // Not at the head the delete itself made: the item is known to be gone there. Nor at the head
  // it was made on, which the shell may still show for a moment after the delete: the item was
  // there, and that says nothing of now.
  const asked = Object.entries(deleted)
    .filter(([key, item]) => key.startsWith(prefix) && item.made !== head && item.at !== head)
    .map(([key]) => key.slice(prefix.length));
  const reads = useQueries({
    queries: asked.map((itemId) => itemOptions(workspaceId, itemId, head)),
  });
  const back = asked.filter((_itemId, place) => reads.at(place)?.isSuccess === true).join(' ');
  useEffect(() => {
    for (const itemId of back.split(' ')) {
      if (itemId.length > 0) forget(workspaceId, itemId);
    }
  }, [back, forget, workspaceId]);
}
