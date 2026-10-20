/**
 * The one place that lists the workspace's Attention warnings.
 *
 * What the list is stands above it, always: the warnings are advisory and false alarms are
 * expected. An empty answer reads "No warnings found", with beside it that this does not make the
 * workspace safe or correct (SPEC section 11: "no clean scan becomes a safe-for-AI certificate").
 * A list that could not be read is a failure notice, never "no warnings". A check that fails
 * after the list was read replaces nothing on screen: what was last answered stays, the empty
 * answer included, with the failure in a notice beside it.
 *
 * Each warning says its kind in words, the service's own explanation, the item it names as a link
 * pinned to the revision that was inspected, and the next step the service suggests. The item's
 * title is the one the list carries (`AttentionItem.item`, ruling 7): nothing is read for a row.
 * An item the list names and does not carry was purged: it is named by its identity, with no link.
 * When the warning is about a place inside the item (`selection`), the link opens the item there
 * and the row says which place. Warnings that uploads are waiting (`uploads_waiting`) link to the
 * uploads the service holds, on the import page. Rows are shown `SHOWN_STEP` at a time: "Show more
 * warnings" brings the next rows, asking the service for a further page when the listed ones are
 * all shown, and moves keyboard focus to the first row it brought.
 *
 * "Check again" asks for the list again; the checks depend on more than the revision (reviews,
 * dates), so the same revision can answer differently later.
 */
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useRef, useState } from 'react';
import type { AttentionItem, Revision } from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { describeSelection, short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import {
  ADVISORY,
  attentionKind,
  NOT_A_CERTIFICATE,
  nextStep,
  useAttention,
  warningsOf,
} from './attention';
import { useAttentionOpen } from './freshness';
import { InstructionFindings } from './InstructionFindings';
import { PreviousCheck } from './PreviousCheck';

export interface AttentionPageProps {
  workspaceId: string;
  /** The revision the workspace shows: the warnings are asked at it. */
  head: Revision;
}

/** Rows shown at first, and added by each "Show more warnings". */
export const SHOWN_STEP = 50;

/** Where the import page lists the uploads the service holds (`WaitingUploads.tsx`). */
export const HELD_UPLOADS = 'uploads-held';

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function AttentionPage({ workspaceId, head }: AttentionPageProps) {
  const heading = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const attention = useAttention(workspaceId, head);
  useAttentionOpen(workspaceId, head);
  const pages = attention.data?.pages;
  const warnings = warningsOf(pages);
  const inspected = pages?.at(0)?.revision ?? head;
  const [shown, setShown] = useState(SHOWN_STEP);
  const rows = warnings.slice(0, shown);
  const rowAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>(':scope > li').item(index) ?? null;

  const nothingListed = attention.data === undefined;
  const previous =
    !nothingListed && (attention.isRefetching || attention.isRefetchError || attention.isStale);
  const listFailure = useRetry({
    request: `${workspaceId} ${head}`,
    failed: attention.isError && nothingListed,
    error: attention.error,
    run: () => attention.refetch(),
    loaded: () => rowAt(0) ?? headingRef.current,
  });
  const laterFailure = useRetry({
    request: `${workspaceId} ${head}`,
    failed: attention.isError && !nothingListed,
    error: attention.error,
    run: () => (attention.isFetchNextPageError ? attention.fetchNextPage() : attention.refetch()),
    loaded: () => rowAt(0) ?? headingRef.current,
  });

  // "Show more warnings": the rows already listed first, then a further page of the service.
  const more = warnings.length > shown || attention.hasNextPage;
  const loadingMore = attention.isFetchingNextPage;
  const moreFailed = attention.isFetchNextPageError;
  const rowCount = rows.length;
  const focusFrom = useRef<number | null>(null);
  const showMore = () => {
    if (loadingMore) return;
    focusFrom.current = rowCount;
    if (warnings.length <= shown) void attention.fetchNextPage();
    setShown(shown + SHOWN_STEP);
  };
  useEffect(() => {
    const from = focusFrom.current;
    // Settled once a row came, or nothing more can come from this press.
    if (from === null || loadingMore || (rowCount <= from && !moreFailed && more)) return;
    focusFrom.current = null;
    (rowAt(from) ?? moreRef.current ?? rowAt(rowCount - 1))?.focus();
  });

  return (
    <section className="workspace-page" aria-labelledby={heading}>
      <h2 id={heading} ref={headingRef} tabIndex={-1} className="page-heading">
        Attention
      </h2>
      <p role="note" className="notice">
        {ADVISORY} A list with nothing in it, or an item with no warning, has not been found safe.
      </p>
      <p className="page-actions">
        {/* aria-disabled rather than disabled: the button keeps focus while the list is asked. */}
        <button
          type="button"
          aria-disabled={attention.isFetching}
          onClick={() => {
            if (!attention.isFetching) void attention.refetch();
          }}
        >
          {attention.isFetching && !attention.isPending ? 'Checking again…' : 'Check again'}
        </button>{' '}
        {attention.dataUpdatedAt > 0 && (
          <span className="meta">
            {previous ? 'Previous check at' : 'Checked at'}{' '}
            <Instant value={new Date(attention.dataUpdatedAt).toISOString()} />, for revision{' '}
            <code title={inspected}>{short(inspected)}</code>.
          </span>
        )}
      </p>
      {attention.isPending && !listFailure.retrying && <p role="status">Listing the warnings…</p>}
      {listFailure.shown && (
        <FailureNotice
          context="Listing this workspace’s warnings"
          error={listFailure.error}
          retry={listFailure}
          workspaceId={workspaceId}
        />
      )}
      {previous && <PreviousCheck failed={attention.isError} empty={warnings.length === 0} />}
      {pages !== undefined && warnings.length === 0 && (
        <p>
          {!previous && <strong>No warnings found.</strong>} {NOT_A_CERTIFICATE}
        </p>
      )}
      {rows.length > 0 && (
        <ul ref={listRef} className="attention-list" aria-labelledby={heading}>
          {keyed(rows, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
            <WarningRow
              key={key}
              workspaceId={workspaceId}
              warning={warning}
              revision={inspected}
            />
          ))}
        </ul>
      )}
      {/* Present from the start, so how many are shown is announced when it changes. */}
      <p role="status" className="meta">
        {warnings.length > 0
          ? `${count(rows.length, 'warning', 'warnings')} shown${more ? '; more exist' : ''}.${
              attention.hasNextPage
                ? ' The marks in the folders and on the tabs cover the warnings listed so far.'
                : ''
            }`
          : ''}
      </p>
      {laterFailure.shown && (
        <FailureNotice
          context="Listing the warnings again"
          error={laterFailure.error}
          retry={laterFailure}
          workspaceId={workspaceId}
        />
      )}
      {more && (
        <button type="button" ref={moreRef} aria-disabled={loadingMore} onClick={showMore}>
          {loadingMore ? 'Listing more warnings…' : 'Show more warnings'}
        </button>
      )}
    </section>
  );
}

