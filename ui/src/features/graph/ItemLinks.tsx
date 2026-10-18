/**
 * An item's links, beside it: what it links to, and what links to it (its backlinks), at the
 * revision the tab reads (`get_links`, both directions; SPEC section 7: "Links/backlinks and a
 * bounded graph derive from the same source state").
 *
 * A link the item makes is its text and the path it names; it opens the item it leads to, pinned
 * to the revision the links were read at. One that leads nowhere (no item is at that path) says
 * so and offers nothing to follow. A backlink is named by its own text and the line it is on in
 * the item that makes it, and opens that item at that line: the operation answers the linking
 * item's identity, not its title, so its title is not shown here, and nothing more is asked for
 * it (a request is in the report: `Link` naming the linking item's path).
 *
 * Links come `LINKS_PAGE` at a time; "Show more links" asks for the next page from the cursor,
 * and the service keeps it at the revision the first page resolved. A list that fails with
 * nothing listed is a failure notice, never "no links".
 */
import { infiniteQueryOptions, useInfiniteQuery } from '@tanstack/react-query';
import { Link as RouterLink } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import { getLinksQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { getLinks } from '../../api/generated/sdk.gen';
import type { Link, Revision } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';

/** Links asked for per page. */
export const LINKS_PAGE = 100;

export function linksOptions(workspaceId: string, itemId: string, revision: Revision) {
  const body = {
    workspace_id: workspaceId,
    item_id: itemId,
    at: { kind: 'revision' as const, revision },
    direction: 'both' as const,
  };
  return infiniteQueryOptions({
    queryKey: [
      {
        ...getLinksQueryKey({ body: { ...body, page: { limit: LINKS_PAGE } } })[0],
        _infinite: true,
      },
    ] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await getLinks({
        body: {
          ...body,
          page:
            pageParam === null ? { limit: LINKS_PAGE } : { limit: LINKS_PAGE, cursor: pageParam },
        },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export interface ItemLinksProps {
  workspaceId: string;
  itemId: string;
  revision: Revision;
}

export function ItemLinks({ workspaceId, itemId, revision }: ItemLinksProps) {
  const headingId = useId();
  const fromId = useId();
  const toId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const links = useInfiniteQuery(linksOptions(workspaceId, itemId, revision));
  const pages = links.data?.pages ?? [];
  const read = pages.at(0)?.revision ?? revision;
  const all = pages.flatMap((page) => page.links);
  const outgoing = all.filter((link) => link.from === itemId);
  const incoming = all.filter((link) => link.to === itemId);
  const nothingListed = links.data === undefined;
  const firstFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: links.isError && nothingListed,
    error: links.error,
    run: () => links.refetch(),
    loaded: () => headingRef.current,
  });
  const moreFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: links.isError && !nothingListed,
    error: links.error,
    run: () => links.fetchNextPage(),
    loaded: () => headingRef.current,
  });
  return (
    <section className="item-links" aria-labelledby={headingId}>
      <h2 id={headingId} ref={headingRef} tabIndex={-1} className="panel-heading">
        Links
      </h2>
      {links.isPending && !firstFailure.retrying && <p role="status">Reading the links…</p>}
      {firstFailure.shown && (
        <FailureNotice
          context="Reading this item’s links"
          error={firstFailure.error}
          retry={firstFailure}
          workspaceId={workspaceId}
        />
      )}
      {links.data !== undefined && all.length === 0 && !links.hasNextPage && (
        <p>This item links to nothing, and nothing links to it.</p>
      )}
      {outgoing.length > 0 && (
        <>
          <h3 id={fromId}>From this item</h3>
          <ul aria-labelledby={fromId} className="link-list">
            {keyed(outgoing, (link) => JSON.stringify(link)).map(({ key, value: link }) => (
              <Outgoing key={key} workspaceId={workspaceId} link={link} revision={read} />
            ))}
          </ul>
        </>
      )}
      {incoming.length > 0 && (
        <>
          <h3 id={toId}>To this item</h3>
          <ul aria-labelledby={toId} className="link-list">
            {keyed(incoming, (link) => JSON.stringify(link)).map(({ key, value: link }) => (
              <Incoming key={key} workspaceId={workspaceId} link={link} revision={read} />
            ))}
          </ul>
        </>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Reading more links"
          error={moreFailure.error}
          retry={moreFailure}
          workspaceId={workspaceId}
        />
      )}
      {links.hasNextPage && (
        <button
          type="button"
          aria-disabled={links.isFetchingNextPage}
          onClick={() => {
            if (!links.isFetchingNextPage) void links.fetchNextPage();
          }}
        >
          {links.isFetchingNextPage ? 'Reading more links…' : 'Show more links'}
        </button>
      )}
    </section>
  );
}

function Outgoing({
  workspaceId,
  link,
  revision,
}: {
  workspaceId: string;
  link: Link;
  revision: Revision;
}) {
  const label = link.label || link.to_path;
  const to = link.to ?? null;
  return (
    <li>
      {to === null ? (
        <>
          {label}{' '}
          <span className="meta">({link.to_path}: leads nowhere, nothing is at that path)</span>
        </>
      ) : (
        <>
          <RouterLink
            to="/w/$workspaceId/i/$itemId"
            params={{ workspaceId, itemId: to }}
            search={{ rev: revision }}
          >
            {label}
          </RouterLink>{' '}
          <span className="meta">
            ({link.to_path}, line {link.line})
          </span>
        </>
      )}
    </li>
  );
}

function Incoming({
  workspaceId,
  link,
  revision,
}: {
  workspaceId: string;
  link: Link;
  revision: Revision;
}) {
  return (
    <li>
      <RouterLink
        to="/w/$workspaceId/i/$itemId"
        params={{ workspaceId, itemId: link.from }}
        search={{
          rev: revision,
          sel: { kind: 'lines', range: { start: link.line, end: link.line } },
        }}
      >
        {link.label || link.to_path}
      </RouterLink>{' '}
      <span className="meta">(in another item, line {link.line})</span>
    </li>
  );
}
