/**
 * One workspace: the Explorer shell over the workspace head the server resolved.
 *
 * The workspace id in the path is checked with the generated schema before anything is sent: a
 * malformed one gets a plain-language message and no request.
 *
 * The head is re-resolved whenever the window regains focus, and on Refresh; the shell shows it
 * "as of" when it was resolved. A failed refresh keeps the Explorer and says so there.
 */
import { useQuery } from '@tanstack/react-query';
import { createFileRoute, Link } from '@tanstack/react-router';
import { openWorkspaceOptions } from '../api/generated/@tanstack/react-query.gen';
import { zWorkspaceId } from '../api/generated/zod.gen';
import { FailureNotice } from '../features/errors/FailureNotice';
import { ExplorerShell } from '../features/explorer/ExplorerShell';
import { validateWorkspaceSearch } from '../features/explorer/search-params';

export const Route = createFileRoute('/w/$workspaceId')({
  validateSearch: validateWorkspaceSearch,
  component: WorkspaceRoute,
});

function WorkspaceRoute() {
  const { workspaceId } = Route.useParams();
  const valid = zWorkspaceId.safeParse(workspaceId).success;
  const workspace = useQuery({
    ...openWorkspaceOptions({ body: { workspace_id: workspaceId } }),
    enabled: valid,
    staleTime: 0,
    refetchOnWindowFocus: true,
  });
  if (!valid) {
    return (
      <div className="page-status">
        <div role="alert" className="failure-notice">
          <p>
            <strong>This link is not valid.</strong>
          </p>
          <p>The address does not name a workspace. Nothing was requested.</p>
          <Link to="/">Go to your workspaces</Link>
        </div>
      </div>
    );
  }
  if (workspace.data === undefined) {
    if (workspace.isError) {
      return (
        <div className="page-status">
          <FailureNotice
            context="Opening this workspace"
            error={workspace.error}
            onRetry={() => void workspace.refetch()}
          />
        </div>
      );
    }
    return (
      <p role="status" className="page-status">
        Opening the workspace…
      </p>
    );
  }
  return (
    <ExplorerShell
      workspace={workspace.data}
      head={{
        resolvedAt: workspace.dataUpdatedAt,
        refreshing: workspace.isFetching,
        refreshError: workspace.isError ? workspace.error : null,
        refresh: async () => {
          const result = await workspace.refetch();
          return result.isError ? null : (result.data?.head ?? null);
        },
      }}
    />
  );
}
