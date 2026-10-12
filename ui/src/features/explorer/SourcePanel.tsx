/** The source panel: the open item's citations and appearance from the sources operation. */
import { useQuery } from '@tanstack/react-query';
import { getSourcesOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { Revision } from '../../api/generated/types.gen';
import { Sources } from '../documents/Sources';
import { FailureNotice } from '../errors/FailureNotice';

export interface SourcePanelProps {
  workspaceId: string;
  itemId: string | null;
  revision: Revision;
}

export function SourcePanel({ workspaceId, itemId, revision }: SourcePanelProps) {
  if (itemId === null) {
    return (
      <div className="source-panel">
        <h2 className="panel-heading">Sources</h2>
        <p>Open an item to see what it cites and where its text comes from.</p>
      </div>
    );
  }
  return <ItemSources workspaceId={workspaceId} itemId={itemId} revision={revision} />;
}

function ItemSources({
  workspaceId,
  itemId,
  revision,
}: {
  workspaceId: string;
  itemId: string;
  revision: Revision;
}) {
  const sources = useQuery(
    getSourcesOptions({
      body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
    }),
  );
  return (
    <div className="source-panel">
      {sources.isPending && (
        <>
          <h2 className="panel-heading">Sources</h2>
          <p role="status">Loading sources…</p>
        </>
      )}
      {sources.isError && (
        <>
          <h2 className="panel-heading">Sources</h2>
          <FailureNotice
            context="Listing this item’s sources"
            error={sources.error}
            onRetry={() => void sources.refetch()}
          />
        </>
      )}
      {sources.isSuccess && <Sources result={sources.data} />}
    </div>
  );
}
