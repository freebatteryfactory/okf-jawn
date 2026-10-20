/**
 * One event log as Administration reads it (ruling C8): its security events listed in words, the
 * other events counted, page by page as the person asks (`events.ts`).
 *
 * Only security events are listed: the section is about who signed in or was refused, which
 * connectors were issued or revoked and which requests were refused, and a workspace's log is
 * mostly its content changes and its jobs' progress, which would bury them. Nothing is dropped
 * silently: the events of every other kind on the pages listed are counted by kind, in words, and
 * the caller says where they are shown (History, Jobs). A kind this build does not know is
 * counted the same way, as a kind this version does not know.
 *
 * Each event says what happened, when, who caused it (subject, the route in words, the client),
 * what was refused (an operation, or the byte transport of a refused transfer, in words), what it
 * touched, and how many occurrences an aggregate stands for (`words.ts`).
 *
 * "Show more events" asks the next page by the service's continuation; once a page had none, the
 * button asks for events newer than the last one listed ("Look for newer events"). Either moves
 * keyboard focus to the first security event it brought, else leaves it on the button, and a
 * polite status says what came. A first page that failed is a failure notice with its retry in
 * place of the list; a later one keeps the events listed, with its notice and a retry from the
 * same place (`useRetry`). A refusal is said in the caller's words when it gives some
 * (`refusal`), with what it allows.
 */
import type { UseInfiniteQueryResult } from '@tanstack/react-query';
import { type RefObject, useEffect, useId, useMemo, useRef, useState } from 'react';
import type { Event } from '../../../api/generated/types.gen';
import { Instant } from '../../../lib/instant';
import { FailureNotice } from '../../errors/FailureNotice';
import { classifyFailure, type FailureMessage } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import { atEnd, EVENTS_PAGE, type EventPages } from './events';
import {
  countWords,
  isSecurity,
  kindWords,
  othersWords,
  refusedWords,
  touchedWords,
  whoWords,
} from './words';

export interface EventLogProps {
  /** The log, as `useInfiniteQuery` reads it. */
  events: UseInfiniteQueryResult<EventPages>;
  /** The section's heading: it names the list, and takes focus when nothing else is there. */
  headingId: string;
  headingRef: RefObject<HTMLHeadingElement | null>;
  /** What is listed, as text that changes when it does (`useRetry`'s `request`). */
  request: string;
  /** What the person is doing, for a failure notice: "Listing this workspace’s security events". */
  context: string;
  /** Where the events of other kinds are shown, said after their count. */
  elsewhere: string;
  /** Words of this place for the service's refusal (`forbidden`), with what it allows. */
  refusal?: FailureMessage;
  /** The workspace whose log this is: an event is not said to concern it again. */
  workspaceId?: string;
}

/** The caller's words for a refusal, and the general words for any other failure. */
function wordsOf(error: unknown, refusal: FailureMessage | undefined): FailureMessage | undefined {
  const failure = classifyFailure(error);
  return failure.kind === 'api' && failure.error.code === 'forbidden' ? refusal : undefined;
}

/** What a request for more brought, said politely once it has answered. */
function broughtWords(brought: number, newer: boolean): string {
  if (brought > 0) {
    return brought === 1
      ? '1 more security event listed.'
      : `${brought} more security events listed.`;
  }
  return newer ? 'No newer events were recorded.' : 'The next events held no security event.';
}

