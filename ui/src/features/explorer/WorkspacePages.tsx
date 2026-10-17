/**
 * The workspace's own pages, beside its documents: bringing files in (`/import`), its jobs
 * (`/jobs`) and its Attention warnings (`/attention`). Each has a URL, which the router owns.
 *
 * A page is shown in place of the documents, between the same navigation and source columns. The
 * open tabs are kept in the Explorer store and are there again under "Documents", which goes back
 * to what the focused pane showed. The links are in the workspace's bar, which stays whatever is
 * shown, so following one never takes the element that has keyboard focus off the page. A page's
 * own heading is a level-two heading: the page's level-one heading stays the workspace's name.
 *
 * The shell is told of every press on a link of the bar (`onFollow`), before the router acts on
 * it: a link to the place the address already names is no navigation, so the router would not
 * tell it of that one (`ExplorerShell.tsx`, the naming rules).
 */
import { Link, useMatches } from '@tanstack/react-router';
import type { MouseEvent } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { AttentionLink } from '../attention/AttentionLink';
import { AttentionPage } from '../attention/AttentionPage';
import { ImportPage } from '../imports/ImportPage';
import { JobsPage } from '../jobs/JobsPage';
import { DocumentPanes, type DocumentPanesProps } from './DocumentPanes';
import { type Pane, tabLocation } from './store';

export type WorkspacePage = 'import' | 'jobs' | 'attention';

/**
 * The page the URL names; null when it names the documents.
 *
 * Read from the routes matched now, which change in the commit that renders a navigation. (The
 * router's "does this route match" answers from the location it has finished with, a commit later:
 * a page that left then would take keyboard focus with it after the router's own focus rule ran.)
 */
export function useWorkspacePage(): WorkspacePage | null {
  const route = useMatches({ select: (matches) => matches.at(-1)?.routeId });
  if (route === '/w/$workspaceId/import') return 'import';
  if (route === '/w/$workspaceId/jobs') return 'jobs';
  if (route === '/w/$workspaceId/attention') return 'attention';
  return null;
}

/** The documents, or the page shown in their place. */
export function DocumentsOrPage({
  page,
  ...documents
}: DocumentPanesProps & { page: WorkspacePage | null }) {
  if (page === null) return <DocumentPanes {...documents} />;
  const { workspaceId, head, canWrite } = documents;
  return (
    <div className="pane">
      {page === 'import' && (
        <ImportPage workspaceId={workspaceId} head={head} canWrite={canWrite} />
      )}
      {page === 'jobs' && <JobsPage workspaceId={workspaceId} />}
      {page === 'attention' && <AttentionPage workspaceId={workspaceId} head={head} />}
    </div>
  );
}

export interface WorkspaceNavProps {
  workspaceId: string;
  /** The revision the workspace shows: the Attention link counts the warnings found at it. */
  head: Revision;
  /** The workspace search the URL carries; every link keeps it. */
  query: string | undefined;
  /** The focused pane: "Documents" returns to its open item. */
  documents: Pane | null;
  /** A link of the bar was pressed; the router acts on the press after this. */
  onFollow?: ((event: MouseEvent<HTMLAnchorElement>) => void) | undefined;
}

/**
 * The links to the workspace's places. The link of the place on screen says so: the router sets
 * `aria-current="page"` on a link to the current URL.
 */
export function WorkspaceNav({ workspaceId, head, query, documents, onFollow }: WorkspaceNavProps) {
  const kept = query === undefined ? {} : { q: query };
  const params = { workspaceId };
  const shown = documents?.active ?? null;
  const read = documents === null ? {} : tabLocation(documents, shown);
  return (
    <nav aria-label="Workspace" className="workspace-nav">
      {shown === null ? (
        <Link
          to="/w/$workspaceId"
          params={params}
          search={kept}
          activeOptions={{ exact: true }}
          onClick={onFollow}
        >
          Documents
        </Link>
      ) : (
        <Link
          to="/w/$workspaceId/i/$itemId"
          params={{ workspaceId, itemId: shown }}
          search={{
            ...kept,
            ...(read.sel === undefined ? {} : { sel: read.sel }),
            ...(read.rev === undefined ? {} : { rev: read.rev }),
          }}
          activeOptions={{ exact: true }}
          onClick={onFollow}
        >
          Documents
        </Link>
      )}
      <Link to="/w/$workspaceId/import" params={params} search={kept} onClick={onFollow}>
        Import
      </Link>
      <Link to="/w/$workspaceId/jobs" params={params} search={kept} onClick={onFollow}>
        Jobs
      </Link>
      <AttentionLink workspaceId={workspaceId} head={head} search={kept} onFollow={onFollow} />
    </nav>
  );
}
