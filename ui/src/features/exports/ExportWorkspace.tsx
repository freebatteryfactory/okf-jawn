/**
 * "Export this workspace", on the workspace's Jobs page, where the job it starts is then listed
 * (`export_workspace`, ruling B3).
 *
 * The person chooses the version and whether to include history. By default the export is of the
 * workspace as it is now: its latest version, resolved once by the service when it takes the
 * request, and the page says that this may be newer than the version it shows. The other choice is
 * a version of its history, picked from the versions of the head the page shows (M3's Timeline
 * reading, `history.ts` `logOptions`, asked only once that choice is made), and exported at its
 * resolved revision whatever has changed since. A listing that fails is said with its retry: the
 * first page in place of the list, a later page under the versions already listed, which stay, and
 * its retry asks again from the same place. Its history is left out unless the person asks
 * for it (`include_history`). It is offered to a person who may write (the operation's permission)
 * and stays offered in an archived workspace, where the service runs it too (`allowedOperation`); a
 * person who may not is told why, in its place. Were the archive to stop it (the contract's list
 * changed), the button would stay, say why it cannot run, and send nothing, as every action of an
 * archived workspace does.
 *
 * It is one group of the page, named by its legend rather than by a heading, so the page's
 * headings stay "Jobs" over the list. The button is always there while offered (`aria-disabled`
 * while the request is out, or while the chosen version is not yet listed), so it keeps keyboard
 * focus. What came of a press is said in the group (`ExportSaid`); the job is listed below as every
 * job is, and once it has finished its row offers the download. What is asked is the whole
 * request, the version and the choice of history: a press made for another (a failure, a press still
 * out, an accepted export) is not said or retried while this one is chosen, and each keeps its
 * own key, so coming back to one never answered asks again under it (`useExportStart`).
 */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { useId, useRef, useState } from 'react';
import type { At, Revision } from '../../api/generated/types.gen';
import { formatInstant } from '../../lib/instant';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowedOperation, archivedWords, Missing, type Rights } from '../explorer/rights';
import { logOptions } from '../history/history';
import { ExportSaid, useExportStart } from './ExportStart';
import { startWorkspaceExport, type WorkspaceExport } from './exports';

export interface ExportWorkspaceProps {
  workspaceId: string;
  /** The head the page shows: the versions offered are its history. */
  head: Revision;
  /** The person's permissions here, and whether the workspace is archived. */
  rights: Rights;
}

/** The first line of a commit message: what a version is called in the list. */
function firstLine(message: string): string {
  return message.split('\n', 1)[0]?.trim() ?? '';
}

