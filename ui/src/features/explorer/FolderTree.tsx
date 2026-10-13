/**
 * The folder tree: react-arborist over the listing operation, loaded folder by folder.
 *
 * Folders and items appear as the workspace organizes them. Each item shows its title and its
 * user type name (an open vocabulary); its rendering role (Note, Source or View) picks the icon.
 * Arborist supplies the tree semantics and keyboard model (arrows, Home/End, type-ahead, Space);
 * Enter also opens the focused entry. The open item of the URL is the tree's selection, and its
 * folders are expanded when it is opened from a link. "Show more entries" and a retry move
 * keyboard focus to the first entry they brought (or keep it on a row still there), and a live
 * region says what happened. A failed folder is a row of the tree, which stays while it is listed
 * again; a failed root listing is a failure notice, kept through its retry (`useRetry`).
 *
 * However keyboard focus enters the tree (Tab onto its container, Shift+Tab onto its tab-stop row
 * or its scrolling list, a click, a screen reader), arborist is told which entry has it, because
 * its arrow keys move from the entry it knows is focused. Entering the tree lands on the selected
 * entry when the tree shows it, otherwise the first (the WAI-ARIA tree pattern).
 *
 * A person who may write gets the tree's changes above it (`tree/TreeActions.tsx`): a new note, a
 * new folder, and for the current item a rename or move and a delete; F2 and Delete on an item's
 * row open the last two. After a change the tree lists the head the change made, and keyboard
 * focus that rested on the bar goes to the row of what was created or moved, or to the entry now
 * where the deleted one was (`TreeReveal`). A row also shows an item's status word when it is not
 * "stable", as "status draft", which is not the mark of the person's own draft.
 */
import { type UseQueryResult, useQueries, useQueryClient } from '@tanstack/react-query';
import { ChartColumn, FileBox, FileText, Folder, FolderOpen } from 'lucide-react';
import {
  createContext,
  forwardRef,
  type HTMLProps,
  type FocusEvent as ReactFocusEvent,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  ListOuterElement,
  type NodeApi,
  type NodeRendererProps,
  type RowRendererProps,
  Tree,
  type TreeApi,
} from 'react-arborist';
import type { ItemSummary, Revision } from '../../api/generated/types.gen';
import { type DraftMark, useDraftMarks } from '../drafts/marks';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, messageFor } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { useWorkspaceChanges } from '../tree/context';
import { splitPath } from '../tree/names';
import {
  NO_FOLDER_CHANGES,
  type OpenForm,
  TreeActions,
  type TreeTarget,
} from '../tree/TreeActions';
import {
  ancestorFolders,
  type FolderListing,
  folderListingOptions,
  visibleFolders,
} from './folder-listing';
import { useExplorerStore, useWorkspaceView } from './store';
import { useElementHeight } from './use-element-height';

type StatusKind = 'loading' | 'error' | 'more' | 'more-loading' | 'more-error';

interface TreeEntry {
  id: string;
  /** Visible label; arborist's type-ahead also reads it. */
  name: string;
  kind: 'folder' | 'item' | 'status';
  folder: string;
  item: ItemSummary | null;
  /** How the item's draft stands, when it has one. */
  mark: DraftMark | null;
  status: StatusKind | null;
  error: unknown;
  children: TreeEntry[] | null;
}

export interface FolderTreeProps {
  workspaceId: string;
  revision: Revision;
  /** The item the URL has open. */
  selectedItemId: string | null;
  /** Its path, once known, so its folders can be expanded. */
  selectedItemPath: string | null;
  onOpenItem: (itemId: string) => void;
}

/** The height of a one-line row; a row whose name wraps measures taller. */
const ROW_HEIGHT = 30;
const ROLE_ICON = { note: FileText, source: FileBox, view: ChartColumn } as const;
const EXTRACTION_WORD = {
  pending: 'conversion pending',
  partial: 'partly converted',
  failed: 'conversion failed',
  unsupported: 'format not supported',
} as const;

const folderId = (folder: string) => `folder:${folder}`;
const itemNodeId = (itemId: string) => `item:${itemId}`;
const baseName = (path: string) => path.slice(path.lastIndexOf('/') + 1);

/** Whether a failed listing may pass when repeated, so its row offers Enter to try again. */
function retryable(error: unknown): boolean {
  return messageFor(classifyFailure(error)).action.kind === 'retry';
}

