/**
 * The workspace's own pages, beside its documents: bringing files in (`/import`). Each has a URL,
 * which the router owns.
 *
 * A page is shown in place of the documents, between the same navigation and source columns. The
 * open tabs are kept in the Explorer store and are there again under "Documents", which goes back
 * to what the focused pane showed. The links are in the workspace's bar, which stays whatever is
 * shown, so following one never takes the element that has keyboard focus off the page. A page's
 * own heading is a level-two heading: the page's level-one heading stays the workspace's name.
 */
import { Link, useMatchRoute } from '@tanstack/react-router';
import { ImportPage } from '../imports/ImportPage';
import { DocumentPanes, type DocumentPanesProps } from './DocumentPanes';
import { type Pane, tabLocation } from './store';

export type WorkspacePage = 'import';

/** The page the URL names; null when it names the documents. */
export function useWorkspacePage(): WorkspacePage | null {
  const matchRoute = useMatchRoute();
  if (matchRoute({ to: '/w/$workspaceId/import' }) !== false) return 'import';
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
      <ImportPage workspaceId={workspaceId} head={head} canWrite={canWrite} />
    </div>
  );
}

export interface WorkspaceNavProps {
  workspaceId: string;
  /** The workspace search the URL carries; every link keeps it. */
  query: string | undefined;
  /** The focused pane: "Documents" returns to its open item. */
  documents: Pane | null;
}

/**
 * The links to the workspace's places. The link of the place on screen says so: the router sets
 * `aria-current="page"` on a link to the current URL.
 */
export function WorkspaceNav({ workspaceId, query, documents }: WorkspaceNavProps) {
  const kept = query === undefined ? {} : { q: query };
  const params = { workspaceId };
  const shown = documents?.active ?? null;
  const read = documents === null ? {} : tabLocation(documents, shown);
  return (
    <nav aria-label="Workspace" className="workspace-nav">
      {shown === null ? (
        <Link to="/w/$workspaceId" params={params} search={kept} activeOptions={{ exact: true }}>
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
        >
          Documents
        </Link>
      )}
      <Link to="/w/$workspaceId/import" params={params} search={kept}>
        Import
      </Link>
    </nav>
  );
}
