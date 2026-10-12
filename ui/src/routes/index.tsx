/** The workspaces this person may open, from the server; never fixture workspaces. */

import { useInfiniteQuery } from '@tanstack/react-query';
import { createFileRoute, Link } from '@tanstack/react-router';
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
        <ul className="workspace-list">
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
        <button
          type="button"
          disabled={result.isFetchingNextPage}
          onClick={() => void result.fetchNextPage()}
        >
          {result.isFetchingNextPage ? 'Loading more workspaces…' : 'Show more workspaces'}
        </button>
      )}
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
