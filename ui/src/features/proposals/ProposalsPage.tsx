/**
 * The workspace's proposals: each with its status as the service answers it, who made it, how it
 * came (a signed-in browser, an agent through MCP) and when, newest first, with a choice of which
 * statuses to show.
 *
 * Proposals are made from an item's page ("Propose a change…"); the page says so, and says why
 * when the person may not propose. Proposals come `PROPOSALS_PAGE` at a time; "Show more
 * proposals" asks for the next page and moves keyboard focus to the first one it brought. A list
 * that fails with nothing listed is a failure notice, never "No proposals"; a later read that
 * fails leaves the list as it is, with its notice beside it. The list is asked again when the
 * workspace's events say a proposal changed.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useRef, useState } from 'react';
import type { Proposal, ProposalStatus } from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowed, missingWords } from '../explorer/rights';
import { useReviewFeed } from '../reviews/feed';
import { ROUTE_WORDS } from '../tree/Committed';
import { useWorkspaceChanges } from '../tree/context';
import { KIND_WORDS, proposalsOptions, STATUS_WORDS } from './proposals';

const SHOWN: Array<{ value: ProposalStatus | 'all'; label: string }> = [
  { value: 'all', label: 'All proposals' },
  { value: 'open', label: 'Open' },
  { value: 'accepted', label: 'Accepted' },
  { value: 'declined', label: 'Declined' },
  { value: 'conflict', label: 'In conflict' },
];

/** How a proposal came, in words: the route the service recorded. */
export function routeWords(proposal: Proposal): string {
  return (
    ROUTE_WORDS[proposal.created_via] ?? `${proposal.created_via} (a route this page does not know)`
  );
}

export function ProposalsPage({ workspaceId }: { workspaceId: string }) {
  const { rights } = useWorkspaceChanges();
  useReviewFeed(workspaceId);
  const headingId = useId();
  const showId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const [shown, setShown] = useState<ProposalStatus | 'all'>('all');
  const list = useInfiniteQuery(proposalsOptions(workspaceId, shown === 'all' ? null : shown));
  const proposals = (list.data?.pages ?? []).flatMap((page) => page.items);
  const rowAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>(':scope > li').item(index) ?? null;
  const nothingListed = list.data === undefined;
  const firstFailure = useRetry({
    request: `${workspaceId} ${shown}`,
    failed: list.isError && nothingListed,
    error: list.error,
    run: () => list.refetch(),
    loaded: () => rowAt(0) ?? headingRef.current,
  });
  const laterFailure = useRetry({
    request: `${workspaceId} ${shown}`,
    failed: list.isError && !nothingListed,
    error: list.error,
    run: () => (list.isFetchNextPageError ? list.fetchNextPage() : list.refetch()),
    loaded: () => rowAt(0) ?? headingRef.current,
  });
  const count = proposals.length;
  const focusFrom = useRef<number | null>(null);
  useEffect(() => {
    const from = focusFrom.current;
    if (from === null || list.isFetchingNextPage) return;
    if (count <= from && !list.isFetchNextPageError && list.hasNextPage) return;
    focusFrom.current = null;
    (rowAt(from) ?? moreRef.current ?? rowAt(count - 1))?.focus();
  });
  const mayPropose = allowed(rights, 'propose') !== 'no-permission';

  return (
    <section className="workspace-page" aria-labelledby={headingId}>
      <h2 id={headingId} ref={headingRef} tabIndex={-1} className="page-heading">
        Proposals
      </h2>
      <p className="meta">
        A proposal is a change someone suggests. It changes nothing until a person who may approve
        it accepts it.{' '}
        {mayPropose
          ? 'To propose a change, open the item and choose “Propose a change…”.'
          : missingWords('Proposing a change', 'propose')}
      </p>
      <p className="page-actions">
        <label htmlFor={showId}>Show</label>{' '}
        <select
          id={showId}
          value={shown}
          onChange={(event) => setShown(event.target.value as ProposalStatus | 'all')}
        >
          {SHOWN.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </p>
      {list.isPending && !firstFailure.retrying && <p role="status">Listing the proposals…</p>}
      {firstFailure.shown && (
        <FailureNotice
          context="Listing the proposals"
          error={firstFailure.error}
          retry={firstFailure}
          workspaceId={workspaceId}
        />
      )}
      {list.data !== undefined && proposals.length === 0 && (
        <p>{shown === 'all' ? 'No proposals.' : 'No proposals of this status.'}</p>
      )}
      {proposals.length > 0 && (
        <ul ref={listRef} className="proposal-list" aria-labelledby={headingId}>
          {keyed(proposals, (proposal) => proposal.id).map(({ key, value: proposal }) => (
            <li key={key} tabIndex={-1} className="proposal-row">
              <p>
                <Link
                  to="/w/$workspaceId/proposals/$proposalId"
                  params={{ workspaceId, proposalId: proposal.id }}
                >
                  {proposal.title || '(untitled proposal)'}
                </Link>
              </p>
              <p>
                <strong>{STATUS_WORDS[proposal.status] ?? proposal.status}</strong>
              </p>
              <p className="meta">
                {KIND_WORDS[proposal.kind] ?? proposal.kind}, proposed by {proposal.created_by}{' '}
                through {routeWords(proposal)} on <Instant value={proposal.created_at} />.
              </p>
            </li>
          ))}
        </ul>
      )}
      {laterFailure.shown && (
        <FailureNotice
          context="Listing the proposals again"
          error={laterFailure.error}
          retry={laterFailure}
          workspaceId={workspaceId}
        />
      )}
      {list.hasNextPage && (
        <button
          type="button"
          ref={moreRef}
          aria-disabled={list.isFetchingNextPage}
          onClick={() => {
            if (list.isFetchingNextPage) return;
            focusFrom.current = count;
            void list.fetchNextPage();
          }}
        >
          {list.isFetchingNextPage ? 'Listing more proposals…' : 'Show more proposals'}
        </button>
      )}
    </section>
  );
}
