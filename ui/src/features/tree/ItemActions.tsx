/**
 * The changes of the item a pane shows: rename or move, delete, its status word, and Archive.
 *
 * The status word is one of draft, stable and deprecated; the contract refuses `other`, so it is
 * not offered: an item whose file holds another word shows it, and can only be given one of the
 * three. Archive hides the item from the folders and keeps its history; the pane goes on showing
 * it, with Unarchive in Archive's place. Neither is a claim about the content.
 *
 * Every control here stays on the page while a change is made and while the pane reads the item
 * again at the head the change made (`aria-disabled`, never `disabled`), so none of them drops
 * keyboard focus: after a rename or a move focus is back on "Rename or move", and the status
 * choice and Archive keep it. After a delete these controls go with the item; the pane then says
 * the item was deleted and puts focus on "Close this tab" (`DocumentView`). A polite status says
 * what was done.
 */
import { useId, useLayoutEffect, useRef, useState } from 'react';
import type { ItemSummary } from '../../api/generated/types.gen';
import type { DraftMark } from '../drafts/marks';
import type { HeadChange } from '../explorer/head';
import { ChangeFailure, DeleteQuestion, MoveForm, useChange } from './ChangeForms';
import { changeLifecycle, type Lifecycle, STATUS_WORDS, type StatusWord } from './changes';

export interface ItemActionsProps {
  /** The item as the pane last read it at the head the workspace shows. */
  item: ItemSummary;
  /**
   * How the person's draft of the item stands, asked when it is needed (a form opens, a change is
   * made): telling it compares the whole text, which is not done on every render of the pane.
   */
  mark: () => DraftMark | null;
  /** The pane is reading the item again (the head moved): nothing is changed meanwhile. */
  busy: boolean;
}

const DRAFT_FOLLOWS =
  'You have a draft of this item: its next Snapshot will first ask whether to keep your text, because the item changed since the draft was started.';

const isStatusWord = (word: string): word is StatusWord =>
  STATUS_WORDS.some((known) => known === word);

export function ItemActions({ item, mark, busy }: ItemActionsProps) {
  const statusId = useId();
  const [form, setForm] = useState<'move' | 'delete' | null>(null);
  const [said, setSaid] = useState('');
  const moveRef = useRef<HTMLButtonElement | null>(null);
  const deleteRef = useRef<HTMLButtonElement | null>(null);
  const statusRef = useRef<HTMLSelectElement | null>(null);
  const focusNext = useRef<'move' | 'delete' | null>(null);
  useLayoutEffect(() => {
    const next = focusNext.current;
    if (next === null || form !== null) return;
    focusNext.current = null;
    (next === 'move' ? moveRef : deleteRef).current?.focus();
  });
  const close = (to: 'move' | 'delete') => {
    focusNext.current = to;
    setForm(null);
  };
  const life = useChange<Lifecycle, HeadChange>(
    changeLifecycle,
    (_result, values) => {
      const done =
        values.archived === undefined
          ? `The status is now ${values.status ?? item.status}.`
          : values.archived
            ? 'Archived. It no longer shows in the folders; its history is kept. Unarchive brings it back.'
            : 'Unarchived. It shows in the folders again.';
      // The item changed under the person's draft, as the service will find at its Snapshot.
      setSaid(mark() === null ? done : `${done} ${DRAFT_FOLLOWS}`);
    },
    statusRef,
  );
  const unavailable = busy || life.pending;
  const title = item.title || item.path;

  return (
    <div className="item-actions">
      <p className="document-actions">
        <button
          type="button"
          ref={moveRef}
          aria-disabled={busy}
          onClick={() => {
            if (!busy) setForm('move');
          }}
        >
          Rename or move
        </button>{' '}
        <button
          type="button"
          ref={deleteRef}
          aria-disabled={busy}
          onClick={() => {
            if (!busy) setForm('delete');
          }}
        >
          Delete
        </button>{' '}
        <label htmlFor={statusId}>Status</label>{' '}
        {/* A select has no aria-disabled of its own in every browser, so a choice made while a
            change is in flight is simply not sent: the select shows the item's status again. */}
        <select
          id={statusId}
          ref={statusRef}
          value={item.status}
          aria-disabled={unavailable}
          onChange={(event) => {
            const next = event.target.value;
            if (unavailable || !isStatusWord(next)) return;
            setSaid('');
            life.start({ itemId: item.id, status: next });
          }}
        >
          {item.status === 'other' && (
            <option value="other" disabled>
              another word (set in the file)
            </option>
          )}
          {STATUS_WORDS.map((word) => (
            <option key={word} value={word}>
              {word}
            </option>
          ))}
        </select>{' '}
        <button
          type="button"
          aria-disabled={unavailable}
          onClick={() => {
            if (unavailable) return;
            setSaid('');
            life.start({ itemId: item.id, archived: !item.archived });
          }}
        >
          {item.archived ? 'Unarchive' : 'Archive'}
        </button>
      </p>
      {/* Present from the start, so what a change did is announced. */}
      <p role="status" className="meta">
        {life.pending ? 'Changing…' : said}
      </p>
      <ChangeFailure context={`Changing “${title}”`} change={life} />
      {form === 'move' && (
        <MoveForm
          item={item}
          mark={mark()}
          onCancel={() => close('move')}
          onDone={(_moved, destination) => {
            setSaid(`“${title}” is now at ${destination}. This tab stays on it.`);
            close('move');
          }}
        />
      )}
      {form === 'delete' && (
        // After the delete these controls leave with the item; the pane places focus.
        <DeleteQuestion
          item={item}
          mark={mark()}
          onCancel={() => close('delete')}
          onDone={() => {}}
        />
      )}
    </div>
  );
}
