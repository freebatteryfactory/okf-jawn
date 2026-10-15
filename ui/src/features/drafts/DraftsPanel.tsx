/**
 * The one place that lists every draft the person has in this workspace, and where a Snapshot is
 * taken of the ones they select.
 *
 * The list: the drafts the service holds (`list_drafts`, every page) and any text still only in
 * this page (typed and not saved yet, or whose save failed), each with how it stands. A row opens
 * its item. Each listed draft comes with its item's summary as `list_drafts` read it (ruling 1):
 * its title and its kind, at the head of that listing. Nothing here reads an item itself. Text only
 * in this page is of an item this page edits as a Note (`DraftSession`), whose title is what this
 * page already holds of it; until it holds one, the row shows a short identity.
 *
 * A draft of a Source or a View can only have been made elsewhere: this workspace edits Notes as
 * drafts. The service takes no such draft into a Snapshot, so its row says its kind and why, its
 * box is unticked and cannot be ticked, and it offers "Discard draft", which asks first. A listed
 * draft whose item the listing did not find (it was deleted, say) says so, is not ticked and is
 * not in the Snapshot or its count, and offers "Discard draft" too, so it can still be got rid of.
 *
 * This panel asks for the list's further pages by itself, one after another, so the marks in the
 * tree and on the tabs cover every draft. A page that fails stops that, says so here and offers
 * its retry; the marks then cover the pages listed so far.
 *
 * The Snapshot: every listed draft is included unless its box is unticked. A draft whose last
 * Snapshot conflicted is left out until the person resolves it in its own pane, so the others are
 * one press away. An item whose properties form holds input back says so in its row, before a
 * Snapshot is taken: the Snapshot takes its saved draft, without that input; when nothing of the
 * item is saved, its box cannot be ticked. A Snapshot has a name; an empty one is not sent and
 * the field says so. While it
 * is taken the button keeps keyboard focus (`aria-disabled`); it is always there, so it never
 * leaves with focus on it. A polite status says what happened, here and after a conflict is
 * resolved in a pane: the name, how many items, and the revision the workspace now shows. When
 * the service finds a conflict it has snapshotted nothing, and the panel says so.
 */
import { useMutation, useQueries, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Draft, ItemSummary, Revision } from '../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { itemOptions } from '../documents/DocumentView';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { useDeleted } from '../explorer/store';
import { rowsByItem, useDrafts } from './drafts-list';
import { discardSession } from './saving';
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
  /**
   * The item: as `list_drafts` read it beside the draft, or, for text only in this page, as this
   * page holds it; undefined when neither tells.
   */
  summary: ItemSummary | undefined;
  /** `list_drafts` read the item, and it was not there. */
  missing: boolean;
}

/** The kinds of item that are not edited as a draft here, as the person is told. */
const OTHER_KIND = { source: 'Source', view: 'View' } as const;

/**
 * What kind of item the draft is of: as the listing read it; for text only in this page, a Note,
 * unless a read of the item since said otherwise (`DraftSession.refused`). Not known for a draft
 * whose item the listing did not find.
 */
function kindOf(row: Row): ItemSummary['kind'] | undefined {
  if (row.summary !== undefined) return row.summary.kind;
  if (row.draft === null && row.session !== null) return row.session.refused ?? 'note';
  return undefined;
}

/**
 * The draft is of a Source or a View: one made elsewhere, since this workspace edits only Notes
 * as drafts. The service takes no such draft into a Snapshot.
 */
const otherKind = (row: Row) => {
  const kind = kindOf(row);
  return kind === undefined || kind === 'note' ? null : OTHER_KIND[kind];
};

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The draft of a row as the service last confirmed or listed it; null when it holds none. */
const savedDraft = (row: Row) => row.session?.confirmed?.draft ?? row.draft;

/**
 * Nothing of this item is saved, and nothing of it will be by a Snapshot's own save: all it has
 * is input of its properties form that is held back. There is nothing to snapshot.
 */
const nothingSaved = (row: Row) =>
  row.session !== null && saveStateOf(row.session) === 'held-back' && savedDraft(row) === null;

