/**
 * "Export this workspace", on the workspace's Jobs page, where the job it starts is then listed
 * (`export_workspace`, ruling B3).
 *
 * The export is of the workspace as it is now: its latest version, resolved once by the service
 * when it takes the request, and the page says that this may be newer than the version it shows.
 * Its history is left out unless the person asks for it (`include_history`). It is offered to a
 * person who may write (the operation's permission) and stays offered in an archived workspace,
 * where the service runs it too (`allowedOperation`); a person who may not is told why, in its
 * place. Were the archive to stop it (the contract's list changed), the button would stay, say why
 * it cannot run, and send nothing, as every action of an archived workspace does.
 *
 * It is one group of the page, named by its legend rather than by a heading, so the page's
 * headings stay "Jobs" over the list. The button is always there while offered (`aria-disabled`
 * while the request is out), so it keeps keyboard focus. What came of a press is said in the group
 * (`ExportSaid`); the job is listed below as every job is, and once it has finished its row offers
 * the download.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useId, useRef, useState } from 'react';
import { allowedOperation, archivedWords, Missing, type Rights } from '../explorer/rights';
import { ExportSaid, useExportStart } from './ExportStart';
import { startWorkspaceExport, type WorkspaceExport } from './exports';

export interface ExportWorkspaceProps {
  workspaceId: string;
  /** The person's permissions here, and whether the workspace is archived. */
  rights: Rights;
}

export function ExportWorkspace({ workspaceId, rights }: ExportWorkspaceProps) {
  const client = useQueryClient();
  const aboutId = useId();
  const historyHintId = useId();
  const archivedId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [includeHistory, setIncludeHistory] = useState(false);
  const can = allowedOperation(rights, 'export_workspace', 'write');
  const stopped = can === 'archived';
  const start = useExportStart<WorkspaceExport>({
    request: workspaceId,
    start: (attempt) => startWorkspaceExport(client, workspaceId, attempt),
    loaded: () => buttonRef.current,
  });

  if (can === 'no-permission') {
    return <Missing action="Exporting this workspace" permission="write" />;
  }
  return (
    <form
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        if (!stopped) start.begin({ includeHistory });
      }}
    >
      <fieldset className="grid justify-items-start gap-1 rounded border border-[color:GrayText] px-3 py-2">
        <legend className="px-1 font-bold">Export this workspace</legend>
        <p id={aboutId} className="meta">
          The export holds the workspace as it is now: its latest version when the service takes the
          request, which can be newer than the version this page shows. It holds each item with the
          originals and converted files it refers to; drafts, sessions and credentials are never
          included. Once its job below has finished, its row offers the download.
        </p>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={includeHistory}
            aria-describedby={historyHintId}
            onChange={(event) => setIncludeHistory(event.target.checked)}
          />
          Include the workspace’s history
        </label>
        <p id={historyHintId} className="meta">
          Every earlier version as well, not only the latest.
        </p>
        {/* aria-disabled rather than disabled: the button keeps focus while the export starts. */}
        <button
          type="submit"
          ref={buttonRef}
          aria-disabled={start.pending || stopped}
          aria-describedby={stopped ? `${archivedId} ${aboutId}` : aboutId}
        >
          {start.pending ? 'Starting the export…' : 'Export the workspace'}
        </button>
        {stopped && rights.archivedAt !== null && (
          <p id={archivedId} className="meta">
            {archivedWords(rights.archivedAt)}
          </p>
        )}
        <ExportSaid
          start={start}
          what="The export"
          where="Its row is in the list below."
          context="Starting the export"
          workspaceId={workspaceId}
        />
      </fieldset>
    </form>
  );
}
