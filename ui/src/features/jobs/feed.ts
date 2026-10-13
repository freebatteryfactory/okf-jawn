/**
 * The workspace's event stream (`stream_events`, declared in `api/transports.json`: server-sent
 * events, resumed with `Last-Event-ID`), as the way the jobs stay current.
 *
 * An event is a notification, not content: it only says that something changed, and the jobs are
 * then asked again. Nothing shown is taken from an event.
 *
 * The stream is opened through the generated `streamEvents`, whose requests go through the
 * workspace's transport (`sendRequest`: a GET, so no CSRF header; the bounded wait covers the time
 * until the stream begins and never cuts it). It counts as open only when the answer is a success
 * whose media type is `text/event-stream`: a 200 that carries something else (a proxy's page, a
 * fallback document) is a failure to open it, tried again like one that may pass.
 *
 * Subscribe, then ask. Every time the stream opens, the first time included, the jobs are asked
 * once after it has opened: a job that changed between the answer on screen and the stream's
 * opening sent its event to nobody, and only a question finds it. While it is open the jobs are
 * asked on the slow clock of `jobs.ts` only (an event can be lost without the stream ending).
 *
 * When it is not open the jobs are asked on the quick clock instead (`jobs.ts`):
 * - it could not be opened and a repeat cannot cure that (`not_implemented`, no permission):
 *   nothing more is tried, and the page says why;
 * - it could not be opened for a reason that may pass, or it ended: it is opened again after
 *   `RECONNECT_MS`, doubling up to `MAX_RECONNECT_MS`, from the last event it saw.
 *
 * A session that ended. When the request for the stream is answered "not signed in"
 * (`unauthenticated`), nothing more is tried by itself: every try would be refused the same way,
 * for as long as the page is shown. The stream is asked for again when the person has acted and
 * the jobs were answered again (`wake`), or when the page is shown again after being hidden.
 *
 * A hidden page holds no stream: it is closed when the page is hidden and opened again, from the
 * last event seen, when the page is shown.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState } from 'react';
import { streamEvents } from '../../api/generated/sdk.gen';
import { zEvent } from '../../api/generated/zod.gen';
import { sendRequest } from '../../app/api-client';
import { classifyFailure, messageFor } from '../errors/failure';
import { jobsChanged, sessionEnded } from './jobs';

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

/**
 * A failure a repeat of the same request may cure by itself. "Not signed in" is not one: its
 * retry is the person's, after signing in.
 */
function mayPass(error: unknown): boolean {
  return !sessionEnded(error) && messageFor(classifyFailure(error)).action.kind === 'retry';
}

/** The media type of an event stream, as `api/transports.json` declares it for `stream_events`. */
const EVENT_STREAM = 'text/event-stream';

/** The request for the stream was answered as a success, with something that is not a stream. */
export class NotAnEventStream extends Error {
  constructor() {
    super('The answer to the event stream request was not an event stream');
    this.name = 'NotAnEventStream';
  }
}

/** A success whose body is an event stream: its media type says so, parameters aside. */
function isEventStream(response: Response): boolean {
  const type = response.headers.get('Content-Type') ?? '';
  return type.split(';').at(0)?.trim().toLowerCase() === EVENT_STREAM;
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
      if (!isEventStream(response)) {
        // Not a stream: nothing of it is read, and nothing is declared open.
        void response.body?.cancel();
        throw new NotAnEventStream();
      }
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
 * and the page is shown. `live` says the stream is open: the jobs are then asked on the slow clock
 * only. `wake` is for the caller to call when the jobs were answered again: a stream that stopped
 * because the session had ended is then asked for once more (and nothing happens otherwise).
 */
export function useJobsFeed(workspaceId: string): Feed & { live: boolean; wake: () => void } {
  const client = useQueryClient();
  const [feed, setFeed] = useState<Feed>({ state: 'opening', error: null });
  const reopen = useRef<() => void>(() => undefined);
  useEffect(() => {
    let stop: (() => string | undefined) | null = null;
    let lastEventId: string | undefined;
    // The stream stopped because the service said the person is not signed in.
    let signedOut = false;
    const open = () => {
      signedOut = false;
      setFeed({ state: 'opening', error: null });
      stop = followEvents(workspaceId, lastEventId, {
        onState: (next) => {
          setFeed(next);
          signedOut = next.state === 'unavailable' && sessionEnded(next.error);
          // Subscribe, then ask: what changed before the stream was open sent its event to
          // nobody, the first time as after a time without it.
          if (next.state === 'live') jobsChanged(client, workspaceId);
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
    reopen.current = () => {
      if (!signedOut || document.visibilityState === 'hidden') return;
      close();
      open();
    };
    visibility();
    document.addEventListener('visibilitychange', visibility);
    return () => {
      document.removeEventListener('visibilitychange', visibility);
      reopen.current = () => undefined;
      close();
    };
  }, [client, workspaceId]);
  const wake = useCallback(() => reopen.current(), []);
  return { ...feed, live: feed.state === 'live', wake };
}
