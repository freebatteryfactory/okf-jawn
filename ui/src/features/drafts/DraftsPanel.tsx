/**
 * The one place that lists every draft the person has in this workspace, and where a Snapshot is
 * taken of the ones they select.
 *
 * The list: the drafts the service holds (`list_drafts`, every page) and any text still only in
 * this page (typed and not saved yet, or whose save failed), each with how it stands. A row opens
 * its item. `list_drafts` answers item ids, so each row reads its item's title from `get_item` at
 * the head the workspace shows; until that answers, the row shows a short identity.
 *
 * This panel asks for the list's further pages by itself, one after another, so the marks in the
 * tree and on the tabs cover every draft. A page that fails stops that, says so here and offers
 * its retry; the marks then cover the pages listed so far.
 *
 * The Snapshot: every listed draft is included unless its box is unticked. A draft whose last
 * Snapshot conflicted is left out until the person resolves it in its own pane, so the others are
 * one press away. A Snapshot has a name; an empty one is not sent and the field says so. While it
 * is taken the button keeps keyboard focus (`aria-disabled`); it is always there, so it never
 * leaves with focus on it. A polite status says what happened, here and after a conflict is
 * resolved in a pane: the name, how many items, and the revision the workspace now shows. When
 * the service finds a conflict it has snapshotted nothing, and the panel says so.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Draft, Revision } from '../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { itemOptions } from '../documents/DocumentView';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { draftsByItem, useDrafts } from './drafts-list';
import {
  type DraftSession,
  type SnapshotConflict,
  saveStateOf,
  sessionKey,
  useDraftSessions,
} from './sessions';
import {
  draftConflictsOf,
  type Snapshotted,
  saySnapshotted,
  takeSnapshot,
  UnsavedSelection,
} from './snapshot';

export interface DraftsPanelProps {
  workspaceId: string;
  /** The head the workspace shows: where item titles are read. */
  head: Revision;
  /** The person may write in this workspace: only then is a Snapshot offered. */
  canWrite: boolean;
  onOpenItem: (itemId: string) => void;
  /** A Snapshot was taken here. */
  onSnapshotted: (snapshotted: Snapshotted) => void;
}

