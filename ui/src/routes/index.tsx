/**
 * The workspaces this person may open, from the server; never fixture workspaces.
 *
 * "Show more workspaces" keeps keyboard focus while it loads (aria-disabled, not disabled: Chromium
 * and WebKit move focus to the body when a focused button becomes disabled), then moves focus to
 * the first workspace it brought; if none came, focus stays on a control still there, and a live
 * region says how many are shown. A failed listing keeps its notice through a retry, which ends
 * with focus on the first workspace it brought (the heading, when there is none).
 */

import { useInfiniteQuery } from '@tanstack/react-query';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useEffect, useRef } from 'react';
import { listWorkspacesQueryKey } from '../api/generated/@tanstack/react-query.gen';
import { listWorkspaces } from '../api/generated/sdk.gen';
import { InstallationAdminLink } from '../features/admin/InstallationAdminLink';
import { NewWorkspaceLink } from '../features/admin/installation/NewWorkspaceLink';
import { FailureNotice } from '../features/errors/FailureNotice';
import { useRetry } from '../features/errors/retry';
import { Instant } from '../lib/instant';

export const Route = createFileRoute('/')({ component: Home });

const PAGE = 50;

/** The link of the workspace at `index` in the list, when it is on screen. */
function workspaceLink(list: HTMLUListElement | null, index: number): HTMLElement | null {
  return list?.querySelectorAll<HTMLElement>(':scope > li > a').item(index) ?? null;
}

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

  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const focusFrom = useRef<{ shown: number; pages: number } | null>(null);
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
    const list = listRef.current;
    (
      workspaceLink(list, asked.shown) ??
      moreRef.current ??
      workspaceLink(list, shownCount - 1)
    )?.focus();
  }, [pageCount, loadingMore, moreFailed, shownCount]);
  const loadMore = () => {
    if (loadingMore) return;
    focusFrom.current = { shown: shownCount, pages: pageCount };
    void result.fetchNextPage();
  };

  const listFailure = useRetry({
    request: 'workspaces',
    failed: result.isError && !result.isFetchNextPageError,
    error: result.error,
    run: () => result.refetch(),
    // The first workspace it listed; with none, the page's heading.
    loaded: () => workspaceLink(listRef.current, 0) ?? headingRef.current,
  });
  const lastPageSize = result.data?.pages.at(-1)?.items.length ?? 0;
  const moreFailure = useRetry({
    request: 'more workspaces',
    failed: result.isFetchNextPageError,
    error: result.error,
    run: () => result.fetchNextPage(),
    // The first workspace of the page it brought; if that page was empty, a control still there.
    loaded: () =>
      workspaceLink(listRef.current, shownCount - lastPageSize) ??
      moreRef.current ??
      workspaceLink(listRef.current, shownCount - 1),
  });
  // Once a later page has come, however it was asked for: how many are shown now.
  const shownNote =
    pageCount > 1
      ? `${shownCount} ${shownCount === 1 ? 'workspace' : 'workspaces'} shown${more ? '; more are available' : ''}.`
      : '';
  return (
    <section className="home" aria-labelledby="workspaces-heading">
      <h1 id="workspaces-heading" ref={headingRef} tabIndex={-1}>
        Your workspaces
      </h1>
      <InstallationAdminLink workspaces={workspaces} />
      <NewWorkspaceLink workspaces={workspaces} />
      {result.isPending && !listFailure.retrying && (
        <p role="status">Contacting the workspace service…</p>
      )}
      {listFailure.shown && (
        <FailureNotice
          context="Listing your workspaces"
          error={listFailure.error}
          retry={listFailure}
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
      {moreFailure.shown && (
        <FailureNotice
          context="Listing more workspaces"
          error={moreFailure.error}
          retry={moreFailure}
        />
      )}
    </section>
  );
}