function WarningRow({
  workspaceId,
  warning,
  revision,
}: {
  workspaceId: string;
  warning: AttentionItem;
  revision: Revision;
}) {
  const itemId = warning.item_id ?? null;
  const item = warning.item ?? null;
  const selection = warning.selection ?? null;
  return (
    <li tabIndex={-1} className="attention-row">
      <p>
        <strong>{attentionKind(warning.kind)}</strong>
        {itemId === null ? (
          <span className="meta"> (the workspace as a whole)</span>
        ) : item === null ? (
          <span className="meta">
            {' '}
            in an item that is no longer there (item {itemId.slice(0, 8)})
          </span>
        ) : (
          <>
            {' in '}
            <Link
              to="/w/$workspaceId/i/$itemId"
              params={{ workspaceId, itemId }}
              // The whole item is no place in it: no `sel`, as a search result's link.
              search={{
                rev: revision,
                ...(selection === null || selection.kind === 'all' ? {} : { sel: selection }),
              }}
            >
              {item.title || item.path}
            </Link>
            {selection === null || selection.kind === 'all' ? null : (
              <span className="meta">, at {describeSelection(selection)}</span>
            )}
          </>
        )}
      </p>
      <p>{warning.message}</p>
      {warning.instruction_like && <InstructionFindings evidence={warning.instruction_like} />}
      {warning.kind === 'uploads_waiting' ? (
        <p>
          <Link to="/w/$workspaceId/import" params={{ workspaceId }} hash={HELD_UPLOADS}>
            See the uploads waiting to be imported
          </Link>
        </p>
      ) : (
        <p className="meta">Suggested next step: {nextStep(warning.action)}.</p>
      )}
    </li>
  );
}
