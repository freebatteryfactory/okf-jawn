/**
 * The workspaces this person may open, from the server; never fixture workspaces.
 *
 * "Show more workspaces" keeps keyboard focus while it loads (aria-disabled, not disabled: Chromium
 * and WebKit move focus to the body when a focused button becomes disabled), then moves focus to
 * the first workspace it brought; if none came, focus stays on a control still there, and a live
 * region says how many are shown.
 */

import { useInfiniteQuery } from '@tanstack/react-query';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useEffect, useRef, useState } from 'react';
import { listWorkspacesQueryKey } from '../api/generated/@tanstack/react-query.gen';
import { listWorkspaces } from '../api/generated/sdk.gen';
import { FailureNotice } from '../features/errors/FailureNotice';
import { Instant } from '../lib/instant';

export const Route = createFileRoute('/')({ component: Home });

const PAGE = 50;

function Home() {
  const options = { body: { page: { limit: PAGE } } };
  const result = useInfiniteQuery({
    queryKey: [{ ...listWorkspacesQueryKey(options)[0], _infinite: true }],
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listWorkspaces({
        body: { page: { limit: PAGE, cursor: pageParam } },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const workspaces = result.data?.pages.flatMap((page) => page.items) ?? [];

  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const focusFrom = useRef<{ shown: number; pages: number } | null>(null);
  const [shownNote, setShownNote] = useState('');
  const loadingMore = result.isFetchingNextPage;
  const moreFailed = result.isFetchNextPageError;
  const pageCount = result.data?.pages.length ?? 0;
  const shownCount = workspaces.length;
  const more = result.hasNextPage;
  useEffect(() => {
    const asked = focusFrom.current;
    // Settled once another page is here (even an empty one), or the next page failed.
    if (asked === null || !(pageCount > asked.pages || (moreFailed && !loadingMore))) return;
    focusFrom.current = null;
    const links = listRef.current?.querySelectorAll<HTMLElement>(':scope > li > a');
    const first = links?.item(asked.shown);
    if (first) first.focus();
    else if (moreRef.current) moreRef.current.focus();
    else links?.item(links.length - 1)?.focus();
    if (!moreFailed) {
      setShownNote(
        `${shownCount} ${shownCount === 1 ? 'workspace' : 'workspaces'} shown${more ? '; more are available' : ''}.`,
      );
    }
  }, [pageCount, loadingMore, moreFailed, shownCount, more]);
  const loadMore = () => {
    if (loadingMore) return;
    focusFrom.current = { shown: shownCount, pages: pageCount };
    void result.fetchNextPage();
  };
  return (
    <section className="home" aria-labelledby="workspaces-heading">
      <h1 id="workspaces-heading">Your workspaces</h1>
      {result.isPending && <p role="status">Contacting the workspace service…</p>}
      {result.isError && !result.isFetchNextPageError && (
        <FailureNotice
          context="Listing your workspaces"
          error={result.error}
          onRetry={() => void result.refetch()}
        />
      )}
      {result.isSuccess && workspaces.length === 0 && (
        <p>You have no workspaces yet. No sample data has been substituted.</p>
      )}
      {workspaces.length > 0 && (
        <ul className="workspace-list" ref={listRef}>
          {workspaces.map((workspace) => (
            <li key={workspace.id}>
              <Link to="/w/$workspaceId" params={{ workspaceId: workspace.id }}>
                {workspace.name}
              </Link>
              {workspace.description ? <p>{workspace.description}</p> : null}
              <p className="meta">
                Created <Instant value={workspace.created_at} />
                {workspace.archived_at ? (
                  <>
                    {' · '}Archived <Instant value={workspace.archived_at} />
                  </>
                ) : null}
              </p>
            </li>
          ))}
        </ul>
      )}
      {result.hasNextPage && (
        <button type="button" ref={moreRef} aria-disabled={loadingMore} onClick={loadMore}>
          {result.isFetchingNextPage ? 'Loading more workspaces…' : 'Show more workspaces'}
        </button>
      )}
      <p role="status" className="sr-only">
        {shownNote}
      </p>
      {result.isFetchNextPageError && (
        <FailureNotice
          context="Listing more workspaces"
          error={result.error}
          onRetry={() => void result.fetchNextPage()}
        />
      )}
    </section>
  );
}