export function EventLog({
  events,
  headingId,
  headingRef,
  request,
  context,
  elsewhere,
  refusal,
  workspaceId,
}: EventLogProps) {
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const [said, setSaid] = useState('');
  const pages = events.data;
  const all = useMemo(() => (pages?.pages ?? []).flatMap((page) => page.events), [pages]);
  const shown = useMemo(() => all.filter(isSecurity), [all]);
  const ended = atEnd(pages);

  const rowAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>(':scope > li').item(index) ?? null;
  const nothingListed = pages === undefined;
  const first = useRetry({
    request,
    failed: events.isError && nothingListed,
    error: events.error,
    run: () => events.refetch(),
    loaded: () => rowAt(0) ?? moreRef.current ?? headingRef.current,
  });
  const later = useRetry({
    request,
    failed: events.isFetchNextPageError,
    error: events.error,
    run: () => events.fetchNextPage(),
    loaded: () => moreRef.current ?? rowAt(0) ?? headingRef.current,
  });

  // A request for more moves keyboard focus to the first security event it brought, else leaves
  // it on the button, and says what came once it has answered.
  const asked = useRef<{ shown: number; pages: number; newer: boolean } | null>(null);
  const pageCount = pages?.pages.length ?? 0;
  const loadingMore = events.isFetchingNextPage;
  const moreFailed = events.isFetchNextPageError;
  const shownCount = shown.length;
  useEffect(() => {
    const before = asked.current;
    if (before === null || !(pageCount > before.pages || (moreFailed && !loadingMore))) return;
    asked.current = null;
    if (pageCount > before.pages) setSaid(broughtWords(shownCount - before.shown, before.newer));
    (rowAt(before.shown) ?? moreRef.current)?.focus();
  });

  const others = othersWords(all);
  return (
    <>
      <p className="meta">
        Listed oldest first, as the service records them, {EVENTS_PAGE} events at a time.
      </p>
      {events.isPending && !first.retrying && <p role="status">Listing the events…</p>}
      {first.shown && (
        <FailureNotice
          context={context}
          error={first.error}
          retry={first}
          words={wordsOf(first.error, refusal)}
          {...(workspaceId === undefined ? {} : { workspaceId })}
        />
      )}
      {pages !== undefined && shown.length === 0 && (
        <p>
          {all.length === 0
            ? 'No event has been recorded yet.'
            : `No security event is among the ${all.length} ${all.length === 1 ? 'event' : 'events'} listed.`}
        </p>
      )}
      {shown.length > 0 && (
        <ul ref={listRef} className="job-list" aria-labelledby={headingId}>
          {shown.map((event) => (
            <EventRow key={event.id} event={event} workspaceId={workspaceId} />
          ))}
        </ul>
      )}
      {others !== '' && (
        <p className="meta">
          {others} {elsewhere}
        </p>
      )}
      {later.shown && (
        <FailureNotice
          context={`${context} further`}
          error={later.error}
          retry={later}
          words={wordsOf(later.error, refusal)}
          {...(workspaceId === undefined ? {} : { workspaceId })}
        />
      )}
      {pages !== undefined && ended && (
        <p className="meta">That is every event recorded when the last page was read.</p>
      )}
      {pages !== undefined && (
        <p>
          {/* aria-disabled rather than disabled: the button keeps focus while more are asked. */}
          <button
            type="button"
            ref={moreRef}
            aria-disabled={loadingMore}
            onClick={() => {
              if (loadingMore) return;
              asked.current = { shown: shownCount, pages: pageCount, newer: ended };
              setSaid('');
              void events.fetchNextPage();
            }}
          >
            {ended
              ? loadingMore
                ? 'Looking for newer events…'
                : 'Look for newer events'
              : loadingMore
                ? 'Listing more events…'
                : 'Show more events'}
          </button>
        </p>
      )}
      {/* Present from the start, so what a request for more brought is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
    </>
  );
}

function EventRow({ event, workspaceId }: { event: Event; workspaceId: string | undefined }) {
  const titleId = useId();
  const who = whoWords(event);
  const refused = refusedWords(event);
  const touched = touchedWords(event, workspaceId);
  const count = countWords(event);
  return (
    <li tabIndex={-1} className="job" aria-labelledby={titleId} data-event={event.id}>
      <p id={titleId} className="job-title">
        {kindWords(event.kind)}
      </p>
      <dl className="m-0 grid grid-cols-[10rem_1fr] gap-x-3 gap-y-0.5 [&_dd]:m-0">
        <dt>When</dt>
        <dd>
          <Instant value={event.at} />
        </dd>
        {who !== null && (
          <>
            <dt>Who</dt>
            <dd>{who}</dd>
          </>
        )}
        {refused !== null && (
          <>
            <dt>What was refused</dt>
            <dd>{refused}</dd>
          </>
        )}
        {touched !== null && (
          <>
            <dt>What it concerned</dt>
            <dd>{touched}</dd>
          </>
        )}
        {count !== null && (
          <>
            <dt>How many</dt>
            <dd>{count}</dd>
          </>
        )}
      </dl>
    </li>
  );
}