interface Row {
  itemId: string;
  /** The draft the service holds, if it holds one. */
  draft: Draft | null;
  /** The text being edited in this page, if any. */
  session: DraftSession | null;
  /** The Snapshot that did not include it, until the person resolves it. */
  conflict: SnapshotConflict | null;
}

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function DraftsPanel({
  workspaceId,
  head,
  canWrite,
  onOpenItem,
  onSnapshotted,
}: DraftsPanelProps) {
  const client = useQueryClient();
  const heading = useId();
  const nameId = useId();
  const nameErrorId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const nameRef = useRef<HTMLInputElement | null>(null);
  const drafts = useDrafts(workspaceId);
  const sessions = useDraftSessions((state) => state.sessions);
  const conflicts = useDraftSessions((state) => state.conflicts);
  const said = useDraftSessions((state) => state.said[workspaceId]);

  // Every page, one after another; a failed page waits for its retry.
  const { hasNextPage, isFetching, isError, fetchNextPage } = drafts;
  useEffect(() => {
    if (hasNextPage && !isFetching && !isError) void fetchNextPage();
  }, [hasNextPage, isFetching, isError, fetchNextPage]);

  const conflictOf = (itemId: string) => conflicts[sessionKey(workspaceId, itemId)] ?? null;
  const listed = draftsByItem(drafts.data?.pages);
  const rows: Row[] = [...listed.values()].map((draft) => ({
    itemId: draft.item_id,
    draft,
    session: null,
    conflict: conflictOf(draft.item_id),
  }));
  for (const session of Object.values(sessions)) {
    if (session.workspaceId !== workspaceId) continue;
    const row = rows.find((candidate) => candidate.itemId === session.itemId);
    if (row !== undefined) row.session = session;
    else if (saveStateOf(session) !== 'unchanged') {
      rows.push({
        itemId: session.itemId,
        draft: null,
        session,
        conflict: conflictOf(session.itemId),
      });
    }
  }

  // The list failed with nothing listed, or with some listed: then those stay and the rest waits.
  const nothingListed = drafts.data === undefined;
  const firstRow = () => listRef.current?.querySelector<HTMLElement>('button');
  const listFailure = useRetry({
    request: workspaceId,
    failed: drafts.isError && nothingListed,
    error: drafts.error,
    run: () => drafts.refetch(),
    loaded: () => firstRow() ?? headingRef.current,
  });
  const moreFailure = useRetry({
    request: workspaceId,
    failed: drafts.isError && !nothingListed,
    error: drafts.error,
    run: () => (drafts.isFetchNextPageError ? drafts.fetchNextPage() : drafts.refetch()),
    loaded: () => firstRow() ?? headingRef.current,
  });

  // A draft's row leaves when its draft is snapshotted or discarded. Keyboard focus that was in
  // it goes to the row now in its place, else the one before, else the panel's heading.
  const heldRow = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    const left = heldRow.current;
    if (!takeHeld(heldRow) || left === null) return;
    const at = Number(left.getAttribute('data-row') ?? 0);
    const remaining = listRef.current?.querySelectorAll<HTMLElement>(':scope > li') ?? [];
    const next = remaining[at] ?? remaining[at - 1];
    (next?.querySelector<HTMLElement>('button') ?? headingRef.current)?.focus();
  });

  // The Snapshot: which drafts, under which name.
  const [left, setLeft] = useState<ReadonlySet<string>>(new Set());
  const [name, setName] = useState('');
  const [nameMissing, setNameMissing] = useState(false);
  const selected = rows
    .filter((row) => row.conflict === null && !left.has(row.itemId))
    .map((row) => row.itemId);
  // Which request it is, and so its key, is `takeSnapshot`'s to say: here is only what was asked.
  const snapshot = useMutation({
    mutationFn: (asked: { itemIds: string[]; message: string }) =>
      takeSnapshot(client, workspaceId, asked.itemIds, asked.message),
    onSuccess: (snapshotted, asked) => {
      setName('');
      saySnapshotted(workspaceId, asked.message, snapshotted);
      onSnapshotted(snapshotted);
    },
  });
  const start = () => {
    if (snapshot.isPending || selected.length === 0) return;
    const message = name.trim();
    if (message.length === 0) {
      setNameMissing(true);
      nameRef.current?.focus();
      return;
    }
    setNameMissing(false);
    snapshot.mutate({ itemIds: selected, message });
  };
  const conflicted = snapshot.isError ? draftConflictsOf(snapshot.error) : null;
  const unsaved =
    snapshot.isError && snapshot.error instanceof UnsavedSelection ? snapshot.error.itemIds : null;
  const asked = snapshot.variables?.itemIds.length ?? 0;
  const snapshotFailure = useRetry({
    request: workspaceId,
    failed: snapshot.isError && conflicted === null && unsaved === null,
    error: snapshot.error,
    // The retry asks for what was asked; it is the same request while nothing was saved since.
    run: () =>
      snapshot.variables === undefined
        ? Promise.resolve()
        : snapshot.mutateAsync(snapshot.variables),
    loaded: () => headingRef.current,
  });

  return (
    <section className="drafts-panel" aria-labelledby={heading}>
      <h2 id={heading} ref={headingRef} tabIndex={-1} className="panel-heading">
        Drafts{drafts.isSuccess || rows.length > 0 ? ` (${rows.length})` : ''}
      </h2>
      {drafts.isPending && !listFailure.retrying && <p role="status">Listing your drafts…</p>}
      {listFailure.shown && (
        <FailureNotice
          context="Listing your drafts"
          error={listFailure.error}
          retry={listFailure}
          workspaceId={workspaceId}
        />
      )}
      {drafts.isSuccess && rows.length === 0 && (
        <p className="meta">You have no drafts in this workspace.</p>
      )}
      {rows.length > 0 && (
        <ul ref={listRef} className="draft-list">
          {rows.map((row, index) => (
            <DraftRow
              key={row.itemId}
              index={index}
              held={heldRow}
              workspaceId={workspaceId}
              head={head}
              row={row}
              selectable={canWrite}
              selected={row.conflict === null && !left.has(row.itemId)}
              onSelect={(include) =>
                setLeft((current) => {
                  const next = new Set(current);
                  if (include) next.delete(row.itemId);
                  else next.add(row.itemId);
                  return next;
                })
              }
              onOpenItem={onOpenItem}
            />
          ))}
        </ul>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Listing the rest of your drafts"
          error={moreFailure.error}
          retry={moreFailure}
          workspaceId={workspaceId}
        />
      )}
      {canWrite && (
        <form
          className="snapshot-form"
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            start();
          }}
        >
          <label htmlFor={nameId}>Snapshot name</label>
          <input
            id={nameId}
            ref={nameRef}
            type="text"
            value={name}
            aria-invalid={nameMissing}
            {...(nameMissing ? { 'aria-describedby': nameErrorId } : {})}
            onChange={(event) => {
              setName(event.target.value);
              if (event.target.value.trim().length > 0) setNameMissing(false);
            }}
          />
          {nameMissing && (
            <p id={nameErrorId} className="field-error">
              Give the Snapshot a name: it is how this version is found again.
            </p>
          )}
          {/* aria-disabled rather than disabled: the button keeps focus while the Snapshot is
              taken, and when the last draft has gone. */}
          <button type="submit" aria-disabled={snapshot.isPending || selected.length === 0}>
            {snapshot.isPending
              ? 'Taking the Snapshot…'
              : `Snapshot ${count(selected.length, 'draft', 'drafts')}`}
          </button>
        </form>
      )}
      {/* Present from the start, so what a Snapshot did is announced. */}
      <p role="status" className="meta snapshot-said">
        {snapshot.isPending && !snapshotFailure.retrying
          ? 'Taking the Snapshot…'
          : (said?.text ?? '')}
      </p>
      {keyed(said?.warnings ?? [], (warning) => warning).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning}
        </p>
      ))}
      {unsaved !== null && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>Nothing was snapshotted.</strong>
          </p>
          <p>
            {count(unsaved.length, 'selected item has', 'selected items have')} text that is not
            saved, and a Snapshot is taken of saved drafts. Open it: its own notice says why and has
            the retry.
          </p>
        </div>
      )}
      {conflicted !== null && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>Nothing was snapshotted.</strong>
          </p>
          <p>
            {conflicted.length} of the {count(asked, 'selected item', 'selected items')} changed
            since your draft of it was started, and the service takes a Snapshot of all the selected
            items or of none. Each one is marked in the list and left out until you open it and keep
            your text or take the newer version.
            {selected.length > 0
              ? ` The other ${count(selected.length, 'draft is', 'drafts are')} still selected: press Snapshot to take ${selected.length === 1 ? 'it' : 'them'} now.`
              : ''}
          </p>
        </div>
      )}
      {snapshotFailure.shown && (
        <FailureNotice
          context="Taking the Snapshot"
          error={snapshotFailure.error}
          retry={snapshotFailure}
          workspaceId={workspaceId}
        />
      )}
    </section>
  );
}

