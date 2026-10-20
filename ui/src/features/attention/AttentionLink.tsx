/**
 * The link to the Attention page, in the workspace's bar, with how many warnings are listed.
 *
 * It is always on screen while a workspace is open, so it is also what asks for the pages the
 * marks cover: `ATTENTION_AUTO_PAGES` pages, one after another. A page that fails stops that; the
 * Attention page shows the failure and its retry. A count is shown only when there are warnings:
 * "(3)", or "(1000+)" when the service has more than are listed. No count is not a statement that
 * there are none: the list may not have been read.
 */
import { Link } from '@tanstack/react-router';
import { type MouseEvent, useEffect } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { ATTENTION_AUTO_PAGES, useAttention, warningsOf } from './attention';
import { useAttentionLifecycle } from './freshness';

export interface AttentionLinkProps {
  workspaceId: string;
  /** The revision the workspace shows. */
  head: Revision;
  /** The workspace search the URL carries. */
  search: { q?: string };
  /** The link was pressed; the router acts on the press after this. */
  onFollow?: ((event: MouseEvent<HTMLAnchorElement>) => void) | undefined;
}

export function AttentionLink({ workspaceId, head, search, onFollow }: AttentionLinkProps) {
  const attention = useAttention(workspaceId, head);
  const feed = useAttentionLifecycle(workspaceId, head);
  const { hasNextPage, isFetching, isError, fetchNextPage } = attention;
  const pageCount = attention.data?.pages.length ?? 0;
  useEffect(() => {
    if (
      feed !== 'hidden' &&
      !attention.isStale &&
      hasNextPage &&
      !isFetching &&
      !isError &&
      pageCount < ATTENTION_AUTO_PAGES
    ) {
      void fetchNextPage();
    }
  }, [feed, attention.isStale, hasNextPage, isFetching, isError, pageCount, fetchNextPage]);
  const listed = warningsOf(attention.data?.pages).length;
  return (
    <Link
      to="/w/$workspaceId/attention"
      params={{ workspaceId }}
      search={search}
      onClick={onFollow}
      title={
        feed === 'unavailable'
          ? 'Live updates are unavailable. Attention rechecks when opened or when you return to this window.'
          : undefined
      }
    >
      Attention{listed > 0 ? ` (${listed}${hasNextPage ? '+' : ''})` : ''}
    </Link>
  );
}
