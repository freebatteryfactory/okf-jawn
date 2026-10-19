/**
 * Who changed each line: for each line of an item's text at one revision, the version that last
 * changed it (`blame_item`). It says who changed a line, not who first wrote what the line says
 * (SPEC section 3: "Attribution says who changed a passage, not who originated every fact"), and
 * a version's author is the label it carries, not a verified person.
 *
 * The lines are those of the text without its properties header, numbered as the service numbers
 * them, from the text `get_item` answered at the same revision; an item with no text asks
 * nothing. They are asked `WHO_PAGE` at a time; "Show the next lines" asks for the next ones and
 * moves keyboard focus to the first line it brought. A run of lines last changed in the same
 * version names it once, and each later line of the run says it is the same version as the line
 * above, so the table reads as runs and not as a column of repeats.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useEffect, useId, useRef } from 'react';
import type { BlameLine, Revision } from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { whoOptions } from './history';

export interface WhoProps {
  workspaceId: string;
  itemId: string;
  revision: Revision;
  /** How many lines the item's text has at that revision. */
  lines: number;
}

export function Who({ workspaceId, itemId, revision, lines }: WhoProps) {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const bodyRef = useRef<HTMLTableSectionElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const who = useInfiniteQuery({
    ...whoOptions(workspaceId, itemId, revision, lines),
    enabled: lines > 0,
  });
  const answered: BlameLine[] = (who.data?.pages ?? []).flatMap((page) => page.lines);
  const rowAt = (index: number) =>
    bodyRef.current?.querySelectorAll<HTMLElement>(':scope > tr').item(index) ?? null;
  const nothingListed = who.data === undefined;
  const firstFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: who.isError && nothingListed,
    error: who.error,
    run: () => who.refetch(),
    loaded: () => rowAt(0) ?? headingRef.current,
  });
  const moreFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: who.isError && !nothingListed,
    error: who.error,
    run: () => who.fetchNextPage(),
    loaded: () => rowAt(answered.length - 1),
  });
  const count = answered.length;
  const focusFrom = useRef<number | null>(null);
  useEffect(() => {
    const from = focusFrom.current;
    if (from === null || who.isFetchingNextPage) return;
    if (count <= from && !who.isFetchNextPageError && who.hasNextPage) return;
    focusFrom.current = null;
    (rowAt(from) ?? moreRef.current ?? rowAt(count - 1))?.focus();
  });

  return (
    <section className="who" aria-labelledby={headingId}>
      <h3 id={headingId} ref={headingRef} tabIndex={-1}>
        Who changed each line
      </h3>
      <p className="meta">
        Each line of the text at revision <code title={revision}>{short(revision)}</code>, with the
        version that last changed it: who changed a line, not who first wrote what it says.
      </p>
      {lines === 0 && <p>This item has no text, so no line has a version to show.</p>}
      {who.isPending && lines > 0 && !firstFailure.retrying && (
        <p role="status">Reading who changed each line…</p>
      )}
      {firstFailure.shown && (
        <FailureNotice
          context="Reading who changed each line"
          error={firstFailure.error}
          retry={firstFailure}
          workspaceId={workspaceId}
        />
      )}
      {answered.length > 0 && (
        <table className="who-lines">
          <thead>
            <tr>
              <th scope="col">Line</th>
              <th scope="col">Text</th>
              <th scope="col">Last changed in</th>
            </tr>
          </thead>
          <tbody ref={bodyRef}>
            {answered.map((line, index) => {
              const same = answered.at(index - 1)?.commit.revision === line.commit.revision;
              return (
                <tr key={line.line} tabIndex={-1}>
                  <td>{line.line}</td>
                  <td className="who-text">{line.text}</td>
                  <td>
                    {index > 0 && same ? (
                      <span className="meta">the same version as the line above</span>
                    ) : (
                      <>
                        <strong>{line.commit.message || '(no description)'}</strong>{' '}
                        <span className="meta">
                          <Instant value={line.commit.committed_at} />, author recorded as{' '}
                          {line.commit.author}, revision{' '}
                          <code title={line.commit.revision}>{short(line.commit.revision)}</code>
                        </span>
                      </>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Reading more lines"
          error={moreFailure.error}
          retry={moreFailure}
          workspaceId={workspaceId}
        />
      )}
      {who.hasNextPage && (
        <button
          type="button"
          ref={moreRef}
          aria-disabled={who.isFetchingNextPage}
          onClick={() => {
            if (who.isFetchingNextPage) return;
            focusFrom.current = count;
            void who.fetchNextPage();
          }}
        >
          {who.isFetchingNextPage ? 'Reading the next lines…' : 'Show the next lines'}
        </button>
      )}
    </section>
  );
}