export function ExportWorkspace({ workspaceId, head, rights }: ExportWorkspaceProps) {
  const client = useQueryClient();
  const aboutId = useId();
  const historyHintId = useId();
  const archivedId = useId();
  const versionId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const listRef = useRef<HTMLSelectElement | null>(null);
  const [includeHistory, setIncludeHistory] = useState(false);
  const [which, setWhich] = useState<'now' | 'version'>('now');
  const [picked, setPicked] = useState<Revision | null>(null);
  const can = allowedOperation(rights, 'export_workspace', 'write');
  const stopped = can === 'archived';
  // The versions of the head shown, asked only once a version of the history is wanted.
  const log = useInfiniteQuery({
    ...logOptions(workspaceId, head, null),
    enabled: which === 'version' && can !== 'no-permission',
  });
  const versions = (log.data?.pages ?? []).flatMap((page) => page.commits);
  // The version picked while it is listed, else the newest.
  const chosen = versions.find((version) => version.revision === picked) ?? versions.at(0) ?? null;
  const listing = useRetry({
    request: `${workspaceId} ${head}`,
    failed: which === 'version' && log.isError && versions.length === 0,
    error: log.error,
    run: () => log.refetch(),
    loaded: () => listRef.current,
  });
  // A later page that failed: the versions listed stay; its retry asks from the same cursor.
  const older = useRetry({
    request: `${workspaceId} ${head} after ${versions.length}`,
    failed: which === 'version' && log.isFetchNextPageError && versions.length > 0,
    error: log.error,
    run: () => log.fetchNextPage(),
    loaded: () => listRef.current,
  });
  const at: At | null =
    which === 'now'
      ? { kind: 'latest' }
      : chosen === null
        ? null
        : { kind: 'revision', revision: chosen.revision };
  const start = useExportStart<WorkspaceExport>({
    // The whole request: another version or choice of history is another request.
    request: JSON.stringify([workspaceId, at, includeHistory]),
    start: (attempt) => startWorkspaceExport(client, workspaceId, attempt),
    loaded: () => buttonRef.current,
  });

  if (can === 'no-permission') {
    return <Missing action="Exporting this workspace" permission="write" />;
  }
  const committed = chosen === null ? null : formatInstant(chosen.committed_at);
  return (
    <form
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        if (!stopped && at !== null) start.begin({ at, includeHistory });
      }}
    >
      <fieldset className="grid justify-items-start gap-1 rounded border border-[color:GrayText] px-3 py-2">
        <legend className="px-1 font-bold">Export this workspace</legend>
        <fieldset className="grid justify-items-start gap-1">
          <legend className="font-bold">Which version</legend>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name={versionId}
              checked={which === 'now'}
              onChange={() => setWhich('now')}
            />
            As it is now
          </label>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name={versionId}
              checked={which === 'version'}
              onChange={() => setWhich('version')}
            />
            A version from its history
          </label>
          {which === 'version' && versions.length > 0 && (
            <label className="grid gap-1">
              Version
              <select
                ref={listRef}
                value={chosen?.revision ?? ''}
                onChange={(event) => setPicked(event.target.value)}
              >
                {versions.map((version) => (
                  <option key={version.revision} value={version.revision}>
                    {`${short(version.revision)} · ${firstLine(version.message)} · ${formatInstant(version.committed_at) ?? version.committed_at}`}
                  </option>
                ))}
              </select>
            </label>
          )}
          {which === 'version' && log.hasNextPage && (
            // aria-disabled rather than disabled: the button keeps focus while more are listed.
            <button
              type="button"
              aria-disabled={log.isFetchingNextPage}
              onClick={() => {
                if (!log.isFetchingNextPage) void log.fetchNextPage();
              }}
            >
              Show older versions
            </button>
          )}
          {which === 'version' && log.isPending && !listing.retrying && (
            <p role="status">Listing the versions of this workspace…</p>
          )}
          {which === 'version' && listing.shown && (
            <FailureNotice
              context="Listing the versions of this workspace"
              error={listing.error}
              retry={listing}
              workspaceId={workspaceId}
            />
          )}
          {which === 'version' && older.shown && (
            <FailureNotice
              context="Listing older versions of this workspace"
              error={older.error}
              retry={older}
              workspaceId={workspaceId}
            />
          )}
        </fieldset>
        <p id={aboutId} className="meta">
          {which === 'now' || chosen === null
            ? 'The export holds the workspace as it is now: its latest version when the service takes the request, which can be newer than the version this page shows.'
            : `The export holds the workspace as of revision ${short(chosen.revision)}${committed === null ? '' : `, committed ${committed}`}: that version, whatever has changed since.`}{' '}
          It holds each item with the originals and converted files it refers to; drafts, sessions
          and credentials are never included. Once its job below has finished, its row offers the
          download.
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
          Every earlier version as well, not only the one exported.
        </p>
        {/* aria-disabled rather than disabled: the button keeps focus while the export starts. */}
        <button
          type="submit"
          ref={buttonRef}
          aria-disabled={start.pending || stopped || at === null}
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
          where="Its row in the list below shows how it stands now."
          context="Starting the export"
          workspaceId={workspaceId}
        />
      </fieldset>
    </form>
  );
}