/** How a draft stands, in the list: the session's state when this page edits it. */
function Standing({ row }: { row: Row }) {
  if (row.conflict !== null) return <>changed since your draft: open it to resolve</>;
  const state = row.session === null ? 'saved' : saveStateOf(row.session);
  if (state === 'saving') return <>saving…</>;
  if (state === 'unsaved') return <>unsaved changes</>;
  if (state === 'failed' || state === 'too-long') return <>not saved</>;
  const saved = row.session?.confirmed?.draft ?? row.draft;
  if (saved === null) return <>not saved</>;
  return (
    <>
      saved <Instant value={saved.saved_at} />
    </>
  );
}

function DraftRow({
  index,
  held,
  workspaceId,
  head,
  row,
  selectable,
  selected,
  onSelect,
  onOpenItem,
}: {
  /** The row's place in the list, and where a row that leaves with focus in it is noted. */
  index: number;
  held: HeldFocus;
  workspaceId: string;
  head: Revision;
  row: Row;
  selectable: boolean;
  selected: boolean;
  onSelect: (include: boolean) => void;
  onOpenItem: (itemId: string) => void;
}) {
  const item = useQuery(itemOptions(workspaceId, row.itemId, head));
  const summary = item.data?.summary;
  const name = summary ? summary.title || summary.path : `Item ${row.itemId.slice(0, 8)}`;
  const conflicting = row.conflict !== null;
  const [, rowRef] = useWatchedPart<HTMLLIElement>(held);
  return (
    <li ref={rowRef} data-row={index}>
      {selectable && (
        // aria-disabled rather than disabled: a conflict can arrive while the box has focus. A
        // conflicting draft's box shows unticked whatever is pressed, until it is resolved.
        <input
          type="checkbox"
          checked={selected}
          aria-disabled={conflicting}
          aria-label={`Include ${name} in the Snapshot`}
          onChange={(event) => onSelect(event.target.checked)}
        />
      )}{' '}
      <button type="button" className="link-button" onClick={() => onOpenItem(row.itemId)}>
        {name}
      </button>{' '}
      <span className="meta">
        <Standing row={row} />
      </span>
    </li>
  );
}
