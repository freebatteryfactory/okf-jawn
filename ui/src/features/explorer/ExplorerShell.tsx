/**
 * The Explorer: folder tree and search, the document panes, and the source panel, in
 * resizable columns.
 *
 * The URL says which item the focused pane shows, at which selection and cited revision; the
 * Explorer store remembers the other tabs. Every read is pinned to the workspace head the
 * server resolved when the workspace opened, unless a citation named an exact revision.
 */
import { useQuery } from '@tanstack/react-query';
import { Outlet, useNavigate, useParams, useSearch } from '@tanstack/react-router';
import { useEffect } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import type { Workspace } from '../../api/generated/types.gen';
import { itemOptions } from '../documents/DocumentView';
import { SearchForm, SearchResults } from '../search/SearchPanel';
import { DocumentPanes, matchingLayout } from './DocumentPanes';
import { FolderTree } from './FolderTree';
import { SourcePanel } from './SourcePanel';
import type { ItemSelection } from './search-params';
import { useExplorerStore, useWorkspaceView } from './store';

export interface ExplorerShellProps {
  workspace: Workspace;
}

const COLUMNS_GROUP = 'explorer-columns';
const COLUMN_IDS = ['navigation', 'documents', 'sources'] as const;

interface Destination {
  sel?: ItemSelection | undefined;
  rev?: string | undefined;
  replace?: boolean;
}

export function ExplorerShell({ workspace }: ExplorerShellProps) {
  const workspaceId = workspace.id;
  const head = workspace.head;
  const params = useParams({ strict: false });
  const search = useSearch({ strict: false });
  const navigate = useNavigate();
  const itemId = params.itemId ?? null;
  const query = search.q;
  const view = useWorkspaceView(workspaceId);
  const openInStore = useExplorerStore((state) => state.openItem);
  const focusPane = useExplorerStore((state) => state.focusPane);
  const closeTab = useExplorerStore((state) => state.closeTab);
  const splitRight = useExplorerStore((state) => state.splitRight);
  const setLayout = useExplorerStore((state) => state.setLayout);

  // The URL is the truth for the focused pane: whatever item it names is open there.
  useEffect(() => {
    if (itemId !== null) openInStore(workspaceId, itemId);
  }, [itemId, openInStore, workspaceId]);

  const goToItem = (target: string, destination: Destination = {}) =>
    void navigate({
      to: '/w/$workspaceId/i/$itemId',
      params: { workspaceId, itemId: target },
      search: {
        ...(query === undefined ? {} : { q: query }),
        ...(destination.sel === undefined ? {} : { sel: destination.sel }),
        ...(destination.rev === undefined ? {} : { rev: destination.rev }),
      },
      replace: destination.replace ?? false,
    });
  const goToWorkspace = () =>
    void navigate({
      to: '/w/$workspaceId',
      params: { workspaceId },
      search: query === undefined ? {} : { q: query },
    });
  const setQuery = (next: string | undefined) =>
    void navigate({
      to: '.',
      search: (previous) => {
        const { q: _old, ...rest } = previous;
        return next === undefined ? rest : { ...rest, q: next };
      },
    });

  const revision = search.rev ?? head;
  const selected = useQuery({
    ...itemOptions(workspaceId, itemId ?? '', revision),
    enabled: itemId !== null,
  });
  const layouts = useExplorerStore((state) => state.layouts);
  const columns = matchingLayout(layouts[COLUMNS_GROUP], COLUMN_IDS);

  const navigateToPane = (paneId: string) => {
    const pane = view.panes.find((candidate) => candidate.id === paneId);
    if (pane?.active) goToItem(pane.active, { replace: true });
    else goToWorkspace();
  };

  return (
    <div className="explorer">
      <div className="explorer-bar">
        <h1 className="workspace-name">{workspace.name}</h1>
        {workspace.description ? <p className="meta">{workspace.description}</p> : null}
      </div>
      <Group
        orientation="horizontal"
        id={COLUMNS_GROUP}
        className="explorer-columns"
        {...(columns === undefined ? {} : { defaultLayout: columns })}
        onLayoutChanged={(next) => setLayout(COLUMNS_GROUP, next)}
      >
        <Panel id="navigation" defaultSize="24%" minSize="12%">
          <div className="sidebar">
            <SearchForm query={query} onSearch={setQuery} />
            {query === undefined ? (
              <FolderTree
                workspaceId={workspaceId}
                revision={head}
                selectedItemId={itemId}
                selectedItemPath={selected.data?.summary.path ?? null}
                onOpenItem={(target) => goToItem(target)}
              />
            ) : (
              <SearchResults
                workspaceId={workspaceId}
                head={head}
                query={query}
                onClear={() => setQuery(undefined)}
              />
            )}
          </div>
        </Panel>
        <Separator className="separator" aria-label="Resize the navigation column" />
        <Panel id="documents" minSize="30%">
          <DocumentPanes
            workspaceId={workspaceId}
            head={head}
            panes={view.panes}
            focusedPaneId={view.focusedPaneId}
            selection={search.sel}
            revision={search.rev}
            onActivate={(paneId, target) => {
              focusPane(workspaceId, paneId);
              goToItem(target);
            }}
            onFocusPane={(paneId) => {
              focusPane(workspaceId, paneId);
              navigateToPane(paneId);
            }}
            onClose={(paneId, target) => {
              const wasFocused = paneId === view.focusedPaneId;
              const next = closeTab(workspaceId, paneId, target);
              if (!wasFocused) return;
              if (next === null) goToWorkspace();
              else goToItem(next, { replace: true });
            }}
            onSplit={(paneId, target) => {
              splitRight(workspaceId, paneId, target);
              goToItem(target, { replace: true });
            }}
            onSelect={(target, sel) => goToItem(target, { sel, rev: search.rev })}
            onReadLatest={(target) => goToItem(target, { sel: search.sel })}
          />
        </Panel>
        <Separator className="separator" aria-label="Resize the source panel" />
        <Panel id="sources" defaultSize="26%" minSize="12%">
          <section aria-label="Source panel" className="source-column">
            <SourcePanel workspaceId={workspaceId} itemId={itemId} revision={revision} />
          </section>
        </Panel>
      </Group>
      <Outlet />
    </div>
  );
}