/** Whether the row's item is a Note is not known: the listing did not find the item. */
const kindUnknown = (row: Row) => kindOf(row) === undefined;

/**
 * The draft is offered for a Snapshot: its item is known to be a Note, it has no open conflict,
 * and something of it is saved.
 */
const offered = (row: Row) =>
  row.conflict === null && !nothingSaved(row) && !kindUnknown(row) && otherKind(row) === null;

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
  const listed = rowsByItem(drafts.data?.pages);
  const gathered: Row[] = [...listed.values()].map(({ draft, item }) => ({
    itemId: draft.item_id,
    draft,
    session: null,
    conflict: conflictOf(draft.item_id),
    summary: item ?? undefined,
    missing: item === null,
  }));
  for (const session of Object.values(sessions)) {
    if (session.workspaceId !== workspaceId) continue;
    const row = gathered.find((candidate) => candidate.itemId === session.itemId);
    if (row !== undefined) row.session = session;
    else if (saveStateOf(session) !== 'unchanged') {
      gathered.push({
        itemId: session.itemId,
        draft: null,
        session,
        conflict: conflictOf(session.itemId),
        summary: undefined,
        missing: false,
      });
    }
  }
  // Text only in this page: its title is what this page already holds of the item at the head
  // shown, from its pane. Nothing is asked for here.
  const held = useQueries({
    queries: gathered.map((row) => ({
      ...itemOptions(workspaceId, row.itemId, head),
      enabled: false,
    })),
  });
  const rows: Row[] = gathered.map((row, index) =>
    row.draft === null ? { ...row, summary: held.at(index)?.data?.summary } : row,
  );

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
    .filter((row) => offered(row) && !left.has(row.itemId))
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
              row={row}
              selectable={canWrite}
              selected={offered(row) && !left.has(row.itemId)}
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
            saved, or a draft that is no longer saved, and a Snapshot is taken of saved drafts. Open
            it: its own notice says why, and has the retry when there is one.
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
  const saved = savedDraft(row);
  if (state === 'held-back') {
    // A Snapshot takes the saved draft, and the held-back input is not in it: said before it is
    // taken, where the draft is ticked.
    return saved === null ? (
      <>unsaved: what you changed in its properties is held back, so there is nothing to snapshot</>
    ) : (
      <>
        saved <Instant value={saved.saved_at} />; unsaved: what you changed in its properties is
        held back, and a Snapshot will not have it
      </>
    );
  }
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
  row: Row;
  selectable: boolean;
  selected: boolean;
  onSelect: (include: boolean) => void;
  onOpenItem: (itemId: string) => void;
}) {
  // An item the person deleted in this page is not at the head: its draft is named by the title
  // the item had, as its tab is.
  const deleted = useDeleted(workspaceId, row.itemId);
  const summary = row.summary;
  const name =
    deleted !== undefined
      ? `${deleted} (deleted)`
      : summary
        ? summary.title || summary.path
        : `Item ${row.itemId.slice(0, 8)}`;
  const kind = otherKind(row);
  const [, rowRef] = useWatchedPart<HTMLLIElement>(held);
  const openRef = useRef<HTMLButtonElement | null>(null);
  // The listing did not find the row's item (and this page did not delete it).
  const notFound = deleted === undefined && row.missing && kindUnknown(row);
  // The discard is offered where the draft cannot be snapshotted from here. It leaves when a later
  // listing finds the item and it is a Note; keyboard focus that was in it goes to the row's item.
  const heldDiscard = useRef<HTMLElement | null>(null);
  const [, discardRef] = useWatchedPart<HTMLDivElement>(heldDiscard);
  useLayoutEffect(() => {
    if (takeHeld(heldDiscard)) openRef.current?.focus();
  });
  return (
    <li ref={rowRef} data-row={index}>
      {selectable && (
        // aria-disabled rather than disabled: a conflict can arrive while the box has focus. The
        // box of a draft that is not offered (a conflict, or nothing of it saved) shows unticked
        // whatever is pressed, until that is over.
        <input
          type="checkbox"
          checked={selected}
          aria-disabled={!offered(row)}
          aria-label={`Include ${name} in the Snapshot`}
          onChange={(event) => onSelect(event.target.checked)}
        />
      )}{' '}
      <button
        type="button"
        ref={openRef}
        className="link-button"
        onClick={() => onOpenItem(row.itemId)}
      >
        {name}
      </button>{' '}
      <span className="meta">
        {kind === null ? null : `${kind} · `}
        {deleted !== undefined ? (
          <>its item was deleted: open it to copy from the draft or discard it</>
        ) : notFound ? (
          <>its item was not found</>
        ) : (
          <Standing row={row} />
        )}
      </span>
      {kind !== null && (
        <p className="meta">
          A draft of a {kind} cannot be snapshotted: only a Note is edited as a draft.
          {selectable ? ' It can be discarded here.' : ''}
        </p>
      )}
      {notFound && (
        <p className="meta">
          This draft’s item was not in the workspace when your drafts were listed, so it cannot be
          snapshotted.
          {selectable ? ' It can be discarded here.' : ''}
        </p>
      )}
      {selectable && (kind !== null || notFound) && (
        <div ref={discardRef}>
          <RowDiscard
            workspaceId={workspaceId}
            itemId={row.itemId}
            name={name}
            committed={kind ?? 'item'}
          />
        </div>
      )}
    </li>
  );
}

