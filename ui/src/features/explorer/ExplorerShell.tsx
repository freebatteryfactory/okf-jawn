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
import {
  Outlet,
  type ShouldBlockFn,
  useBlocker,
  useNavigate,
  useParams,
  useSearch,
} from '@tanstack/react-router';
import { type Ref, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { openWorkspaceQueryKey } from '../../api/generated/@tanstack/react-query.gen';
import { openWorkspace } from '../../api/generated/sdk.gen';
import type { Workspace } from '../../api/generated/types.gen';
import { zItemId } from '../../api/generated/zod.gen';
import { pageHeading } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { type LeaveGuard, NamingRulesPanel } from '../conventions/NamingRules';
import { itemOptions } from '../documents/DocumentView';
import { short } from '../documents/describe';
import { DraftsPanel } from '../drafts/DraftsPanel';
import { useLeaveGuard } from '../drafts/leave-guard';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, messageFor } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { SearchForm, SearchResults } from '../search/SearchPanel';
import { ChangesContext } from '../tree/context';
import { DocumentPanes, matchingLayout } from './DocumentPanes';
import { FolderTree } from './FolderTree';
import type { HeadChange } from './head';
import { useForgetRestored } from './restored';
import { SourcePanel } from './SourcePanel';
import { type ItemSelection, validateItemSearch, validateWorkspaceSearch } from './search-params';
import {
  type PaneLocation,
  tabLocation,
  useDeleted,
  useExplorerStore,
  useWorkspaceView,
} from './store';

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

/** Where a navigation starts or ends, as the router's blocker tells it. */
interface Place {
  pathname: string;
  search: object;
}

/** A place without its search query: the query changes what the sidebar lists, nothing else. */
const apartFromQuery = (place: Place) =>
  JSON.stringify([
    place.pathname,
    Object.entries(place.search)
      .filter(([key]) => key !== 'q')
      .sort(([a], [b]) => a.localeCompare(b)),
  ]);

/**
 * Whether a navigation takes the naming rules off the page: it goes to another page, or to an
 * item (another one, or the same at another selection or revision), which is then shown in the
 * rules' place. A change of the search query alone does not.
 */
