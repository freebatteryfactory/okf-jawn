/**
 * "Export visual" for one View (`export_view`, ruling B3): its specification, its sources, its
 * data table and its rendering assets, at the revision its tab reads (`revision`), never at the
 * latest. Built to stand by itself; a View's tab holds it.
 *
 * It is offered to a person who may write (the operation's permission) and stays offered in an
 * archived workspace, where the service runs it too (`allowedOperation`); a person who may not is
 * told why. Were the archive to stop it (the contract's list changed), the button would stay, say
 * why it cannot run, and send nothing. The button is always there while offered (`aria-disabled`
 * while the request is out), so it keeps keyboard focus. What came of a press is said beside it
 * (`ExportSaid`), with a link to the Jobs page, where the job is listed as every job is and where
 * its row offers the download once it has finished.
 */
import { useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { short } from '../documents/describe';
import { allowedOperation, archivedWords, Missing, type Rights } from '../explorer/rights';
import { ExportSaid, useExportStart } from './ExportStart';
import { startViewExport, type ViewExport } from './exports';

export interface ExportViewButtonProps {
  workspaceId: string;
  /** The View item. */
  itemId: string;
  /** The revision the View's tab reads: the export is of the View at it. */
  revision: Revision;
  /** The person's permissions here, and whether the workspace is archived. */
  rights: Rights;
}

export function ExportViewButton({ workspaceId, itemId, revision, rights }: ExportViewButtonProps) {
  const client = useQueryClient();
  const aboutId = useId();
  const archivedId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const can = allowedOperation(rights, 'export_view', 'write');
  const stopped = can === 'archived';
  const start = useExportStart<ViewExport>({
    request: `${itemId}@${revision}`,
    start: (attempt) => startViewExport(client, workspaceId, attempt),
    loaded: () => buttonRef.current,
  });

  if (can === 'no-permission') {
    return <Missing action="Exporting this View" permission="write" />;
  }
  return (
    <div className="grid justify-items-start gap-1">
      {/* aria-disabled rather than disabled: the button keeps focus while the export starts. */}
      <button
        type="button"
        ref={buttonRef}
        aria-disabled={start.pending || stopped}
        aria-describedby={stopped ? `${archivedId} ${aboutId}` : aboutId}
        onClick={() => {
          if (!stopped) start.begin({ itemId, revision });
        }}
      >
        {start.pending ? 'Starting the export…' : 'Export visual'}
      </button>
      {stopped && rights.archivedAt !== null && (
        <p id={archivedId} className="meta">
          {archivedWords(rights.archivedAt)}
        </p>
      )}
      <p id={aboutId} className="meta">
        Exports this View as of revision {short(revision)}: its specification, its sources, its data
        table and what it needs to be drawn. The export is a job; the Jobs page shows how it goes.
      </p>
      <ExportSaid
        start={start}
        what="The export of this View"
        context="Starting the export of this View"
        workspaceId={workspaceId}
      />
      {start.started !== null && (
        <p>
          <Link className="underline" to="/w/$workspaceId/jobs" params={{ workspaceId }}>
            All jobs of this workspace
          </Link>
        </p>
      )}
    </div>
  );
}
