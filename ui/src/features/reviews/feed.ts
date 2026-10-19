/**
 * Proposals and reviews kept current from the workspace's event stream (`stream_events`), the
 * stream the jobs page follows (`jobs/feed.ts`, `followEvents`).
 *
 * An event is a notification, not content: `proposal_updated` says a proposal changed (opened,
 * accepted, declined, put in conflict), `reviewed` that an item's review was recorded. The
 * proposals and the comments, or the item's reviews, are then asked again; nothing shown is taken
 * from an event. An event this build cannot read may concern either, so both are asked again.
 * Subscribe, then ask: each time the stream opens, the first time included, what is on screen is
 * asked once, since a change made before the stream opened sent its event to nobody. A read in
 * flight then was sent before the reason to ask, so it is given up and asked anew (`askAgain`).
 *
 * A read asked again replaces nothing on screen when it fails: TanStack Query keeps the last
 * answer, and the screens show the failure beside it.
 *
 * The stream is held while the page is shown, closed when it is hidden, and opened again from the
 * last event seen when it is shown again. When it cannot be opened for a reason a repeat cannot
 * cure (`not_implemented`, no permission), nothing more is tried and the screens show what they
 * last read; a refresh of the workspace, or the person's own action, asks again.
 */
import { type QueryClient, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { z } from 'zod';
import { zEvent } from '../../api/generated/zod.gen';
import { followEvents } from '../jobs/feed';
import { askAgain } from '../jobs/jobs';

/** The parts of a generated query key these screens read. */
const zKeyOf = z.object({
  _id: z.string(),
  body: z
    .object({
      workspace_id: z.string(),
      item_id: z.string().optional(),
      proposal_id: z.string().optional(),
    })
    .loose(),
});

/** The operation and request a query is keyed by, when it is a generated one. */
function keyOf(queryKey: readonly unknown[]) {
  const parsed = zKeyOf.safeParse(queryKey.at(0));
  return parsed.success ? parsed.data : null;
}

/** Ask again for an item's reviews, or every item's when `itemId` is null. */
export function reviewsChanged(client: QueryClient, workspaceId: string, itemId: string | null) {
  // A request in flight was sent before the reason to ask: it is given up and asked anew.
  void askAgain(client, {
    predicate: (query) => {
      const key = keyOf(query.queryKey);
      return (
        key !== null &&
        key._id === 'listReviews' &&
        key.body.workspace_id === workspaceId &&
        (itemId === null || key.body.item_id === itemId)
      );
    },
  });
}

/** The reads of a proposal: the list, the proposal itself and its discussion. */
const PROPOSAL_READS: ReadonlySet<string> = new Set([
  'listProposals',
  'getProposal',
  'listComments',
]);

/** Ask again for the proposals, and for one proposal and its comments (every one's when null). */
export function proposalsChanged(
  client: QueryClient,
  workspaceId: string,
  proposalId: string | null,
) {
  // A request in flight was sent before the reason to ask: it is given up and asked anew.
  void askAgain(client, {
    predicate: (query) => {
      const key = keyOf(query.queryKey);
      if (key === null || !PROPOSAL_READS.has(key._id)) return false;
      if (key.body.workspace_id !== workspaceId) return false;
      return (
        key._id === 'listProposals' || proposalId === null || key.body.proposal_id === proposalId
      );
    },
  });
}

/** What an event asks to be read again. */
function concerned(client: QueryClient, workspaceId: string, data: unknown) {
  const event = zEvent.safeParse(data);
  if (!event.success) {
    proposalsChanged(client, workspaceId, null);
    reviewsChanged(client, workspaceId, null);
    return;
  }
  if (event.data.kind === 'proposal_updated') {
    proposalsChanged(client, workspaceId, event.data.proposal_id ?? null);
  } else if (event.data.kind === 'reviewed') {
    reviewsChanged(client, workspaceId, event.data.item_id ?? null);
  }
}

/** Follow the workspace's events while the calling screen is mounted and the page is shown. */
export function useReviewFeed(workspaceId: string) {
  const client = useQueryClient();
  useEffect(() => {
    let stop: (() => string | undefined) | null = null;
    let lastEventId: string | undefined;
    const open = () => {
      stop = followEvents(workspaceId, lastEventId, {
        onState: (next) => {
          if (next.state !== 'live') return;
          proposalsChanged(client, workspaceId, null);
          reviewsChanged(client, workspaceId, null);
        },
        onEvent: (data) => concerned(client, workspaceId, data),
      });
    };
    const close = () => {
      if (stop === null) return;
      lastEventId = stop();
      stop = null;
    };
    const visibility = () => {
      if (document.visibilityState === 'hidden') close();
      else if (stop === null) open();
    };
    visibility();
    document.addEventListener('visibilitychange', visibility);
    return () => {
      document.removeEventListener('visibilitychange', visibility);
      close();
    };
  }, [client, workspaceId]);
}
