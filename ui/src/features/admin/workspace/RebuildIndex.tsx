/**
 * "Rebuild the search index" (`rebuild_index`, ruling C9): the workspace's derived search and link
 * data made again from its content, with its reviews and receipts kept. It needs `admin`, and
 * stays offered in an archived workspace, where the service runs it (`ALLOWED_WHILE_ARCHIVED`).
 *
 * The service answers 202 with a job, said as one: accepted as that job, in the state the service
 * reported then, never as an index already rebuilt (`acceptedWords`). The job goes into the cache
 * (`lifecycle.ts`), so the Jobs page lists it and follows it as it follows every job. One press is
 * one request, repeated under its key from its failure's notice until a job answers it
 * (`useExportStart`); the next press is a new rebuild. The button keeps keyboard focus while the
 * request is out (`aria-disabled`).
 */
import { useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import { FailureNotice } from '../../errors/FailureNotice';
import { allowedOperation, archivedWords, Missing } from '../../explorer/rights';
import { acceptedWords, useExportStart } from '../../exports/ExportStart';
import { useWorkspaceChanges } from '../../tree/context';
import { type Rebuild, startRebuild } from './lifecycle';

export function RebuildIndex() {
  const { workspaceId, rights } = useWorkspaceChanges();
  const client = useQueryClient();
  const headingId = useId();
  const aboutId = useId();
  const archivedId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const start = useExportStart<Rebuild>({
    request: workspaceId,
    start: (attempt) => startRebuild(client, attempt),
    loaded: () => buttonRef.current,
  });
  const can = allowedOperation(rights, 'rebuild_index', 'admin');
  const said =
    start.pending && !start.failure.retrying
      ? 'Starting the rebuild…'
      : start.started === null
        ? ''
        : `${acceptedWords('The rebuild', start.started)} The Jobs page lists it.`;
  return (
    <section aria-labelledby={headingId}>
      <h4 id={headingId} className="mt-3 font-bold">
        Search index
      </h4>
      {can === 'no-permission' ? (
        <Missing action="Rebuilding the search index" permission="admin" />
      ) : (
        <>
          <p id={aboutId} className="meta">
            Rebuilding makes the search and link data again from the workspace’s content; reviews
            and receipts are kept. It runs as a job.
          </p>
          <p>
            {/* aria-disabled rather than disabled: the button keeps focus while the rebuild starts. */}
            <button
              type="button"
              ref={buttonRef}
              aria-disabled={start.pending || can === 'archived'}
              aria-describedby={can === 'archived' ? `${archivedId} ${aboutId}` : aboutId}
              onClick={() => {
                if (can === 'yes') start.begin({ workspaceId });
              }}
            >
              {start.pending ? 'Starting the rebuild…' : 'Rebuild the search index'}
            </button>
          </p>
          {can === 'archived' && rights.archivedAt !== null && (
            <p id={archivedId} className="meta">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          {/* Present from the start, so what pressing did is announced. */}
          <p role="status" className="meta">
            {said}
          </p>
          {start.started !== null && (
            <p>
              <Link to="/w/$workspaceId/jobs" params={{ workspaceId }}>
                Open the Jobs page
              </Link>
            </p>
          )}
          {start.failure.shown && (
            <FailureNotice
              context="Starting the rebuild"
              error={start.failure.error}
              retry={start.failure}
              workspaceId={workspaceId}
            />
          )}
        </>
      )}
    </section>
  );
}