/** A failure in the tree's words: what happened, the server's own message, and what Enter does. */
function describeListingFailure(error: unknown): string {
  const failure = classifyFailure(error);
  const said =
    failure.kind === 'api' && failure.error.message.length > 0
      ? ` The service said: “${failure.error.message}”.`
      : '';
  const retry = retryable(error) ? ' Press Enter to try again.' : '';
  return `${messageFor(failure).title}${said}${retry}`;
}

function statusEntry(folder: string, status: StatusKind, error: unknown = null): TreeEntry {
  const failure = error === null ? '' : describeListingFailure(error);
  const name = {
    loading: 'Loading…',
    error: `Could not list this folder: ${failure}`,
    more: 'Show more entries',
    'more-loading': 'Loading more entries…',
    'more-error': `Could not list more entries: ${failure}`,
  }[status];
  return {
    // One id per row through its states (loading, failed; more, loading more, failed), so a
    // row that has keyboard focus keeps it while it changes.
    id: statusRowId(folder, status),
    name,
    kind: 'status',
    folder,
    item: null,
    mark: null,
    status,
    error,
    children: null,
  };
}

function entriesOf(
  folder: string,
  listings: ReadonlyMap<string, UseQueryResult<FolderListing>>,
  expanded: Record<string, true>,
  revision: Revision,
  markOf: (itemId: string) => DraftMark | null,
): TreeEntry[] {
  const result = listings.get(folder);
  if (result === undefined) return [];
  if (result.isPending) return [statusEntry(folder, 'loading')];
  if (result.isError) return [statusEntry(folder, 'error', result.error)];
  const entries: TreeEntry[] = [];
  for (const page of result.data.pages) {
    for (const child of page.folders) {
      entries.push({
        id: folderId(child),
        name: baseName(child),
        kind: 'folder',
        folder: child,
        item: null,
        mark: null,
        status: null,
        error: null,
        children: expanded[child] ? entriesOf(child, listings, expanded, revision, markOf) : [],
      });
    }
    for (const item of page.items) {
      entries.push({
        id: itemNodeId(item.id),
        name: item.title.length > 0 ? item.title : baseName(item.path),
        kind: 'item',
        folder,
        item,
        mark: markOf(item.id),
        status: null,
        error: null,
        children: null,
      });
    }
  }
  const last = result.data.pages.at(-1);
  // A placeholder at this revision is one more page loading; one listed at an older head is the
  // listing shown before a refresh, kept as it was until the newer one arrives.
  if (result.isPlaceholderData && result.data.revision === revision)
    entries.push(statusEntry(folder, 'more-loading'));
  else if (result.data.moreError !== null) {
    entries.push(statusEntry(folder, 'more-error', result.data.moreError));
  } else if (last?.next_cursor) entries.push(statusEntry(folder, 'more'));
  return entries;
}

function contains(entries: readonly TreeEntry[], id: string): boolean {
  return entries.some((entry) => entry.id === id || contains(entry.children ?? [], id));
}

function findEntry(entries: readonly TreeEntry[], id: string): TreeEntry | null {
  for (const entry of entries) {
    if (entry.id === id) return entry;
    const found = findEntry(entry.children ?? [], id);
    if (found !== null) return found;
  }
  return null;
}

function statusRowId(folder: string, status: StatusKind): string {
  return status === 'loading' || status === 'error' ? `listing:${folder}` : `more:${folder}`;
}

/** The entries listed in `folder` (the root's for ''), as the tree shows them. */
function childrenOf(entries: readonly TreeEntry[], folder: string): readonly TreeEntry[] {
  if (folder === '') return entries;
  for (const entry of entries) {
    if (entry.id === folderId(folder)) return entry.children ?? [];
    const found = childrenOf(entry.children ?? [], folder);
    if (found.length > 0) return found;
  }
  return [];
}

/**
 * The entry keyboard focus enters the tree on, as the WAI-ARIA tree pattern says: the selected
 * entry (the open item's row) when the tree shows it, otherwise the first entry.
 */
function tabStopOf(tree: TreeApi<TreeEntry> | null): NodeApi<TreeEntry> | null {
  return tree === null ? null : (tree.selectedNodes.at(0) ?? tree.firstNode);
}

