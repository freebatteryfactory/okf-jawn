/**
 * The folder tree's own changes: a bar of four buttons above the tree, and the form each opens
 * in place under it.
 *
 * "New note" and "New folder" start in the folder of the tree's current entry (the entry keyboard
 * focus was last on), or at the top level. "Rename or move" and "Delete" act on the current entry
 * when it is an item, and say what they act on in their names. On a row of the tree, F2 opens
 * "Rename or move" and Delete opens the question before a delete. A folder has no rename, move or
 * delete: the service has no operation for one, and the bar says so when asked.
 *
 * The bar's buttons are always there (`aria-disabled` when they have nothing to act on), so one
 * of them can hold keyboard focus when a form closes: Cancel and Escape give focus back to what
 * opened the form (the button, or the row); after a change focus rests on the button for the
 * moment it takes the tree to list the new head, and then goes to the row of what was created or
 * moved, or to the entry now at the place of what was deleted (`TreeReveal`; the tree moves it
 * only while it is still on the bar). A new note opens in its editor, with focus in its text: the
 * next thing a person does with an empty note is write in it. A polite status says what was done.
 */
import { type RefObject, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { ItemSummary } from '../../api/generated/types.gen';
import type { DraftMark } from '../drafts/marks';
import { useExplorerStore } from '../explorer/store';
import { DeleteQuestion, MoveForm, NewEntryForm, useSaid } from './ChangeForms';
import { useWorkspaceChanges } from './context';

/** The tree's current entry, as far as a change needs it. */
export type TreeTarget =
  | { kind: 'folder'; folder: string }
  | { kind: 'item'; item: ItemSummary; folder: string; mark: DraftMark | null; index: number };

type What = 'note' | 'folder' | 'move' | 'delete';

/** The form that is open, and what opened it. */
export interface OpenForm {
  what: What;
  /** The folder a new entry starts in. */
  folder: string;
  /** The item a move or a delete is about. */
  item: (TreeTarget & { kind: 'item' }) | null;
  /** Opened from a row of the tree (F2, Delete) rather than from the bar. */
  fromRow: boolean;
}

export interface TreeActionsProps {
  target: TreeTarget | null;
  form: OpenForm | null;
  onForm: (form: OpenForm | null) => void;
  /** Give keyboard focus back to the tree's current row. */
  onReturnToRow: () => void;
  onOpenItem: (itemId: string) => void;
  barRef: RefObject<HTMLDivElement | null>;
}

export const NO_FOLDER_CHANGES =
  'A folder cannot be renamed, moved or deleted here: the service has no operation for it. Rename, move or delete its items one by one.';
const CHOOSE_FIRST = 'Choose an item in the folders first.';

export function TreeActions({
  target,
  form,
  onForm,
  onReturnToRow,
  onOpenItem,
  barRef,
}: TreeActionsProps) {
  const { workspaceId, head } = useWorkspaceChanges();
  const revealInTree = useExplorerStore((state) => state.revealInTree);
  const requestEdit = useExplorerStore((state) => state.requestEdit);
  const [said, say] = useSaid();
  const buttons = useRef<Partial<Record<What, HTMLButtonElement | null>>>({});
  // Where focus goes in the commit that closes a form: a button of the bar, or the tree's row.
  const focusNext = useRef<What | 'row' | null>(null);
  useLayoutEffect(() => {
    const next = focusNext.current;
    if (next === null || form !== null) return;
    focusNext.current = null;
    if (next === 'row') onReturnToRow();
    else buttons.current[next]?.focus();
  });

  // A new note is opened once the shell shows the head that holds it: opened a moment sooner,
  // its pane would ask for it at the head before, where it does not exist.
  const [created, setCreated] = useState<ItemSummary | null>(null);
  useEffect(() => {
    if (created === null || created.revision !== head) return;
    setCreated(null);
    requestEdit({ workspaceId, itemId: created.id });
    onOpenItem(created.id);
  }, [created, head, requestEdit, workspaceId, onOpenItem]);

  const item = target?.kind === 'item' ? target : null;
  const folder = target === null ? '' : target.folder;
  const title = item === null ? '' : item.item.title || item.item.path;
  const open = (what: What) => {
    say('');
    if (what === 'move' || what === 'delete') {
      if (item === null) {
        say(target?.kind === 'folder' ? NO_FOLDER_CHANGES : CHOOSE_FIRST);
        return;
      }
      onForm({ what, folder, item, fromRow: false });
    } else onForm({ what, folder, item: null, fromRow: false });
  };
  const close = (to: What | 'row') => {
    focusNext.current = to;
    onForm(null);
  };
  const cancel = () => close(form?.fromRow ? 'row' : (form?.what ?? 'note'));
  const named = (label: string) =>
    item === null ? `${label} (choose an item in the folders first)` : `${label} ${title}`;

  return (
    <div className="tree-actions">
      {/* aria-disabled, never disabled: a button of this bar can hold focus when its form closes. */}
      <div ref={barRef} role="toolbar" aria-label="Change the folders" className="tree-bar">
        <button
          type="button"
          ref={(element) => {
            buttons.current.note = element;
          }}
          onClick={() => open('note')}
        >
          New note
        </button>
        <button
          type="button"
          ref={(element) => {
            buttons.current.folder = element;
          }}
          onClick={() => open('folder')}
        >
          New folder
        </button>
        <button
          type="button"
          ref={(element) => {
            buttons.current.move = element;
          }}
          aria-label={named('Rename or move')}
          aria-disabled={item === null}
          onClick={() => open('move')}
        >
          Rename or move
        </button>
        <button
          type="button"
          ref={(element) => {
            buttons.current.delete = element;
          }}
          aria-label={named('Delete')}
          aria-disabled={item === null}
          onClick={() => open('delete')}
        >
          Delete
        </button>
      </div>
      {/* Present from the start, so what a change did is announced. */}
      <p role="status" className="meta tree-said">
        {said}
      </p>
      {form !== null && (form.what === 'note' || form.what === 'folder') && (
        <NewEntryForm
          key={form.what}
          kind={form.what}
          folder={form.folder}
          onCancel={cancel}
          onDone={(created) => {
            if ('item' in created) {
              const name = created.item.title || created.item.path;
              say(`Created the note “${name}” at ${created.item.path}. It is open for writing.`);
              setCreated(created.item);
              close('note');
            } else {
              say(`Created the folder ${created.folder}.`);
              revealInTree({
                workspaceId,
                revision: created.revision,
                kind: 'folder',
                folder: created.folder,
              });
              close('folder');
            }
          }}
        />
      )}
      {form !== null && form.what === 'move' && form.item !== null && (
        <MoveForm
          item={form.item.item}
          mark={form.item.mark}
          onCancel={cancel}
          onDone={(change, destination) => {
            const moved = form.item;
            if (moved === null) return;
            say(`“${moved.item.title || moved.item.path}” is now at ${destination}.`, change.draft);
            revealInTree({
              workspaceId,
              revision: change.revision,
              kind: 'item',
              itemId: moved.item.id,
              path: destination,
            });
            close('move');
          }}
        />
      )}
      {form !== null && form.what === 'delete' && form.item !== null && (
        <DeleteQuestion
          item={form.item.item}
          mark={form.item.mark}
          onCancel={cancel}
          onDone={(change) => {
            const removed = form.item;
            if (removed === null) return;
            say(`Deleted “${removed.item.title || removed.item.path}”. Its history is kept.`);
            revealInTree({
              workspaceId,
              revision: change.revision,
              kind: 'place',
              index: removed.index,
            });
            close('delete');
          }}
        />
      )}
    </div>
  );
}
