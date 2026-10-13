/**
 * The source panel: the open item's citations and appearance from the sources operation. A failed
 * listing keeps its notice through a retry, which ends with keyboard focus on the heading of the
 * sources it loaded.
 */
import { useQuery } from '@tanstack/react-query';
import { useRef } from 'react';
import { getSourcesOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { Revision } from '../../api/generated/types.gen';
import { Sources } from '../documents/Sources';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';

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
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const failure = useRetry({
    request: `${itemId} ${revision}`,
    failed: sources.isError,
    error: sources.error,
    run: () => sources.refetch(),
    loaded: () => headingRef.current,
  });
  return (
    <div className="source-panel">
      {sources.isPending && !failure.retrying && (
        <>
          <h2 className="panel-heading">Sources</h2>
          <p role="status">Loading sources…</p>
        </>
      )}
      {failure.shown && (
        <>
          <h2 className="panel-heading">Sources</h2>
          <FailureNotice
            context="Listing this item’s sources"
            error={failure.error}
            retry={failure}
            workspaceId={workspaceId}
          />
        </>
      )}
      {sources.isSuccess && <Sources result={sources.data} headingRef={headingRef} />}
    </div>
  );
}