export const leavesRules = (current: Place, next: Place) =>
  apartFromQuery(current) !== apartFromQuery(next);

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
  const repin = useExplorerStore((state) => state.repin);
  const canWrite = workspace.permissions.includes('write');
  useLeaveGuard();
  // An item remembered as deleted is forgotten when the head shown holds it again.
  useForgetRestored(workspaceId, head);
  // The naming rules stand in the documents column while they are open. Which column shows what
  // is this page's own state, like its open tabs. Closing gives focus back to the button.
  const [rulesOpen, setRulesOpen] = useState(false);
  const rulesButton = useRef<HTMLButtonElement | null>(null);
  // The panel's guard, while it is open: leaving it asks first when edits would be lost.
  const rulesGuard = useRef<LeaveGuard | null>(null);
  const leaveRules = (proceed: () => void, stay?: () => void) => {
    const guard = rulesOpen ? rulesGuard.current : null;
    if (guard === null) proceed();
    else guard(proceed, stay);
  };
  // Every other way off the rules goes through the router, and the router holds it back until
  // the same question is answered: Back and Forward, a link to another page, a search result's
  // link. Not the page's own corrections of the address (a replace: a tab follows a change), and
  // not a change of the search query alone. A navigation that goes on closes the rules: an item
  // it names on this page is on screen, not behind them. Reload and closing the browser tab are
  // the panel's own (`beforeunload`).
  const holdBack = useRef<ShouldBlockFn>(() => false);
  // What had keyboard focus when a way off the rules was taken, once the documents are back in
  // their place: focus goes back to it (a row of the folders, a link), and when it went with the
  // rules (Back or Forward with focus in them), to the page's heading. Never to the page body.
  const askedToLeave = useRef<Element | null | undefined>(undefined);
  const showDocuments = (asker: Element | null) => {
    askedToLeave.current = asker;
    setRulesOpen(false);
  };
  useLayoutEffect(() => {
    const asker = askedToLeave.current;
    if (asker === undefined || rulesOpen) return;
    askedToLeave.current = undefined;
    const still = asker instanceof HTMLElement && asker.isConnected && asker !== document.body;
    (still ? asker : pageHeading())?.focus();
  });
  useEffect(() => {
    holdBack.current = ({ current, next, action }) => {
      if (action === 'REPLACE' || !leavesRules(current, next)) return false;
      const asker = document.activeElement;
      return new Promise<boolean>((resolve) => {
        leaveRules(
          () => {
            showDocuments(asker);
            resolve(false);
          },
          () => resolve(true),
        );
      });
    };
  });
  useBlocker({
    disabled: !rulesOpen,
    enableBeforeUnload: false,
    shouldBlockFn: useCallback<ShouldBlockFn>((navigation) => holdBack.current(navigation), []),
  });
  const focusRulesButton = useRef(false);
  useLayoutEffect(() => {
    if (!focusRulesButton.current || rulesOpen) return;
    focusRulesButton.current = false;
    rulesButton.current?.focus();
  });

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
  const goToItem = (target: string, destination: Destination = {}) => {
    // An item that is opened is shown: the documents come back in place of the naming rules,
    // after the panel's question when its edits are not saved.
    if (destination.replace !== true && rulesOpen) {
      const asker = document.activeElement;
      leaveRules(() => {
        showDocuments(asker);
        // The question was asked here: the router does not ask it again.
        openItemAt(target, destination, true);
      });
      return;
    }
    openItemAt(target, destination);
  };
  const openItemAt = (target: string, destination: Destination, asked = false) => {
    void navigate({
      to: '/w/$workspaceId/i/$itemId',
      params: { workspaceId, itemId: target },
      search: {
        ...(query === undefined ? {} : { q: query }),
        ...(destination.sel === undefined ? {} : { sel: destination.sel }),
        ...(destination.rev === undefined ? {} : { rev: destination.rev }),
      },
      replace: destination.replace ?? false,
      ignoreBlocker: asked,
    });
  };
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

  /**
   * The person's own change moved the shown head to the revision it made (`head.ts`): a
   * Snapshot, a move or rename, a status word. A tab that reads the shown head follows by itself.
   * One that cited the head as it was (a search result's link, the latest version read) would go
   * on reading the version before the person's own change, so each such tab of a changed item now
   * cites the revision the change made: in the store, and in the URL when it is the item the URL
   * names. An item's identity does not change with its path, so the tabs of a moved item go on
   * reading it.
   */
  const followSnapshot = (taken: Pick<HeadChange, 'revision' | 'previousHead' | 'itemIds'>) => {
    const before = taken.previousHead ?? head;
    repin(workspaceId, taken.itemIds, before, taken.revision);
    if (itemId !== null && taken.itemIds.includes(itemId) && rev === before) {
      goToItem(itemId, {
        ...(sel === undefined ? {} : { sel }),
        rev: taken.revision,
        replace: true,
      });
    }
  };

  const revision = search.rev ?? head;
  // The tree lists the head, so the item is revealed where it is at the head, found by its
  // id: a cited revision may have held it at another path, or the head may no longer hold it.
  // An item the person deleted in this page is not at the head: nothing is asked about it there.
  const gone = useDeleted(workspaceId, itemId ?? '') !== undefined && search.rev === undefined;
  const atListedHead = useQuery({
    ...itemOptions(workspaceId, itemId ?? '', head),
    enabled: itemId !== null && !gone,
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

  const changes = {
    workspaceId,
    head,
    canWrite,
    refresh: headState.refresh,
    onChanged: followSnapshot,
  };
  return (
    <ChangesContext.Provider value={changes}>
      <div className="explorer">
        <div className="explorer-bar">
          <h1 className="workspace-name" ref={nameRef} tabIndex={-1}>
            {workspace.name}
          </h1>
          {workspace.description ? <p className="meta">{workspace.description}</p> : null}
          <button
            type="button"
            ref={rulesButton}
            aria-expanded={rulesOpen}
            onClick={() => {
              if (!rulesOpen) setRulesOpen(true);
              else leaveRules(() => setRulesOpen(false));
            }}
          >
            Naming rules
          </button>
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
              <DraftsPanel
                workspaceId={workspaceId}
                head={head}
                canWrite={canWrite}
                onSnapshotted={followSnapshot}
                onOpenItem={(target) => goToItem(target)}
              />
            </div>
          </Panel>
          <Separator className="separator" aria-label="Resize the navigation column" />
          <Panel id="documents" minSize="30%">
            {rulesOpen && (
              <div className="pane">
                <NamingRulesPanel
                  leaveGuard={rulesGuard}
                  onClose={() => {
                    focusRulesButton.current = true;
                    setRulesOpen(false);
                  }}
                />
              </div>
            )}
            <div className="documents-area" hidden={rulesOpen}>
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
                canWrite={canWrite}
                onOpenCurrent={(target) => goToItem(target)}
                onSnapshotted={followSnapshot}
              />
            </div>
          </Panel>
          <Separator className="separator" aria-label="Resize the source panel" />
          <Panel id="sources" defaultSize="26%" minSize="12%">
            <section aria-label="Source panel" className="source-column">
              <SourcePanel
                workspaceId={workspaceId}
                itemId={gone ? null : itemId}
                revision={revision}
              />
            </section>
          </Panel>
        </Group>
        <Outlet />
      </div>
    </ChangesContext.Provider>
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
  // A failed refresh is said in a notice with no retry of its own: Refresh is the retry. The
  // failure can still leave while this line stays (a refresh that succeeds, or the workspace being
  // asked again when the network returns), and its notice can hold keyboard focus on its way out
  // (the workspace is gone). The hook follows that focus; with the failure gone it goes to Refresh.
  const refreshRef = useRef<HTMLButtonElement | null>(null);
  const refreshFailure = useRetry({
    request: workspace.id,
    failed: state.refreshError !== null,
    error: state.refreshError,
    loaded: () => refreshRef.current,
  });
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
      <button
        type="button"
        ref={refreshRef}
        aria-disabled={state.refreshing}
        onClick={() => void refresh()}
      >
        {state.refreshing ? 'Refreshing…' : 'Refresh'}
      </button>
      <p role="status" className="sr-only">
        {outcome}
      </p>
      {refreshFailure.shown && (
        <FailureNotice
          context="Refreshing this workspace"
          error={refreshFailure.error}
          retry={refreshFailure}
        />
      )}
    </div>
  );
}
