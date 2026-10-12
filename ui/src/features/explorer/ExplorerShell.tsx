/**
 * The Explorer: folder tree and search, the document panes, and the source panel, in
 * resizable columns.
 *
 * The URL says which item the focused pane shows, at which selection and cited revision; the
 * Explorer store remembers the other tabs. Listings, search and reads are pinned to one resolved
 * workspace head, so their pages agree, unless a link names an exact revision. That head is
 * shown "as of" the moment it was resolved, never as "the latest". Returning to the window checks
 * for a newer head and says so; only Refresh re-resolves the listed head, and then the tree keeps
 * its place. (SPEC section 3: a latest selector is resolved once, by a request; section 7:
 * pagination stays tied to the same revision.)
 */
import { useQuery } from '@tanstack/react-query';
import { Outlet, useNavigate, useParams, useSearch } from '@tanstack/react-router';
import { type Ref, useEffect, useState } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { openWorkspaceQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { openWorkspace } from '../../api/generated/sdk.gen';
import type { Workspace } from '../../api/generated/types.gen';
import { zItemId } from '../../api/generated/zod.gen';
import { Instant } from '../../lib/instant';
import { itemOptions } from '../documents/DocumentView';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, messageFor } from '../errors/failure';
import { SearchForm, SearchResults } from '../search/SearchPanel';
import { DocumentPanes, matchingLayout } from './DocumentPanes';
import { FolderTree } from './FolderTree';
import { SourcePanel } from './SourcePanel';
import { type ItemSelection, validateItemSearch, validateWorkspaceSearch } from './search-params';
import { type PaneLocation, tabLocation, useExplorerStore, useWorkspaceView } from './store';

/** The resolved head the shell is pinned to: when it was resolved, and how to resolve it again. */
export interface HeadState {
  /** When the server's answer resolving the head arrived (milliseconds since the epoch). */
  resolvedAt: number;
  refreshing: boolean;
  /** The last refresh failed; the shell keeps the head it had. */
  refreshError: unknown;
  /** Re-resolve the head; the new head, or null when the refresh failed. */
  refresh: () => Promise<string | null>;
}

export interface ExplorerShellProps {
  workspace: Workspace;
  head: HeadState;
  /** The workspace's name: where keyboard focus goes when a retry has opened the workspace. */
  nameRef?: Ref<HTMLHeadingElement>;
}

/** A newer-head check older than this is repeated when the head line mounts again. */
const NEWER_HEAD_CHECK_AGE = 30_000;
const COLUMNS_GROUP = 'explorer-columns';
const COLUMN_IDS = ['navigation', 'documents', 'sources'] as const;

interface Destination {
  sel?: ItemSelection | undefined;
  rev?: string | undefined;
  replace?: boolean;
}

