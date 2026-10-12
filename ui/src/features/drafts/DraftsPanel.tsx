/**
 * The one place that lists every draft the person has in this workspace: the drafts the service
 * holds (`list_drafts`, every page) and any text still only in this page (typed and not saved
 * yet, or whose save failed), each with how it stands. A row opens its item.
 *
 * `list_drafts` answers item ids, so each row reads its item's title from `get_item` at the head
 * the workspace shows; until that answers, the row shows a short identity.
 *
 * This panel asks for the list's further pages by itself, one after another, so the marks in the
 * tree and on the tabs cover every draft. A page that fails stops that, says so here and offers
 * its retry; the marks then cover the pages listed so far.
 */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useId, useRef } from 'react';
import type { Draft, Revision } from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { itemOptions } from '../documents/DocumentView';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { draftsByItem, useDrafts } from './drafts-list';
import { type DraftSession, saveStateOf, useDraftSessions } from './sessions';

export interface DraftsPanelProps {
  workspaceId: string;
  /** The head the workspace shows: where item titles are read. */
  head: Revision;
  onOpenItem: (itemId: string) => void;
}

interface Row {
  itemId: string;
  /** The draft the service holds, if it holds one. */
  draft: Draft | null;
  /** The text being edited in this page, if any. */
  session: DraftSession | null;
}

export function DraftsPanel({ workspaceId, head, onOpenItem }: DraftsPanelProps) {
  const heading = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const drafts = useDrafts(workspaceId);
  const sessions = useDraftSessions((state) => state.sessions);

  // Every page, one after another; a failed page waits for its retry.
  const { hasNextPage, isFetching, isFetchNextPageError, fetchNextPage } = drafts;
  useEffect(() => {
    if (hasNextPage && !isFetching && !isFetchNextPageError) void fetchNextPage();
  }, [hasNextPage, isFetching, isFetchNextPageError, fetchNextPage]);

  const listed = draftsByItem(drafts.data?.pages);
  const rows: Row[] = [...listed.values()].map((draft) => ({
    itemId: draft.item_id,
    draft,
    session: null,
  }));
  for (const session of Object.values(sessions)) {
    if (session.workspaceId !== workspaceId) continue;
    const row = rows.find((candidate) => candidate.itemId === session.itemId);
    if (row !== undefined) row.session = session;
    else if (saveStateOf(session) !== 'unchanged') {
      rows.push({ itemId: session.itemId, draft: null, session });
    }
  }

  const firstRow = () => listRef.current?.querySelector<HTMLElement>('button');
  const listFailure = useRetry({
    request: workspaceId,
    failed: drafts.isError && !drafts.isFetchNextPageError,
    error: drafts.error,
    run: () => drafts.refetch(),
    loaded: () => firstRow() ?? headingRef.current,
  });
  const moreFailure = useRetry({
    request: workspaceId,
    failed: drafts.isFetchNextPageError,
    error: drafts.error,
    run: () => drafts.fetchNextPage(),
    loaded: () => firstRow() ?? headingRef.current,
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
          {rows.map((row) => (
            <DraftRow
              key={row.itemId}
              workspaceId={workspaceId}
              head={head}
              row={row}
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
    </section>
  );
}

/** How a draft stands, in the list: the session's state when this page edits it. */
function Standing({ row }: { row: Row }) {
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
  workspaceId,
  head,
  row,
  onOpenItem,
}: {
  workspaceId: string;
  head: Revision;
  row: Row;
  onOpenItem: (itemId: string) => void;
}) {
  const item = useQuery(itemOptions(workspaceId, row.itemId, head));
  const summary = item.data?.summary;
  const name = summary ? summary.title || summary.path : `Item ${row.itemId.slice(0, 8)}`;
  return (
    <li>
      <button type="button" className="link-button" onClick={() => onOpenItem(row.itemId)}>
        {name}
      </button>{' '}
      <span className="meta">
        <Standing row={row} />
      </span>
    </li>
  );
}
