/**
 * The workspace's event stream (`stream_events`, declared in `api/transports.json`: server-sent
 * events, resumed with `Last-Event-ID`), as the way the jobs stay current.
 *
 * An event is a notification, not content: it only says that something changed, and the jobs are
 * then asked again. Nothing shown is taken from an event.
 *
 * The stream is opened through the generated `streamEvents`, whose requests go through the
 * workspace's transport (`sendRequest`: a GET, so no CSRF header; the bounded wait covers the time
 * until the stream begins and never cuts it). While it is open the jobs are not asked on a clock.
 *
 * When it is not open the jobs are asked on a clock instead (`jobs.ts`):
 * - it could not be opened and a repeat cannot cure that (`not_implemented`, no permission):
 *   nothing more is tried, and the page says why;
 * - it could not be opened for a reason that may pass, or it ended: it is opened again after
 *   `RECONNECT_MS`, doubling up to `MAX_RECONNECT_MS`, from the last event it saw.
 *
 * A hidden page holds no stream: it is closed when the page is hidden and opened again, from the
 * last event seen, when the page is shown. Each time it opens again the jobs are asked once, which
 * covers whatever happened meanwhile.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { streamEvents } from '../../api/generated/sdk.gen';
import { zEvent } from '../../api/generated/zod.gen';
import { sendRequest } from '../../app/api-client';
import { classifyFailure, messageFor } from '../errors/failure';
import { jobsChanged } from './jobs';

/** How long after the stream ends or fails it is opened again, at first. */
export const RECONNECT_MS = 3_000;
/** The longest wait between two tries to open it. */
export const MAX_RECONNECT_MS = 30_000;

export type FeedState =
  /** Being opened. */
  | 'opening'
  /** Open: events arrive. */
  | 'live'
  /** Not open; it will be opened again. */
  | 'waiting'
  /** Not open, and it will not be tried again: a repeat cannot cure the failure. */
  | 'unavailable'
  /** The page is hidden: no stream is held. */
  | 'hidden';

export interface Feed {
  state: FeedState;
  /** Why it is not open, when a failure is the reason. */
  error: unknown;
}

interface FeedHooks {
  onState: (feed: Feed) => void;
  /** An event arrived: its JSON, or its text when it was not JSON. */
  onEvent: (data: unknown) => void;
}

/** A failure a repeat of the same request may cure. */
function mayPass(error: unknown): boolean {
  return messageFor(classifyFailure(error)).action.kind === 'retry';
}

/** What a refused request answered: the service's error when it sent one. */
async function refusalOf(response: Response): Promise<unknown> {
  const text = await response.text();
  try {
    return JSON.parse(text);
  } catch {
    return new Error(`The event stream answered ${response.status}`);
  }
}

function pause(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      signal.removeEventListener('abort', done);
      resolve();
    }
    signal.addEventListener('abort', done, { once: true });
  });
}

/**
 * Follow a workspace's events from `lastEventId` on. Returns how to stop, which answers the last
 * event identity seen, for the next time it is followed.
 */
export function followEvents(
  workspaceId: string,
  lastEventId: string | undefined,
  hooks: FeedHooks,
): () => string | undefined {
  const stopping = new AbortController();
  const { signal } = stopping;
  let seen = lastEventId;
  // Nothing is in flight and nothing will be tried: stopping then has nothing to abort.
  let over = false;

  // The transport, saying when the stream has begun and what a refusal was: the generated stream
  // reader reports a refused request by its status alone.
  const opening: typeof fetch = Object.assign(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const response = await sendRequest(input, init);
      if (!response.ok) throw await refusalOf(response);
      hooks.onState({ state: 'live', error: null });
      return response;
    },
    globalThis.fetch,
  );

  void (async () => {
    let wait = RECONNECT_MS;
    while (!signal.aborted) {
      let failure: unknown = null;
      let failed = false;
      try {
        const { stream } = await streamEvents({
          path: { workspace_id: workspaceId },
          signal,
          fetch: opening,
          ...(seen === undefined ? {} : { headers: { 'Last-Event-ID': seen } }),
          // One try a time: when to try again, and whether, is decided here.
          sseMaxRetryAttempts: 1,
          onSseError: (error) => {
            failed = true;
            failure = error;
          },
          onSseEvent: (event) => {
            if (event.id !== undefined) seen = event.id;
          },
        });
        for await (const data of stream) {
          wait = RECONNECT_MS;
          hooks.onEvent(data);
        }
      } catch (error) {
        failed = true;
        failure = error;
      }
      if (signal.aborted) return;
      if (failed && !mayPass(failure)) {
        over = true;
        hooks.onState({ state: 'unavailable', error: failure });
        return;
      }
      hooks.onState({ state: 'waiting', error: failed ? failure : null });
      await pause(wait, signal);
      wait = Math.min(wait * 2, MAX_RECONNECT_MS);
    }
  })();

  return () => {
    if (!over) stopping.abort();
    return seen;
  };
}

/** An event that says the workspace's jobs may have changed; one that cannot be read may too. */
function concernsJobs(data: unknown): boolean {
  const event = zEvent.safeParse(data);
  return !event.success || event.data.kind === 'job_updated' || event.data.kind === 'imported';
}

/**
 * Keep the workspace's jobs current from its event stream while the calling component is mounted
 * and the page is shown. `live` says the stream is open: the jobs are then not asked on a clock.
 */
export function useJobsFeed(workspaceId: string): Feed & { live: boolean } {
  const client = useQueryClient();
  const [feed, setFeed] = useState<Feed>({ state: 'opening', error: null });
  useEffect(() => {
    let stop: (() => string | undefined) | null = null;
    let lastEventId: string | undefined;
    let wasLive = false;
    const open = () => {
      setFeed({ state: 'opening', error: null });
      stop = followEvents(workspaceId, lastEventId, {
        onState: (next) => {
          setFeed(next);
          if (next.state !== 'live') return;
          // Open again after a time without it: whatever changed meanwhile is asked for once.
          // The first time, the jobs were asked when the component mounted.
          if (wasLive) jobsChanged(client, workspaceId);
          wasLive = true;
        },
        onEvent: (data) => {
          if (concernsJobs(data)) jobsChanged(client, workspaceId);
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
        setFeed({ state: 'hidden', error: null });
      } else if (stop === null) open();
    };
    visibility();
    document.addEventListener('visibilitychange', visibility);
    return () => {
      document.removeEventListener('visibilitychange', visibility);
      close();
    };
  }, [client, workspaceId]);
  return { ...feed, live: feed.state === 'live' };
}
