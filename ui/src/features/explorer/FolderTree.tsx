/**
 * The folder tree: react-arborist over the listing operation, loaded folder by folder.
 *
 * Folders and items appear as the workspace organizes them. Each item shows its title and its
 * user type name (an open vocabulary); its rendering role (Note, Source or View) picks the icon.
 * Arborist supplies the tree semantics and keyboard model (arrows, Home/End, type-ahead, Space);
 * Enter also opens the focused entry. The open item of the URL is the tree's selection, and its
 * folders are expanded when it is opened from a link. "Show more entries" and a retry move
 * keyboard focus to the first entry they brought (or keep it on a row still there), and a live
 * region says what happened.
 */
import { type UseQueryResult, useQueries, useQueryClient } from '@tanstack/react-query';
import { ChartColumn, FileBox, FileText, Folder, FolderOpen } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import {
  type NodeApi,
  type NodeRendererProps,
  type RowRendererProps,
  Tree,
  type TreeApi,
} from 'react-arborist';
import type { ItemSummary, Revision } from '../../api/generated/types.gen';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, messageFor } from '../errors/failure';
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
    status,
    error,
    children: null,
  };
}

function entriesOf(
  folder: string,
  listings: ReadonlyMap<string, UseQueryResult<FolderListing>>,
  expanded: Record<string, true>,
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
        status: null,
        error: null,
        children: expanded[child] ? entriesOf(child, listings, expanded) : [],
      });
    }
    for (const item of page.items) {
      entries.push({
        id: itemNodeId(item.id),
        name: item.title.length > 0 ? item.title : baseName(item.path),
        kind: 'item',
        folder,
        item,
        status: null,
        error: null,
        children: null,
      });
    }
  }
  const last = result.data.pages.at(-1);
  if (result.isPlaceholderData) entries.push(statusEntry(folder, 'more-loading'));
  else if (result.data.moreError !== null) {
    entries.push(statusEntry(folder, 'more-error', result.data.moreError));
  } else if (last?.next_cursor) entries.push(statusEntry(folder, 'more'));
  return entries;
}

function contains(entries: readonly TreeEntry[], id: string): boolean {
  return entries.some((entry) => entry.id === id || contains(entry.children ?? [], id));
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
  const entries = entriesOf('', listings, view.expanded);
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
    <section className="folder-tree" aria-labelledby={heading}>
      <h2 id={heading} className="panel-heading">
        Folders
      </h2>
      {root?.isPending && <p role="status">Loading folders…</p>}
      {root?.isError && (
        <FailureNotice
          context="Listing this workspace"
          error={root.error}
          onRetry={() => void root.refetch()}
        />
      )}
      {root?.isSuccess && entries.length === 0 && (
        <p>This workspace has no folders or items yet.</p>
      )}
      <p role="status" className="sr-only">
        {loadNote}
      </p>
      <div ref={boxRef} className="tree-box">
        {root?.isSuccess && entries.length > 0 && (
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
            rowHeight={ROW_HEIGHT}
            indent={16}
            width="100%"
            height={height}
            renderRow={TreeRow}
          >
            {TreeEntryContent}
          </Tree>
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

/** The row's accessible name: the label, then the type name and any conversion state. */
function entryLabel(entry: TreeEntry): string {
  if (entry.item === null) return entry.name;
  const extraction = extractionWord(entry.item);
  const type = `${entry.name}, type ${entry.item.type_name}`;
  return extraction === null ? type : `${type}, ${extraction}`;
}

/**
 * Arborist's row, with `aria-expanded` only on folders and Enter opening the entry. The tree is
 * virtualized, so not every entry is in the DOM: each row states its place among its siblings
 * (`aria-posinset` of `aria-setsize`), as the WAI-ARIA tree pattern asks.
 */
function TreeRow({ node, attrs, innerRef, children }: RowRendererProps<TreeEntry>) {
  const { 'aria-expanded': _expanded, ...rest } = attrs;
  const siblings = node.parent?.children?.length ?? 1;
  return (
    <div
      {...rest}
      role="treeitem"
      tabIndex={-1}
      aria-label={entryLabel(node.data)}
      aria-expanded={node.data.kind === 'folder' ? node.isOpen : undefined}
      aria-setsize={siblings}
      aria-posinset={node.childIndex + 1}
      ref={innerRef}
      onFocus={(event) => event.stopPropagation()}
      onClick={node.handleClick}
      onKeyDown={(event) => {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        node.activate();
      }}
    >
      {children}
    </div>
  );
}

function TreeEntryContent({ node, style }: NodeRendererProps<TreeEntry>) {
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
    <div style={style} className={`tree-entry tree-${entry.kind}`}>
      {Icon === null ? null : <Icon aria-hidden="true" size={16} />}
      <span className="tree-name">{entry.name}</span>
      {item === null ? null : <span className="tree-type">{item.type_name}</span>}
      {extraction === null ? null : <span className="tree-flag">{extraction}</span>}
    </div>
  );
}