/** A listing the person asked for ("Show more entries", or a retry), awaiting its answer. */
interface PendingLoad {
  folder: string;
  /** How many entries the folder showed when asked: the first new one is at this index. */
  from: number;
  /** For "more": the larger page count is a new query, settled once it is not a placeholder. */
  more: boolean;
  /** For a retry: settled once the query has answered after this instant. */
  since: number;
}

export function FolderTree({
  workspaceId,
  revision,
  selectedItemId,
  selectedItemPath,
  onOpenItem,
}: FolderTreeProps) {
  const client = useQueryClient();
  const view = useWorkspaceView(workspaceId);
  const setExpanded = useExplorerStore((state) => state.setExpanded);
  const showMore = useExplorerStore((state) => state.showMore);
  const treeRef = useRef<TreeApi<TreeEntry> | undefined>(undefined);
  const boxRef = useRef<HTMLDivElement | null>(null);
  const height = useElementHeight(boxRef);
  const heading = useId();
  const { canWrite } = useWorkspaceChanges();
  // The tree's current entry: the one keyboard focus was last on. The bar above acts on it.
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [form, setForm] = useState<OpenForm | null>(null);
  const barRef = useRef<HTMLDivElement | null>(null);
  const reveal = useExplorerStore((state) => state.treeReveal);
  const revealInTree = useExplorerStore((state) => state.revealInTree);

  // Measured row heights by entry id: a wrapped name makes its row taller. The list keeps
  // virtualizing; a changed height re-lays out the list from that row on.
  const rowHeights = useRef(new Map<string, number>());
  const rowHeight = useCallback(
    (node: NodeApi<TreeEntry>) => rowHeights.current.get(node.id) ?? ROW_HEIGHT,
    [],
  );
  const measureRow = useCallback((id: string, index: number, measured: number) => {
    if (measured <= 0) return; // no layout (or not yet)
    const next = Math.max(ROW_HEIGHT, Math.ceil(measured));
    if (rowHeights.current.get(id) === next) return;
    rowHeights.current.set(id, next);
    treeRef.current?.redrawList(index);
  }, []);

  // F2 and Delete on a row, for a person who may write: the form opens for that row's item.
  const rowKey = useRef<(node: NodeApi<TreeEntry>, key: string) => boolean>(() => false);
  const rowContext = useMemo(
    () => ({
      measure: measureRow,
      onKey: (node: NodeApi<TreeEntry>, key: string) => rowKey.current(node, key),
    }),
    [measureRow],
  );

  // Keyboard focus on a row that a newer listing no longer holds would drop to the page when the
  // row goes (a Refresh lands while focus is in the tree). It moves to the entry now at that
  // place, or the nearest remaining one; with no entry left, to the empty-workspace line. The
  // row that last had focus and the place of arborist's focused entry are remembered for this.
  const lastFocusIndex = useRef<number | null>(null);
  const lastFocusedRow = useRef<HTMLElement | null>(null);
  const emptyRef = useRef<HTMLParagraphElement | null>(null);
  const rememberRow = (event: ReactFocusEvent<HTMLElement>) => {
    if (event.target.getAttribute('role') === 'treeitem') lastFocusedRow.current = event.target;
  };
  const forgetRow = (event: ReactFocusEvent<HTMLElement>) => {
    const row = event.target;
    const to = event.relatedTarget;
    if (to instanceof Node && !boxRef.current?.contains(to)) lastFocusedRow.current = null;
    else if (to === null) {
      // Focus left for no element: if the row is still there, it was not removed.
      window.setTimeout(() => {
        if (lastFocusedRow.current === row && row.isConnected && document.activeElement !== row) {
          lastFocusedRow.current = null;
        }
      });
    }
  };

  // Keyboard focus that lands on the tree itself (Tab onto its container) or on its scrolling
  // list (Shift+Tab while the tab-stop row is scrolled out of the rendered window) moves on to the
  // tab-stop entry, scrolled into view, and arborist is told it is focused. A press of the pointer
  // is left to arborist, so clicking the list or its scrollbar never scrolls it away.
  const pointing = useRef(false);
  const enterTree = (event: ReactFocusEvent<HTMLElement>) => {
    const tree = treeRef.current;
    const box = boxRef.current;
    if (tree === undefined || box === null || pointing.current) return;
    const from = event.relatedTarget;
    if (from instanceof Node && box.contains(from)) return;
    const target = event.target;
    if (!box.contains(target) || target.getAttribute('role') === 'treeitem') return;
    const stop = tabStopOf(tree);
    if (stop !== null) tree.focus(stop.id);
  };

  // Opening an item from a link expands the folders above it, so the tree can show it selected.
  useEffect(() => {
    if (selectedItemPath === null) return;
    for (const folder of ancestorFolders(selectedItemPath)) setExpanded(workspaceId, folder, true);
  }, [selectedItemPath, setExpanded, workspaceId]);

  // Arborist keeps its own open state; folders expanded here are opened there too, before
  // their children arrive, so a selected item is visible (and selectable) when it appears.
  useEffect(() => {
    for (const folder of Object.keys(view.expanded)) treeRef.current?.open(folderId(folder));
  }, [view.expanded]);

  const folders = visibleFolders(view.expanded);
  const results = useQueries({
    queries: folders.map((folder) =>
      folderListingOptions(client, workspaceId, revision, folder, view.folderPages[folder] ?? 1),
    ),
  });
  const listings = new Map<string, UseQueryResult<FolderListing>>();
  folders.forEach((folder, index) => {
    const result = results[index];
    if (result !== undefined) listings.set(folder, result);
  });
  const root = listings.get('');
  const markOf = useDraftMarks(workspaceId);
  const entries = entriesOf('', listings, view.expanded, revision, markOf);
  // A root listing that failed keeps its notice through a retry, which ends with keyboard focus
  // on the tree's tab-stop entry (the empty-workspace line, when it listed nothing).
  const rootFailure = useRetry({
    request: `${workspaceId} ${revision}`,
    failed: root?.isError ?? false,
    error: root?.error,
    run: () => root?.refetch() ?? Promise.resolve(),
    loaded: () =>
      boxRef.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]') ??
      emptyRef.current,
  });
  // Put keyboard focus on an entry's row. Arborist focuses the row of an entry that becomes its
  // focused one; an entry that already is (focus went to a form and comes back) is focused here.
  const focusRow = (tree: TreeApi<TreeEntry>, id: string) => {
    tree.focus(id);
    for (const element of boxRef.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ??
      []) {
      if (element.dataset.entry === id) element.focus();
    }
  };
  // A form opened from a row gives keyboard focus back to that row when it is left. The bar asks
  // for it in its own layout effect, which runs before the tree has its handle again in the same
  // commit (a child's layout effects run first), so the row is focused here, in this component's.
  const returnToRow = useRef(false);
  useLayoutEffect(() => {
    if (!returnToRow.current) return;
    returnToRow.current = false;
    const tree = treeRef.current ?? null;
    const row = current !== null ? current.id : tabStopOf(tree)?.id;
    if (tree !== null && row !== undefined) focusRow(tree, row);
    else barRef.current?.querySelector('button')?.focus();
  });
  const targetOf = (entry: TreeEntry | null, index: number): TreeTarget | null => {
    if (entry === null || entry.kind === 'status') return null;
    return entry.item === null
      ? { kind: 'folder', folder: entry.folder }
      : { kind: 'item', item: entry.item, folder: entry.folder, mark: entry.mark, index };
  };
  const current = currentId === null ? null : findEntry(entries, currentId);
  rowKey.current = (node, key) => {
    if (!canWrite || (key !== 'F2' && key !== 'Delete')) return false;
    const target = targetOf(node.data, node.rowIndex ?? 0);
    if (target === null) return false;
    if (target.kind === 'folder') setLoadNote(NO_FOLDER_CHANGES);
    else {
      setCurrentId(node.id);
      setForm({
        what: key === 'F2' ? 'move' : 'delete',
        folder: target.folder,
        item: target,
        fromRow: true,
      });
    }
    return true;
  };

  // A change names what the tree shows focused next: its folders are opened, and once every
  // folder on screen is listed at the head the change made, focus that rested on the bar (or fell
  // to the page) goes to that row. Focus the person has put elsewhere is left there.
  useEffect(() => {
    if (reveal === null || reveal.workspaceId !== workspaceId || reveal.kind === 'place') return;
    const above = reveal.kind === 'item' ? reveal.path : reveal.folder;
    for (const folder of ancestorFolders(above)) setExpanded(workspaceId, folder, true);
  }, [reveal, setExpanded, workspaceId]);
  useEffect(() => {
    if (reveal === null || reveal.workspaceId !== workspaceId) return;
    // The shell shows the head a change made a moment after the change answers.
    if (reveal.revision !== revision) return;
    const wanted =
      reveal.kind === 'place'
        ? []
        : ancestorFolders(reveal.kind === 'item' ? reveal.path : reveal.folder);
    if (wanted.some((folder) => !folders.includes(folder))) return;
    const listed = folders.every((folder) => {
      const result = listings.get(folder);
      if (result === undefined) return false;
      return (
        result.isError ||
        (result.isSuccess && !result.isPlaceholderData && result.data.revision === revision)
      );
    });
    if (!listed) return;
    revealInTree(null);
    const active = document.activeElement;
    const resting = active === null || active === document.body || barRef.current?.contains(active);
    if (!resting) return;
    // React clears the ref when the tree leaves the page (an emptied workspace).
    const tree = treeRef.current ?? null;
    if (reveal.kind === 'place') {
      const near = tree?.at(Math.min(reveal.index, tree.visibleNodes.length - 1)) ?? null;
      if (tree !== null && near !== null) focusRow(tree, near.id);
      else emptyRef.current?.focus();
      return;
    }
    const id = reveal.kind === 'item' ? itemNodeId(reveal.itemId) : folderId(reveal.folder);
    if (tree !== null && contains(entries, id)) focusRow(tree, id);
    else {
      const name = splitPath(reveal.kind === 'item' ? reveal.path : reveal.folder).name;
      setLoadNote(`“${name}” is not among the entries shown yet. Show more entries to find it.`);
    }
  });

  const selected =
    selectedItemId !== null && contains(entries, itemNodeId(selectedItemId))
      ? itemNodeId(selectedItemId)
      : undefined;

  // A listing asked for from the keyboard moves focus to the first entry it brought. If none
  // came, focus stays on a row that is still there, and the status says what happened.
  const pending = useRef<PendingLoad | null>(null);
  const [loadNote, setLoadNote] = useState('');
  useEffect(() => {
    const load = pending.current;
    if (load === null) return;
    const result = listings.get(load.folder);
    if (result === undefined) {
      pending.current = null;
      return;
    }
    const settled = load.more
      ? !result.isPlaceholderData && !result.isPending && !result.isFetching
      : !result.isFetching && Math.max(result.dataUpdatedAt, result.errorUpdatedAt) >= load.since;
    if (!settled) return;
    pending.current = null;
    const tree = treeRef.current;
    const shown = childrenOf(entries, load.folder);
    const listed = shown.filter((entry) => entry.kind !== 'status');
    const firstNew = listed.at(load.from);
    if (firstNew !== undefined) {
      const arrived = listed.length - load.from;
      tree?.focus(firstNew.id);
      setLoadNote(`${arrived} ${arrived === 1 ? 'entry' : 'entries'} shown.`);
      return;
    }
    const stillThere = shown.find((entry) => entry.kind === 'status') ?? listed.at(-1);
    if (stillThere !== undefined) tree?.focus(stillThere.id);
    setLoadNote(
      result.isError || (result.data?.moreError ?? null) !== null
        ? 'The entries could not be listed.'
        : 'No more entries.',
    );
  });

  // After each render: remember where arborist's focused entry stands; if the row that had
  // keyboard focus was removed with its entry, move focus to the nearest entry (see above).
  useLayoutEffect(() => {
    const tree = treeRef.current;
    const row = lastFocusedRow.current;
    const active = document.activeElement;
    const dropped =
      row !== null && !row.isConnected && (active === null || active === document.body);
    if (tree) {
      const focusId = tree.state.nodes.focus.id;
      const index = focusId === null ? undefined : tree.indexOf(focusId);
      if (index !== null && index !== undefined) {
        lastFocusIndex.current = index;
        return;
      }
      if (!dropped) return;
      lastFocusedRow.current = null;
      const near = tree.at(Math.min(lastFocusIndex.current ?? 0, tree.visibleNodes.length - 1));
      if (near !== null) tree.focus(near.id);
      return;
    }
    if (dropped) {
      lastFocusedRow.current = null;
      emptyRef.current?.focus();
    }
  });

  const activate = (node: NodeApi<TreeEntry>) => {
    const entry = node.data;
    const shownCount = () =>
      childrenOf(entries, entry.folder).filter((child) => child.kind !== 'status').length;
    if (entry.kind === 'item' && entry.item !== null) onOpenItem(entry.item.id);
    else if (entry.kind === 'folder') node.toggle();
    else if (entry.status === 'more') {
      pending.current = { folder: entry.folder, from: shownCount(), more: true, since: 0 };
      showMore(workspaceId, entry.folder);
    } else if (
      (entry.status === 'error' || entry.status === 'more-error') &&
      retryable(entry.error)
    ) {
      pending.current = {
        folder: entry.folder,
        from: shownCount(),
        more: false,
        since: Date.now(),
      };
      void listings.get(entry.folder)?.refetch();
    }
  };

  return (
    <section
      className="folder-tree"
      aria-labelledby={heading}
      onPointerDownCapture={() => {
        // Focus a pointer press causes happens before this task ends; the flag lasts no longer.
        pointing.current = true;
        window.setTimeout(() => {
          pointing.current = false;
        });
      }}
      onFocusCapture={(event) => {
        rememberRow(event);
        enterTree(event);
      }}
      onBlurCapture={forgetRow}
    >
      <h2 id={heading} className="panel-heading">
        Folders
      </h2>
      {canWrite && (
        <TreeActions
          target={targetOf(
            current,
            current === null ? 0 : (treeRef.current?.indexOf(current.id) ?? 0),
          )}
          form={form}
          onForm={setForm}
          onReturnToRow={() => {
            returnToRow.current = true;
          }}
          onOpenItem={onOpenItem}
          barRef={barRef}
        />
      )}
      {root?.isPending && !rootFailure.retrying && <p role="status">Loading folders…</p>}
      {rootFailure.shown && (
        <FailureNotice
          context="Listing this workspace"
          error={rootFailure.error}
          retry={rootFailure}
        />
      )}
      {root?.isSuccess && entries.length === 0 && (
        <p ref={emptyRef} tabIndex={-1}>
          This workspace has no folders or items yet.
        </p>
      )}
      <p role="status" className="sr-only">
        {loadNote}
      </p>
      <div ref={boxRef} className="tree-box">
        {root?.isSuccess && entries.length > 0 && (
          <TreeRowContext.Provider value={rowContext}>
            <Tree<TreeEntry>
              ref={treeRef}
              data={entries}
              aria-labelledby={heading}
              openByDefault={false}
              initialOpenState={Object.fromEntries(
                Object.keys(view.expanded).map((folder) => [folderId(folder), true]),
              )}
              disableMultiSelection
              disableEdit
              disableDrag
              disableDrop
              disableSelect={(entry) => entry.kind === 'status'}
              // Arborist memoizes on its props as a list, so every prop is always passed: no
              // selection is the empty string.
              selection={selected ?? ''}
              onToggle={(id) => {
                if (!id.startsWith('folder:')) return;
                const open = treeRef.current?.isOpen(id) ?? false;
                setExpanded(workspaceId, id.slice('folder:'.length), open);
              }}
              onActivate={activate}
              onFocus={(node) => {
                lastFocusIndex.current = node.rowIndex;
                setCurrentId(node.id);
              }}
              rowHeight={rowHeight}
              indent={16}
              width="100%"
              height={height}
              renderRow={TreeRow}
              outerElementType={TreeScroller}
            >
              {TreeEntryContent}
            </Tree>
          </TreeRowContext.Provider>
        )}
      </div>
    </section>
  );
}

