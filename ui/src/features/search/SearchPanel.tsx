/**
 * Workspace search: a search form whose query lives in the URL, and the results of the search
 * operation, pinned to one resolved revision and paged through the operation's cursor. Each
 * result opens its item at the exact cited selection.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId } from 'react';
import { searchItemsQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { searchItems } from '../../api/generated/sdk.gen';
import type { Revision, SearchRequest } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { describeSelection } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';

/** Results per page. */
export const SEARCH_PAGE = 20;

export interface SearchFormProps {
  query: string | undefined;
  onSearch: (query: string | undefined) => void;
}

export function SearchForm({ query, onSearch }: SearchFormProps) {
  const inputId = useId();
  return (
    <search aria-label="Search this workspace" className="search-form">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          const value = new FormData(event.currentTarget).get('q');
          const text = typeof value === 'string' ? value.trim() : '';
          onSearch(text.length > 0 ? text : undefined);
        }}
      >
        <label htmlFor={inputId}>Search this workspace</label>
        <div className="search-row">
          <input id={inputId} key={query ?? ''} name="q" type="search" defaultValue={query ?? ''} />
          <button type="submit">Search</button>
        </div>
      </form>
    </search>
  );
}

export interface SearchResultsProps {
  workspaceId: string;
  head: Revision;
  query: string;
  onClear: () => void;
}

export function SearchResults({ workspaceId, head, query, onClear }: SearchResultsProps) {
  const headingId = useId();
  const request: SearchRequest = {
    workspace_id: workspaceId,
    at: { kind: 'revision', revision: head },
    query,
    include_archived: false,
    page: { limit: SEARCH_PAGE, cursor: null },
  };
  const results = useInfiniteQuery({
    queryKey: [{ ...searchItemsQueryKey({ body: request })[0], _infinite: true }],
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await searchItems({
        body: { ...request, page: { limit: SEARCH_PAGE, cursor: pageParam } },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const hits = results.data?.pages.flatMap((page) => page.hits) ?? [];
  return (
    <section className="search-results" aria-labelledby={headingId}>
      <div className="panel-title-row">
        <h2 id={headingId} className="panel-heading">
          Results for “{query}”
        </h2>
        <button type="button" onClick={onClear}>
          Back to folders
        </button>
      </div>
      {results.isPending && <p role="status">Searching…</p>}
      {results.isError && !results.isFetchNextPageError && (
        <FailureNotice
          context="Searching"
          error={results.error}
          onRetry={() => void results.refetch()}
        />
      )}
      {results.isSuccess && hits.length === 0 && <p role="status">No matches for “{query}”.</p>}
      {hits.length > 0 && (
        <>
          <p role="status">
            {hits.length} {hits.length === 1 ? 'result' : 'results'} shown
            {results.hasNextPage ? '; more are available' : ''}.
          </p>
          <ol className="hit-list">
            {keyed(hits, (hit) => JSON.stringify(hit.source)).map(({ key, value: hit }) => (
              <li key={key}>
                <Link
                  to="/w/$workspaceId/i/$itemId"
                  params={{ workspaceId, itemId: hit.source.item_id }}
                  search={{
                    q: query,
                    ...(hit.source.selection.kind === 'all' ? {} : { sel: hit.source.selection }),
                    ...(hit.source.revision === head ? {} : { rev: hit.source.revision }),
                  }}
                  className="hit"
                >
                  <span className="hit-title">{hit.title}</span>
                  <span className="hit-where">
                    {hit.source.path}, {describeSelection(hit.source.selection)}
                  </span>
                  <span className="hit-snippet">{hit.snippet}</span>
                </Link>
              </li>
            ))}
          </ol>
        </>
      )}
      {results.hasNextPage && (
        <button
          type="button"
          disabled={results.isFetchingNextPage}
          onClick={() => void results.fetchNextPage()}
        >
          {results.isFetchingNextPage ? 'Loading more results…' : 'Show more results'}
        </button>
      )}
      {results.isFetchNextPageError && (
        <FailureNotice
          context="Loading more results"
          error={results.error}
          onRetry={() => void results.fetchNextPage()}
        />
      )}
    </section>
  );
}
