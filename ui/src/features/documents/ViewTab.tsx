/**
 * A View item's tab: the View itself, drawn by the views components (`SavedView`, which resolves
 * it with `resolve_view`) over the Explorer's reads (`view-data.ts`), then the sources it cites,
 * each a link that opens that source in the Explorer at the revision the View pins.
 *
 * The View is opened at the revision the tab reads (`revision`, the one it cites or the head the
 * workspace shows, both resolved), never at "the latest": a pinned View does not move under a
 * reader. The sources it cites are the server's reading of the saved View at that revision
 * (`get_view`, its bindings), so a link names the item, revision and selection the View binds,
 * whatever the source is called or holds now. Every read here is allowed in an archived workspace
 * (`ALLOWED_WHILE_ARCHIVED`), so nothing here asks what the person may change.
 *
 * Limit: the links stand under the View, not in its own list of "Sources read" (`Resolution.tsx`,
 * the views lane's), which names each binding without a way to open it.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useRef } from 'react';
import { getViewOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { Revision } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { SavedView } from '../views/SavedView';
import { describeSelection, short } from './describe';
import { workspaceViewData } from './view-data';

export interface ViewTabProps {
  workspaceId: string;
  itemId: string;
  /** The revision the tab reads: the one it cites, else the head the workspace shows. */
  revision: Revision;
}

export function ViewTab({ workspaceId, itemId, revision }: ViewTabProps) {
  return (
    <section aria-label="The View" className="view-tab">
      {/* The View's own actions go here, above it: "Export visual" (`export_view`, M4b B3) is
          built as its own package and needs only this tab's workspace, item and revision. */}
      <SavedView
        workspaceId={workspaceId}
        itemId={itemId}
        at={{ kind: 'revision', revision }}
        data={workspaceViewData}
      />
      <CitedSources workspaceId={workspaceId} itemId={itemId} revision={revision} />
    </section>
  );
}

/** The sources the saved View binds, each opened in the Explorer at the revision it binds. */
function CitedSources({ workspaceId, itemId, revision }: ViewTabProps) {
  const view = useQuery({
    ...getViewOptions({
      body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
    }),
    // What a resolved revision holds never changes.
    staleTime: Number.POSITIVE_INFINITY,
  });
  const listRef = useRef<HTMLUListElement | null>(null);
  const failure = useRetry({
    request: `${itemId} ${revision}`,
    failed: view.isError,
    error: view.error,
    run: () => view.refetch(),
    loaded: () => listRef.current?.querySelector<HTMLElement>('a') ?? null,
  });
  const bindings = view.data?.bindings ?? [];
  return (
    <nav aria-label="Sources this View cites" className="view-sources">
      <h3>Sources this View cites</h3>
      {view.isPending && !failure.retrying && (
        <p role="status">Listing the sources this View cites…</p>
      )}
      {failure.shown && (
        <FailureNotice
          context="Listing the sources this View cites"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {view.data !== undefined && bindings.length === 0 && <p>This View cites no sources.</p>}
      {bindings.length > 0 && (
        <>
          <p className="meta">Each opens in a tab at the revision this View cites.</p>
          <ul ref={listRef}>
            {keyed(bindings, (binding) => binding.name).map(({ key, value: { name, source } }) => (
              <li key={key}>
                {name}:{' '}
                <Link
                  to="/w/$workspaceId/i/$itemId"
                  params={{ workspaceId, itemId: source.item_id }}
                  search={{
                    ...(source.selection.kind === 'all' ? {} : { sel: source.selection }),
                    rev: source.revision,
                  }}
                >
                  {source.path}, {describeSelection(source.selection)}, at revision{' '}
                  <code title={source.revision}>{short(source.revision)}</code>
                </Link>
              </li>
            ))}
          </ul>
        </>
      )}
    </nav>
  );
}