function extractionWord(item: ItemSummary | null): string | null {
  return item?.extraction && item.extraction.status !== 'completed'
    ? EXTRACTION_WORD[item.extraction.status]
    : null;
}

/** An item's status word when it is not the ordinary one: "status draft", never just "draft". */
function statusWord(item: ItemSummary | null): string | null {
  return item === null || item.status === 'stable' ? null : `status ${item.status}`;
}

/** The row's accessible name: the label, then the type name and any conversion state. */
function entryLabel(entry: TreeEntry): string {
  if (entry.item === null) return entry.name;
  const extraction = extractionWord(entry.item);
  const type = `${entry.name}, type ${entry.item.type_name}`;
  const converted = extraction === null ? type : `${type}, ${extraction}`;
  const status = statusWord(entry.item);
  const stated = status === null ? converted : `${converted}, ${status}`;
  return entry.mark === null ? stated : `${stated}, ${entry.mark}`;
}

/**
 * Arborist's row, with `aria-expanded` only on folders and Enter opening the entry. The tree is
 * virtualized, so not every entry is in the DOM: each row states its place among its siblings
 * (`aria-posinset` of `aria-setsize`), as the WAI-ARIA tree pattern asks.
 *
 * One row is the tree's tab stop (roving tabindex): see `tabStopOf`. Shift+Tab lands on it
 * directly when it is rendered; arborist keeps Tab inside the tree to one stop. Focus that reaches
 * a row directly is told to arborist, whose arrow keys move from the entry it knows is focused.
 */
