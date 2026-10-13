/**
 * The tree's own changes: a new note, a new folder, a move or rename, a delete, a status word and
 * Archive. Each is one request that the service commits at once (SPEC section 8: "Create, move,
 * delete … remain immediate commits"); none of them makes a draft.
 *
 * Each request carries the revision the workspace shows as its base, so the service refuses it as
 * a `conflict` when someone else has moved the head. A request is repeated under the idempotency
 * key it had while it says the same thing (the same values on the same base), so one whose answer
 * was lost is completed, not carried out twice; changed values, or another base, are a new request
 * with a new key.
 *
 * What each change tells the cache. The revision the service returns becomes the head the shell
 * shows (`adoptHead`), and the checks against the latest head run again. Listings, reads and
 * search are keyed by revision, so the tree, the open panes and the search ask afresh under the
 * new head; nothing cached at an older revision is wrong, so nothing of it is touched. Beyond that:
 * - `create_item` answers the committed document, which is exactly what `get_item` answers at the
 *   revision it made, so it is kept as that answer and the new note opens without a request;
 * - `delete_item` leaves the person's draft of the item with the service (a Snapshot of it then
 *   conflicts as deleted, and it can only be discarded), so the drafts list is not touched; the
 *   page remembers that the person deleted the item, to say so in its open tabs;
 * - `move_item` and `set_lifecycle` change one item, whose identity stays: its tabs go on reading
 *   it, by its id, at the new head. The person's draft of that item follows their own change
 *   (`followOwnChange` in `drafts/saving.ts`): it takes the item's header as the change left it,
 *   and the status word when the change set one, and is saved on the revision the change made, so
 *   its next Snapshot does not conflict with the person's own change. Each of the two resolves
 *   once the draft has followed, and says how the draft then stands.
 */
import type { QueryClient } from '@tanstack/react-query';
import { getItemOptions } from '../../api/generated/@tanstack/react-query.gen';
import {
  createFolder,
  createItem,
  deleteItem,
  moveItem,
  setLifecycle,
} from '../../api/generated/sdk.gen';
import type { ItemSummary, Revision } from '../../api/generated/types.gen';
import { type DraftAfterChange, followOwnChange } from '../drafts/saving';
import { adoptHead, checkHeadsAgain, type HeadChange } from '../explorer/head';
import { useExplorerStore } from '../explorer/store';
import { joinPath } from './names';

/** One request of a change, with the key it is sent under. */
export interface Attempt<T> {
  key: string;
  /** The revision the workspace showed: the request's base. */
  base: Revision;
  values: T;
}

/** The request for these values on this base: the last one again while it says the same. */
export function attemptFor<T>(last: Attempt<T> | null, base: Revision, values: T): Attempt<T> {
  const same =
    last !== null && last.base === base && JSON.stringify(last.values) === JSON.stringify(values);
  return same ? last : { key: crypto.randomUUID(), base, values };
}

export interface NewNote {
  /** The folder it is created in; '' is the top level. */
  folder: string;
  name: string;
  title: string;
  typeName: string;
}

/** The status words a person may set; the contract refuses `other`. */
export const STATUS_WORDS = ['draft', 'stable', 'deprecated'] as const;
export type StatusWord = (typeof STATUS_WORDS)[number];

/** A change of an item's status word, of its archived flag, or of both. */
export interface Lifecycle {
  itemId: string;
  status?: StatusWord;
  archived?: boolean;
}

/** A change of one item, and how the person's draft of it stands after it. */
export type ItemChange = HeadChange & { draft: DraftAfterChange };

/** What is said of the person's draft after their own change of its item; nothing when they have none. */
export const DRAFT_AFTER: Record<DraftAfterChange, string> = {
  none: '',
  follows:
    'Your draft of this item follows the change: it now starts from this version, so its next Snapshot will not ask about it.',
  asks: 'Your draft of this item is kept. It was started on an older version than the one you changed, so its next Snapshot will first ask whether to keep your text.',
  'not-saved':
    'Your draft of this item is kept, but it could not be saved on this version: its own notice says why. Until it is, its next Snapshot may first ask whether to keep your text.',
};

/** The shell shows the revision a change made; the head checks run again. */
function changed(
  client: QueryClient,
  workspaceId: string,
  revision: Revision,
  itemIds: string[],
): HeadChange {
  const previousHead = adoptHead(client, workspaceId, revision);
  checkHeadsAgain(client, workspaceId);
  return { revision, previousHead, itemIds };
}

/** The text a new note starts with: its title as the first heading. */
export const firstText = (title: string) => `# ${title}\n`;

/** Create a note. Resolves with the head change and the note as committed. */
export async function createNote(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<NewNote>,
): Promise<HeadChange & { item: ItemSummary }> {
  const { folder, name, title, typeName } = attempt.values;
  const { data } = await createItem({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      idempotency_key: attempt.key,
      kind: 'note',
      path: joinPath(folder, name),
      title,
      type_name: typeName,
      body: firstText(title),
      properties: {},
    },
    throwOnError: true,
  });
  const item = data.summary;
  client.setQueryData(
    getItemOptions({
      body: {
        workspace_id: workspaceId,
        item_id: item.id,
        at: { kind: 'revision', revision: item.revision },
      },
    }).queryKey,
    data,
  );
  return { ...changed(client, workspaceId, item.revision, [item.id]), item };
}

/** Create a folder (with the index the service keeps in it). */
export async function createNewFolder(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<{ folder: string }>,
): Promise<HeadChange> {
  const { data } = await createFolder({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      idempotency_key: attempt.key,
      folder: attempt.values.folder.normalize('NFC'),
    },
    throwOnError: true,
  });
  return changed(client, workspaceId, data.revision, []);
}

/** Move or rename an item: its path changes, its identity does not. */
export async function moveItemTo(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<{ itemId: string; destination: string }>,
): Promise<ItemChange> {
  const { itemId, destination } = attempt.values;
  const { data } = await moveItem({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      idempotency_key: attempt.key,
      item_id: itemId,
      destination: destination.normalize('NFC'),
    },
    throwOnError: true,
  });
  const change = changed(client, workspaceId, data.revision, [itemId]);
  const draft = await followOwnChange(client, workspaceId, itemId, attempt.base, data.revision, {
    status: false,
  });
  return { ...change, draft };
}

/** Delete an item from the workspace as it is now; its history stays. */
export async function deleteItemNow(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<{ itemId: string; title: string }>,
): Promise<HeadChange> {
  const { itemId, title } = attempt.values;
  const { data } = await deleteItem({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      idempotency_key: attempt.key,
      item_id: itemId,
    },
    throwOnError: true,
  });
  useExplorerStore
    .getState()
    .markDeleted(workspaceId, itemId, { title, at: attempt.base, made: data.revision });
  // No tab is moved to the new head: the item is not there. A tab that cites a revision keeps it.
  return changed(client, workspaceId, data.revision, []);
}

/** Set an item's status word, archive or unarchive it. */
export async function changeLifecycle(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<Lifecycle>,
): Promise<ItemChange> {
  const { itemId, status, archived } = attempt.values;
  const { data } = await setLifecycle({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      idempotency_key: attempt.key,
      item_id: itemId,
      ...(status === undefined ? {} : { status }),
      ...(archived === undefined ? {} : { archived }),
    },
    throwOnError: true,
  });
  const change = changed(client, workspaceId, data.revision, [itemId]);
  const draft = await followOwnChange(client, workspaceId, itemId, attempt.base, data.revision, {
    status: status !== undefined,
  });
  return { ...change, draft };
}