/**
 * The way to discard a draft from its row, for a draft that cannot be snapshotted from here (of a
 * Source or a View, or of an item that could not be read). It asks first, in place ("Keep the
 * draft" takes keyboard focus, and choosing it gives focus back to "Discard draft"). The discard
 * is the item's own (`saving.ts`): the service is asked unless this page knows it holds no draft
 * of the item. When it is done the row leaves, and the panel says where keyboard focus goes.
 */
function RowDiscard({
  workspaceId,
  itemId,
  name,
  committed,
}: {
  workspaceId: string;
  itemId: string;
  name: string;
  /** What stays as it is, as the question says it: the kind of the item, or "item". */
  committed: (typeof OTHER_KIND)[keyof typeof OTHER_KIND] | 'item';
}) {
  const client = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const discardRef = useRef<HTMLButtonElement | null>(null);
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const focusAfterConfirm = useRef<'keep' | 'discard' | null>(null);
  useEffect(() => {
    const target = focusAfterConfirm.current;
    focusAfterConfirm.current = null;
    if (target === 'keep') keepRef.current?.focus();
    else if (target === 'discard') discardRef.current?.focus();
  });
  const discard = useMutation({
    mutationFn: () => discardSession(client, workspaceId, itemId),
    onSuccess: () =>
      useDraftSessions.getState().say(workspaceId, `Your draft of ${name} was discarded.`),
  });
  const discardFailure = useRetry({
    request: `${workspaceId}/${itemId}`,
    failed: discard.isError,
    error: discard.error,
    run: () => discard.mutateAsync(),
    loaded: () => null,
  });
  return (
    <>
      {confirming ? (
        <fieldset className="confirm">
          <legend>Discard the draft of {name}?</legend>
          <p>The draft will be removed. The committed {committed} stays as it is.</p>
          <button
            type="button"
            ref={keepRef}
            onClick={() => {
              focusAfterConfirm.current = 'discard';
              setConfirming(false);
            }}
          >
            Keep the draft
          </button>{' '}
          {/* aria-disabled rather than disabled: the button keeps focus while it works. */}
          <button
            type="button"
            aria-disabled={discard.isPending}
            onClick={() => {
              if (!discard.isPending) discard.mutate();
            }}
          >
            {discard.isPending ? 'Discarding…' : 'Discard the draft'}
          </button>
        </fieldset>
      ) : (
        <p className="draft-actions">
          <button
            type="button"
            ref={discardRef}
            aria-label={`Discard draft of ${name}`}
            onClick={() => {
              focusAfterConfirm.current = 'keep';
              setConfirming(true);
            }}
          >
            Discard draft
          </button>
        </p>
      )}
      {discardFailure.shown && (
        <FailureNotice
          context={`Discarding the draft of ${name}`}
          error={discardFailure.error}
          retry={discardFailure}
          workspaceId={workspaceId}
        />
      )}
    </>
  );
}