function TreeRow({ node, attrs, innerRef, children }: RowRendererProps<TreeEntry>) {
  const { 'aria-expanded': _expanded, style, ...rest } = attrs;
  const siblings = node.parent?.children?.length ?? 1;
  const tree = node.tree;
  const stop = tabStopOf(tree);
  const { onKey } = useContext(TreeRowContext);
  return (
    <div
      {...rest}
      // Arborist lets a row grow to `max-content`, which makes the list scroll sideways and
      // hides a long name from the keyboard. Rows stay as wide as the column and wrap instead.
      style={{ ...style, minWidth: 0 }}
      role="treeitem"
      data-entry={node.id}
      tabIndex={stop?.id === node.id ? 0 : -1}
      aria-label={entryLabel(node.data)}
      aria-expanded={node.data.kind === 'folder' ? node.isOpen : undefined}
      aria-setsize={siblings}
      aria-posinset={node.childIndex + 1}
      ref={innerRef}
      onFocus={(event) => {
        if (event.target === event.currentTarget && !node.isFocused) {
          tree.focus(node.id, { scroll: false });
        }
      }}
      onClick={node.handleClick}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          node.activate();
        } else if (onKey(node, event.key)) {
          // The key opened a form: arborist's own handling of it (Delete) is not wanted as well.
          event.preventDefault();
          event.stopPropagation();
        }
      }}
    >
      {children}
    </div>
  );
}

