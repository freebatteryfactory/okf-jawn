/** One workspace: the Explorer shell, pinned to the workspace head the server resolved. */
import { useQuery } from '@tanstack/react-query';
import { createFileRoute } from '@tanstack/react-router';
import { openWorkspaceOptions } from '../api/generated/@tanstack/react-query.gen';
import { FailureNotice } from '../features/errors/FailureNotice';
import { ExplorerShell } from '../features/explorer/ExplorerShell';
import { validateWorkspaceSearch } from '../features/explorer/search-params';

export const Route = createFileRoute('/w/$workspaceId')({
  validateSearch: validateWorkspaceSearch,
  component: WorkspaceRoute,
});

function WorkspaceRoute() {
  const { workspaceId } = Route.useParams();
  const workspace = useQuery(openWorkspaceOptions({ body: { workspace_id: workspaceId } }));
  if (workspace.isPending) {
    return (
      <p role="status" className="page-status">
        Opening the workspace…
      </p>
    );
  }
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
  return <ExplorerShell workspace={workspace.data} />;
}
