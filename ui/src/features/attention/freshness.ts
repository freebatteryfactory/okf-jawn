/** One AttentionLink owns event/return refreshes; observers share the query, with no scan clock. */
import { type QueryClient, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { zEvent } from '../../api/generated/zod.gen';
import { type FeedState, followEvents } from '../jobs/feed';
import { attentionOptions } from './attention';

/** The jobs askAgain cancellation rule, with lifecycle checks on both sides of its await. */
async function checkCurrent(
  client: QueryClient,
  queryKey: ReturnType<typeof attentionOptions>['queryKey'],
  current: () => boolean,
) {
  if (!current()) return;
  await client.cancelQueries({ queryKey, predicate: (query) => query.state.data === undefined });
  if (!current()) return;
  await client.invalidateQueries({ queryKey });
}

/** Opening the list is an explicit request, including where events cannot be followed. */
export function useAttentionOpen(workspaceId: string, head: Revision) {
  const client = useQueryClient();
  useEffect(() => {
    let active = true;
    const queryKey = attentionOptions(workspaceId, head).queryKey;
    // Its first observer already asks; a later visit must not reuse an indefinitely old answer.
    if (client.getQueryData(queryKey) !== undefined) {
      void checkCurrent(client, queryKey, () => active && document.visibilityState !== 'hidden');
    }
    return () => {
      active = false;
    };
  }, [client, workspaceId, head]);
}

/** Subscribe then ask; close while hidden and resume from the last observed event identity. */
export function useAttentionLifecycle(workspaceId: string, head: Revision) {
  const client = useQueryClient();
  const [state, setState] = useState<FeedState>('opening');
  useEffect(() => {
    const queryKey = attentionOptions(workspaceId, head).queryKey;
    let stop: (() => string | undefined) | null = null;
    let lastEventId: string | undefined;
    let feed: FeedState = 'opening';
    let wasLive = false;
    let active = true;
    let generation = 0;
    // A read begun before losing contact cannot make old-rule findings current again.
    let uncertainty = Promise.resolve();
    const previous = () => {
      const requested = ++generation;
      const current = () => active && requested === generation;
      uncertainty = uncertainty.then(async () => {
        if (!current()) return;
        await client.cancelQueries({ queryKey });
        if (!current()) return;
        await client.invalidateQueries({ queryKey, refetchType: 'none' });
      });
    };
    const check = () => {
      const requested = ++generation;
      void uncertainty.then(() =>
        checkCurrent(
          client,
          queryKey,
          () => active && requested === generation && document.visibilityState !== 'hidden',
        ),
      );
    };
    const open = () => {
      generation += 1;
      feed = 'opening';
      setState(feed);
      stop = followEvents(workspaceId, lastEventId, {
        onState: (next) => {
          feed = next.state;
          setState(feed);
          if (feed === 'live') {
            wasLive = true;
            check();
          } else if (wasLive) previous();
        },
        onEvent: (data) => {
          const event = zEvent.safeParse(data);
          // Progress notifications do not justify scanning the entire workspace on each tick.
          if (
            !event.success ||
            ['changed', 'imported', 'reviewed', 'proposal_updated'].includes(event.data.kind)
          )
            check();
        },
      });
    };
    const close = () => {
      if (stop === null) return;
      lastEventId = stop();
      stop = null;
    };
    const visibility = () => {
      if (document.visibilityState === 'hidden') {
        close();
        feed = 'hidden';
        setState(feed);
        previous();
      } else if (stop === null) open();
    };
    const focus = () => {
      // A returning hidden page is about to subscribe then ask; do not duplicate that read.
      if (document.visibilityState !== 'hidden' && feed !== 'hidden' && feed !== 'opening') check();
    };
    visibility();
    document.addEventListener('visibilitychange', visibility);
    window.addEventListener('focus', focus);
    return () => {
      active = false;
      generation += 1;
      document.removeEventListener('visibilitychange', visibility);
      window.removeEventListener('focus', focus);
      close();
    };
  }, [client, workspaceId, head]);
  return state;
}