export function ExplorerShell({ workspace, head: headState, nameRef }: ExplorerShellProps) {
  const workspaceId = workspace.id;
  const head = workspace.head;
  const params = useParams({ strict: false });
  // The router passes unvalidated search values through to child routes, so the values used
  // here are validated again with the generated schemas. The item id in the path is checked
  // the same way: a malformed one is reported and never sent. (The workspace id was checked by
  // the workspace route before this shell rendered.)
  const raw: Record<string, unknown> = useSearch({ strict: false });
  const search = { ...validateWorkspaceSearch(raw), ...validateItemSearch(raw) };
  const navigate = useNavigate();
  const linkedItemId = params.itemId ?? null;
  const itemIdValid = linkedItemId !== null && zItemId.safeParse(linkedItemId).success;
  const itemId = itemIdValid ? linkedItemId : null;
  const invalidItemLink = linkedItemId !== null && !itemIdValid;
  const query = search.q;
  const view = useWorkspaceView(workspaceId);
  const openInStore = useExplorerStore((state) => state.openItem);
  const focusPane = useExplorerStore((state) => state.focusPane);
  const closeTab = useExplorerStore((state) => state.closeTab);
  const splitRight = useExplorerStore((state) => state.splitRight);
  const setLayout = useExplorerStore((state) => state.setLayout);
  const requestFocus = useExplorerStore((state) => state.requestFocus);

  // The URL is the truth for the focused pane: whatever item it names is open there, at the
  // selection and cited revision it names. The pane remembers them for when it loses focus.
  const sel = search.sel;
  const rev = search.rev;
  useEffect(() => {
    if (itemId !== null) openInStore(workspaceId, itemId, { sel, rev });
  }, [itemId, openInStore, workspaceId, sel, rev]);

  /**
   * Every link to an item this shell builds says which revision it reads (SPEC section 3: a
   * source reference keeps its resolved revision):
   * - pinned, carrying `rev`: a search result (the revision its hit resolved), "Read the latest
   *   version" (the revision the server resolved), a tab returned to, the tab shown after a close,
   *   a refocused pane and a split (the revision their URL last named), and a selection change
   *   within an item (the revision that pane reads);
   * - unpinned, meaning "the item now", read at the listed head: an entry opened from the folder
   *   tree, which lists that head. Refresh moves such a tab to the newer head; that is its intent.
   * Links to a workspace (the list, failure notices) name no item revision.
   */
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
  // The tree lists the head, so the item is revealed where it is at the head, found by its
  // id: a cited revision may have held it at another path, or the head may no longer hold it.
  const atListedHead = useQuery({
    ...itemOptions(workspaceId, itemId ?? '', head),
    enabled: itemId !== null,
  });
  const layouts = useExplorerStore((state) => state.layouts);
  const columns = matchingLayout(layouts[COLUMNS_GROUP], COLUMN_IDS);

  // Focusing a pane makes the URL describe it again: its item, selection and cited revision.
  const navigateToPane = (paneId: string) => {
    const pane = view.panes.find((candidate) => candidate.id === paneId);
    if (pane?.active) goToItem(pane.active, { ...tabLocation(pane, pane.active), replace: true });
    else goToWorkspace();
  };
  /** Where a pane's tab reads, as its URL last named it: the active tab's unless `itemId`. */
  const paneLocation = (paneId: string, itemId?: string): PaneLocation => {
    const pane = view.panes.find((candidate) => candidate.id === paneId);
    return pane === undefined ? {} : tabLocation(pane, itemId ?? pane.active);
  };
  const locationOf = (paneId: string): PaneLocation =>
    paneId === view.focusedPaneId
      ? { ...(sel === undefined ? {} : { sel }), ...(rev === undefined ? {} : { rev }) }
      : paneLocation(paneId);

  return (
    <div className="explorer">
      <div className="explorer-bar">
        <h1 className="workspace-name" ref={nameRef} tabIndex={-1}>
          {workspace.name}
        </h1>
        {workspace.description ? <p className="meta">{workspace.description}</p> : null}
        <HeadLine workspace={workspace} state={headState} />
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
                selectedItemPath={atListedHead.data?.summary.path ?? null}
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
            urlItemId={itemId}
            invalidItemLink={invalidItemLink}
            selection={search.sel}
            revision={search.rev}
            onActivate={(paneId, target) => {
              focusPane(workspaceId, paneId);
              // A tab returned to reads where it read: its selection and cited revision.
              goToItem(target, paneLocation(paneId, target));
            }}
            onFocusPane={(paneId) => {
              focusPane(workspaceId, paneId);
              navigateToPane(paneId);
            }}
            onClose={(paneId, target) => {
              const result = closeTab(workspaceId, paneId, target);
              if (result === null) return;
              // Keyboard focus never drops to the page: it goes where WAI-ARIA tabs put it.
              requestFocus(result.focus);
              if (!result.focusedChanged) return;
              if (result.show === null) goToWorkspace();
              else goToItem(result.show.itemId, { ...result.show.location, replace: true });
            }}
            onSplit={(paneId, target) => {
              const location = locationOf(paneId);
              splitRight(workspaceId, paneId, target, location);
              goToItem(target, { ...location, replace: true });
            }}
            onSelect={(target, location) => goToItem(target, location)}
            onReadLatest={(target, location) => goToItem(target, location)}
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

/**
 * The head the shell lists, "as of" when it was resolved, with Refresh to resolve it again.
 * Refresh keeps keyboard focus (aria-disabled while it runs) and a live region says the outcome.
 *
 * Whether a newer head exists is checked apart from the listed head, when the window regains focus
 * (and when the line mounts after its last check grew old). A newer head never replaces the listed
 * one: a polite notice says it exists, and only Refresh moves the shell to it. A check that fails
 * says so in the same polite line, with the failure's plain words; it is never silent.
 */
function HeadLine({ workspace, state }: { workspace: Workspace; state: HeadState }) {
  const head = workspace.head;
  const body = { workspace_id: workspace.id };
  const check = useQuery({
    queryKey: [{ ...openWorkspaceQueryKey({ body })[0], _check: 'newer-head' }],
    queryFn: async ({ signal }) => {
      const { data } = await openWorkspace({ body, signal, throwOnError: true });
      return data;
    },
    // Seeded with the listed head's own answer, so opening the workspace sends one request.
    initialData: workspace,
    initialDataUpdatedAt: state.resolvedAt,
    staleTime: NEWER_HEAD_CHECK_AGE,
    refetchOnWindowFocus: 'always',
  });
  const newer = check.data.head !== head && check.dataUpdatedAt > state.resolvedAt ? check : null;
  // A failed check is said quietly, in plain words, never as an alert: the line above stays true.
  // A Refresh since then resolved the head itself, so the failure no longer applies.
  const checkFailed =
    check.isError && check.errorUpdatedAt > state.resolvedAt
      ? messageFor(classifyFailure(check.error)).title
      : null;
  const [outcome, setOutcome] = useState('');
  const refresh = async () => {
    if (state.refreshing) return;
    setOutcome('Refreshing…');
    const next = await state.refresh();
    setOutcome(
      next === null
        ? 'The workspace could not be refreshed; still showing the version above.'
        : next === head
          ? `Refreshed: no newer version; still revision ${short(head)}.`
          : `Refreshed: now showing revision ${short(next)}.`,
    );
  };
  return (
    <div className="head-line">
      <p className="meta">
        Showing the workspace as of <Instant value={new Date(state.resolvedAt).toISOString()} />,
        revision <code title={head}>{short(head)}</code>.
      </p>
      {/* Present from the start, so the notice is announced when it appears. */}
      <p role="status" className="meta newer-head">
        {newer === null ? null : (
          <>
            A newer version of this workspace exists (checked at{' '}
            <Instant value={new Date(newer.dataUpdatedAt).toISOString()} />
            ).
          </>
        )}
        {newer !== null && checkFailed !== null ? ' ' : null}
        {checkFailed === null ? null : <>Could not check for a newer version: {checkFailed}</>}
      </p>
      <button type="button" aria-disabled={state.refreshing} onClick={() => void refresh()}>
        {state.refreshing ? 'Refreshing…' : 'Refresh'}
      </button>
      <p role="status" className="sr-only">
        {outcome}
      </p>
      {state.refreshError !== null && (
        <FailureNotice context="Refreshing this workspace" error={state.refreshError} />
      )}
    </div>
  );
}