/**
 * The tree's scrolling list (react-window's outer element). It is in the Tab order itself, so the
 * region a keyboard scrolls is reachable at every scroll offset, even when the tab-stop row is
 * scrolled out of the rendered window (axe scrollable-region-focusable). Its role is `group`,
 * which a tree may own, so the rows inside keep their tree semantics (axe aria-required-children).
 * Focus arriving here moves on to the tab-stop entry (`enterTree`).
 */
const TreeScroller = forwardRef<HTMLDivElement, HTMLProps<HTMLDivElement>>(
  function TreeScroller(props, ref) {
    return <ListOuterElement {...props} ref={ref} role="group" tabIndex={0} />;
  },
);

/**
 * What rows share with the tree: `measure` reports a row's measured height. Rows wrap long names
 * inside the column instead of scrolling sideways, so their heights vary, and the virtualized list
 * re-lays out from that row on.
 */
const TreeRowContext = createContext<{
  measure: (id: string, index: number, height: number) => void;
  /** A key pressed on a row; true when it opened a form. */
  onKey: (node: NodeApi<TreeEntry>, key: string) => boolean;
}>({ measure: () => {}, onKey: () => false });

function TreeEntryContent({ node, style }: NodeRendererProps<TreeEntry>) {
  const { measure } = useContext(TreeRowContext);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const { id, rowIndex } = node;
  useLayoutEffect(() => {
    const element = contentRef.current;
    if (element === null) return;
    const report = () => measure(id, rowIndex ?? 0, element.getBoundingClientRect().height);
    report();
    const observer = new ResizeObserver(report);
    observer.observe(element);
    return () => observer.disconnect();
  }, [id, rowIndex, measure]);
  const entry = node.data;
  const item = entry.item;
  const Icon =
    entry.kind === 'folder'
      ? node.isOpen
        ? FolderOpen
        : Folder
      : item === null
        ? null
        : ROLE_ICON[item.kind];
  const extraction = extractionWord(item);
  return (
    <div ref={contentRef} style={style} className={`tree-entry tree-${entry.kind}`}>
      {Icon === null ? null : <Icon aria-hidden="true" size={16} className="tree-icon" />}
      {/* One text block beside the icon, so a long name wraps next to it, not under it. */}
      <span className="tree-text">
        <span className="tree-name">{entry.name}</span>
        {item === null ? null : <span className="tree-type">{item.type_name}</span>}
        {extraction === null ? null : <span className="tree-flag">{extraction}</span>}
        {statusWord(item) === null ? null : <span className="tree-flag">{statusWord(item)}</span>}
        {entry.mark === null ? null : <span className="tree-flag">{entry.mark}</span>}
      </span>
    </div>
  );
}
