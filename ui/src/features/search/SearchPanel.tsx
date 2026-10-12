/**
 * Workspace search: a search form whose query lives in the URL, and the results of the search
 * operation, pinned to one resolved revision and paged through the operation's cursor. Each
 * result opens its item at the exact cited selection and the revision its hit was resolved at
 * (SPEC section 3), and a result from partly converted, agent-supplied or corrected text carries
 * the same labels the reader shows. A failed search or further page keeps its notice through a
 * retry (`useRetry`).
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useRef, useState } from 'react';
import { searchItemsQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { searchItems } from '../../api/generated/sdk.gen';
import type { Revision, SearchRequest } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { describeSelection, extractionLabels } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';

/** Results per page. */
export const SEARCH_PAGE = 20;

export interface SearchFormProps {
  query: string | undefined;
  onSearch: (query: string | undefined) => void;
}

/**
 * The search box. It is never remounted, so submitting keeps keyboard focus in it; its text
 * follows the URL's query when that changes from elsewhere (back, "Back to folders").
 */
export function SearchForm({ query, onSearch }: SearchFormProps) {
  const inputId = useId();
  const [text, setText] = useState(query ?? '');
  const [followed, setFollowed] = useState(query);
  if (followed !== query) {
    setFollowed(query);
    setText(query ?? '');
  }
  return (
    <search aria-label="Search this workspace" className="search-form">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          const trimmed = text.trim();
          onSearch(trimmed.length > 0 ? trimmed : undefined);
        }}
      >
        <label htmlFor={inputId}>Search this workspace</label>
        <div className="search-row">
          <input
            id={inputId}
            name="q"
            type="search"
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
          <button type="submit">Search</button>
        </div>
      </form>
    </search>
  );
}

/**
 * The words of the results' live region: searching, failed, none, or how many are shown. While a
 * failure notice is trying again, the notice says so and this line says nothing of that request.
 */
function searchStatus(
  results: {
    isPending: boolean;
    isError: boolean;
    isFetchNextPageError: boolean;
    isFetchingNextPage: boolean;
    hasNextPage: boolean;
  },
  shown: number,
  query: string,
  retrying: boolean,
): string {
  if (results.isPending) return retrying ? '' : 'Searching…';
  if (results.isError && !results.isFetchNextPageError) return 'The search did not complete.';
  if (shown === 0) return `No matches for “${query}”.`;
  if (results.isFetchingNextPage && !retrying) return 'Loading more results…';
  const count = `${shown} ${shown === 1 ? 'result' : 'results'} shown`;
  if (results.isFetchNextPageError) return `${count}; more could not be loaded.`;
  return `${count}${results.hasNextPage ? '; more are available' : ''}.`;
}

/** The link of the result at `index` in the list, when it is on screen. */
function hitLink(list: HTMLOListElement | null, index: number): HTMLElement | null {
  return list?.querySelectorAll<HTMLElement>('a.hit').item(index) ?? null;
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

  // "Show more results" moves keyboard focus to the first newly loaded result. If nothing new
  // arrived, focus stays on a control that is still there, and the status says what happened.
  const listRef = useRef<HTMLOListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const focusFrom = useRef<{ hits: number; pages: number } | null>(null);
  const loadingMore = results.isFetchingNextPage;
  const moreFailed = results.isFetchNextPageError;
  const pageCount = results.data?.pages.length ?? 0;
  useEffect(() => {
    const asked = focusFrom.current;
    // Settled once another page is here (even an empty one), or the next page failed.
    if (asked === null || !(pageCount > asked.pages || (moreFailed && !loadingMore))) return;
    focusFrom.current = null;
    const links = listRef.current?.querySelectorAll<HTMLElement>('a.hit');
    const first = links?.item(asked.hits);
    if (first) first.focus();
    else if (moreRef.current) moreRef.current.focus();
    else links?.item(links.length - 1)?.focus();
  }, [pageCount, loadingMore, moreFailed]);
  const loadMore = () => {
    if (loadingMore) return;
    focusFrom.current = { hits: hits.length, pages: pageCount };
    void results.fetchNextPage();
  };

  // A failed search, or a failed further page, keeps its notice through a retry, which ends with
  // keyboard focus on the first result it brought (the heading, when the search found none).
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const searched = `${query} ${head}`;
  const searchFailure = useRetry({
    request: searched,
    failed: results.isError && !results.isFetchNextPageError,
    error: results.error,
    run: () => results.refetch(),
    loaded: () => hitLink(listRef.current, 0) ?? headingRef.current,
  });
  const lastPageSize = results.data?.pages.at(-1)?.hits.length ?? 0;
  const moreFailure = useRetry({
    request: searched,
    failed: results.isFetchNextPageError,
    error: results.error,
    run: () => results.fetchNextPage(),
    // The first result of the page it brought; if that page was empty, a control still there.
    loaded: () =>
      hitLink(listRef.current, hits.length - lastPageSize) ??
      moreRef.current ??
      hitLink(listRef.current, hits.length - 1),
  });

  return (
    <section className="search-results" aria-labelledby={headingId}>
      <div className="panel-title-row">
        <h2 id={headingId} ref={headingRef} tabIndex={-1} className="panel-heading">
          Results for “{query}”
        </h2>
        <button type="button" onClick={onClear}>
          Back to folders
        </button>
      </div>
      {/* One polite live region, present from the start, announces what the search found. */}
      <p role="status">
        {searchStatus(results, hits.length, query, searchFailure.retrying || moreFailure.retrying)}
      </p>
      {searchFailure.shown && (
        <FailureNotice context="Searching" error={searchFailure.error} retry={searchFailure} />
      )}
      {hits.length > 0 && (
        <ol className="hit-list" ref={listRef}>
          {keyed(hits, (hit) => JSON.stringify(hit.source)).map(({ key, value: hit }) => (
            <li key={key}>
              <Link
                to="/w/$workspaceId/i/$itemId"
                params={{ workspaceId, itemId: hit.source.item_id }}
                search={{
                  q: query,
                  ...(hit.source.selection.kind === 'all' ? {} : { sel: hit.source.selection }),
                  // Always the revision the hit was resolved at, even when it is the listed head:
                  // reopened after the head moves, the link still reads the same commit.
                  rev: hit.source.revision,
                }}
                className="hit"
              >
                <span className="hit-title">{hit.title}</span>
                <span className="hit-where">
                  {hit.source.path}, {describeSelection(hit.source.selection)}
                </span>
                {extractionLabels(hit.extraction).map((label) => (
                  <span key={label} className="hit-label">
                    {label}
                  </span>
                ))}
                <span className="hit-snippet">{hit.snippet}</span>
              </Link>
            </li>
          ))}
        </ol>
      )}
      {results.hasNextPage && (
        // aria-disabled rather than disabled while loading: the button stays focusable and
        // announced. A disabled one would lose focus: Chromium and WebKit move focus to the body
        // within a frame of a focused button becoming disabled (HTML's focus fixup).
        <button type="button" ref={moreRef} aria-disabled={loadingMore} onClick={loadMore}>
          {loadingMore ? 'Loading more results…' : 'Show more results'}
        </button>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Loading more results"
          error={moreFailure.error}
          retry={moreFailure}
        />
      )}
    </section>
  );
}
