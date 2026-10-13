/**
 * Ephemeral Explorer state: open tabs and panes, expanded folders, pane sizes and where keyboard
 * focus goes next.
 *
 * It holds references only: item ids, folder paths, and for each open tab the selection and cited
 * revision its URL last named (what the tab shows when it is not the one the URL names). Titles,
 * paths, the workspace head and content live in the Query cache and are read from there; nothing
 * the server owns is copied here. The router owns what the focused pane shows; this store
 * remembers the rest. Nothing here is persisted: it lasts as long as the page.
 *
 * It also carries what one part of the page asks of another after a change of the tree: the row
 * the tree is to show focused, the note that is to open on its editor, and which items the person
 * deleted here (so their tabs can say so). The last is what the person did in this page, by item
 * id and the title it had; it is not a copy of what the service holds, and an item brought back
 * later (Rewind, a later package) must be taken out of it.
 */
import { create } from 'zustand';
import type { ItemSelection } from './search-params';

/** What a pane's URL names besides the item: a selection inside it and a cited revision. */
export interface PaneLocation {
  sel?: ItemSelection | undefined;
  rev?: string | undefined;
}

export interface Pane {
  id: string;
  /** Item identities, in tab order. */
  tabs: string[];
  active: string | null;
  /**
   * For each open tab, by item id: the selection and cited revision its URL last named. A tab
   * returned to reads where it read, so a revision a link resolved is never dropped on the way.
   */
  locations: Record<string, PaneLocation>;
}

export interface WorkspaceView {
  panes: Pane[];
  focusedPaneId: string;
  /** Folder paths whose children are shown. */
  expanded: Record<string, true>;
  /** How many listing pages each folder shows. */
  folderPages: Record<string, number>;
}

/** Where keyboard focus goes once the documents area has re-rendered. */
export type FocusTarget =
  | { kind: 'tab'; paneId: string; itemId: string }
  | { kind: 'empty'; paneId: string };

/** What closing a tab leaves: the focused pane, what it shows, and where focus goes. */
export interface CloseResult {
  focusedPaneId: string;
  /** Whether the focused pane changed or its active item changed, so the URL must follow. */
  focusedChanged: boolean;
  /** The focused pane's active item and location afterwards, if any. */
  show: { itemId: string; location: PaneLocation } | null;
  focus: FocusTarget;
}

/**
 * Where keyboard focus goes in the folder tree once it lists the head a change made: the row of
 * what was created or moved, or the entry now at the place of what was deleted.
 */
export type TreeReveal = {
  workspaceId: string;
  /** The revision the change made: the tree shows the row once it lists that head. */
  revision: string;
} & (
  | { kind: 'item'; itemId: string; path: string }
  | { kind: 'folder'; folder: string }
  | { kind: 'place'; index: number }
);

/** Sizes of a resizable group, by panel id, as percentages. */
export type PanelLayout = Record<string, number>;

/** At most this many panes side by side. */
export const MAX_PANES = 3;

interface ExplorerState {
  views: Record<string, WorkspaceView>;
  layouts: Record<string, PanelLayout>;
  nextPane: number;
  focusRequest: FocusTarget | null;
  /** What the folder tree is to show focused next, once it lists it. */
  treeReveal: TreeReveal | null;
  /** The note just created: its pane opens on its editor, with keyboard focus in the text. */
  editRequest: { workspaceId: string; itemId: string } | null;
  /**
   * The items the person deleted in this page, by workspace and item, with the title each had: a
   * tab of one says so. It is what the person did here, not a copy of what the service holds.
   */
  deleted: Record<string, string>;
  revealInTree: (reveal: TreeReveal | null) => void;
  requestEdit: (request: { workspaceId: string; itemId: string } | null) => void;
  markDeleted: (workspaceId: string, itemId: string, title: string) => void;
  /** Show `itemId` at `location` in the focused pane, adding a tab when it has none. */
  openItem: (workspaceId: string, itemId: string, location?: PaneLocation) => void;
  focusPane: (workspaceId: string, paneId: string) => void;
  /**
   * Close a tab. Focus goes to the tab after it, else the one before (WAI-ARIA tabs); a pane left
   * empty closes and focus goes to its neighbour's active tab; with nothing open, to the empty
   * documents area.
   */
  closeTab: (workspaceId: string, paneId: string, itemId: string) => CloseResult | null;
  /** Open `itemId` at `location` in a new pane to the right of `paneId` and focus it. */
  splitRight: (workspaceId: string, paneId: string, itemId: string, location: PaneLocation) => void;
  /**
   * After a Snapshot: every tab of these items that cited revision `from` cites `to`, so it reads
   * what was just committed instead of the version before it.
   */
  repin: (workspaceId: string, itemIds: readonly string[], from: string, to: string) => void;
  setExpanded: (workspaceId: string, folder: string, open: boolean) => void;
  showMore: (workspaceId: string, folder: string) => void;
  setLayout: (group: string, layout: PanelLayout) => void;
  requestFocus: (target: FocusTarget) => void;
  clearFocusRequest: () => void;
}

const FIRST_PANE = 'pane-1';

export function emptyView(): WorkspaceView {
  return {
    panes: [{ id: FIRST_PANE, tabs: [], active: null, locations: {} }],
    focusedPaneId: FIRST_PANE,
    expanded: {},
    folderPages: {},
  };
}

/** Where a pane's tab for `itemId` reads: its selection and cited revision ({} for none). */
export function tabLocation(pane: Pane, itemId: string | null): PaneLocation {
  return itemId === null ? {} : (pane.locations[itemId] ?? {});
}

function updatePane(view: WorkspaceView, paneId: string, change: (pane: Pane) => Pane) {
  return view.panes.map((pane) => (pane.id === paneId ? change(pane) : pane));
}

function sameLocation(a: PaneLocation, b: PaneLocation): boolean {
  return (
    JSON.stringify([a.sel ?? null, a.rev ?? null]) ===
    JSON.stringify([b.sel ?? null, b.rev ?? null])
  );
}

function cleanLocation(location: PaneLocation): PaneLocation {
  return {
    ...(location.sel === undefined ? {} : { sel: location.sel }),
    ...(location.rev === undefined ? {} : { rev: location.rev }),
  };
}

export const useExplorerStore = create<ExplorerState>()((set, get) => {
  const view = (workspaceId: string) => get().views[workspaceId] ?? emptyView();
  const write = (workspaceId: string, next: WorkspaceView) =>
    set((state) => ({ views: { ...state.views, [workspaceId]: next } }));
  return {
    views: {},
    layouts: {},
    nextPane: 2,
    focusRequest: null,
    treeReveal: null,
    editRequest: null,
    deleted: {},
    revealInTree: (reveal) => set({ treeReveal: reveal }),
    requestEdit: (request) => set({ editRequest: request }),
    markDeleted: (workspaceId, itemId, title) =>
      set((state) => ({ deleted: { ...state.deleted, [`${workspaceId}/${itemId}`]: title } })),
    openItem: (workspaceId, itemId, location = {}) => {
      const current = view(workspaceId);
      const pane = current.panes.find((candidate) => candidate.id === current.focusedPaneId);
      if (
        pane !== undefined &&
        pane.active === itemId &&
        pane.tabs.includes(itemId) &&
        sameLocation(tabLocation(pane, itemId), location)
      ) {
        return;
      }
      const panes = updatePane(current, current.focusedPaneId, (focused) => ({
        ...focused,
        tabs: focused.tabs.includes(itemId) ? focused.tabs : [...focused.tabs, itemId],
        active: itemId,
        locations: { ...focused.locations, [itemId]: cleanLocation(location) },
      }));
      write(workspaceId, { ...current, panes });
    },
    focusPane: (workspaceId, paneId) => {
      const current = view(workspaceId);
      if (current.focusedPaneId === paneId) return;
      if (!current.panes.some((pane) => pane.id === paneId)) return;
      write(workspaceId, { ...current, focusedPaneId: paneId });
    },
    closeTab: (workspaceId, paneId, itemId) => {
      const current = view(workspaceId);
      const at = current.panes.findIndex((candidate) => candidate.id === paneId);
      const pane = current.panes[at];
      if (pane === undefined) return null;
      const index = pane.tabs.indexOf(itemId);
      if (index < 0) return null;
      const tabs = pane.tabs.filter((tab) => tab !== itemId);
      // The tab after the closed one, else the one before it.
      const following = tabs[index] ?? tabs[index - 1] ?? null;
      const closedActive = pane.active === itemId;
      const active = closedActive ? following : pane.active;
      const { [itemId]: _closed, ...locations } = pane.locations;
      const wasFocused = current.focusedPaneId === paneId;

      if (tabs.length === 0 && current.panes.length > 1) {
        const panes = current.panes.filter((candidate) => candidate.id !== paneId);
        const neighbour = current.panes[at + 1] ?? current.panes[at - 1];
        const focusedPaneId = wasFocused && neighbour ? neighbour.id : current.focusedPaneId;
        write(workspaceId, { ...current, panes, focusedPaneId });
        const focusedPane = panes.find((candidate) => candidate.id === focusedPaneId);
        const shown = focusedPane?.active ?? null;
        return {
          focusedPaneId,
          focusedChanged: wasFocused,
          show:
            focusedPane && shown !== null
              ? { itemId: shown, location: tabLocation(focusedPane, shown) }
              : null,
          focus:
            neighbour !== undefined && neighbour.active !== null
              ? { kind: 'tab', paneId: neighbour.id, itemId: neighbour.active }
              : { kind: 'empty', paneId: neighbour?.id ?? current.focusedPaneId },
        };
      }

      write(workspaceId, {
        ...current,
        panes: updatePane(current, paneId, () => ({ id: paneId, tabs, active, locations })),
      });
      const location = active === null ? {} : (locations[active] ?? {});
      return {
        focusedPaneId: current.focusedPaneId,
        focusedChanged: wasFocused && closedActive,
        show: active === null ? null : { itemId: active, location },
        focus:
          following === null
            ? { kind: 'empty', paneId }
            : { kind: 'tab', paneId, itemId: following },
      };
    },
    splitRight: (workspaceId, paneId, itemId, location) => {
      const current = view(workspaceId);
      if (current.panes.length >= MAX_PANES) return;
      const at = current.panes.findIndex((pane) => pane.id === paneId);
      if (at < 0) return;
      const id = `pane-${get().nextPane}`;
      const panes = [...current.panes];
      panes.splice(at + 1, 0, {
        id,
        tabs: [itemId],
        active: itemId,
        locations: { [itemId]: cleanLocation(location) },
      });
      set((state) => ({ nextPane: state.nextPane + 1 }));
      write(workspaceId, { ...current, panes, focusedPaneId: id });
    },
    repin: (workspaceId, itemIds, from, to) => {
      const current = view(workspaceId);
      const panes = current.panes.map((pane) => ({
        ...pane,
        locations: Object.fromEntries(
          Object.entries(pane.locations).map(([itemId, location]) => [
            itemId,
            itemIds.includes(itemId) && location.rev === from ? { ...location, rev: to } : location,
          ]),
        ),
      }));
      write(workspaceId, { ...current, panes });
    },
    setExpanded: (workspaceId, folder, open) => {
      const current = view(workspaceId);
      if (Boolean(current.expanded[folder]) === open) return;
      const expanded = { ...current.expanded };
      if (open) expanded[folder] = true;
      else delete expanded[folder];
      write(workspaceId, { ...current, expanded });
    },
    showMore: (workspaceId, folder) => {
      const current = view(workspaceId);
      const pages = (current.folderPages[folder] ?? 1) + 1;
      write(workspaceId, { ...current, folderPages: { ...current.folderPages, [folder]: pages } });
    },
    setLayout: (group, layout) =>
      set((state) => ({ layouts: { ...state.layouts, [group]: layout } })),
    requestFocus: (target) => set({ focusRequest: target }),
    clearFocusRequest: () => set({ focusRequest: null }),
  };
});

export function useWorkspaceView(workspaceId: string): WorkspaceView {
  return useExplorerStore((state) => state.views[workspaceId]) ?? EMPTY;
}

const EMPTY = emptyView();

/** The title an item had when the person deleted it in this page; undefined when they did not. */
export function useDeleted(workspaceId: string, itemId: string): string | undefined {
  return useExplorerStore((state) => state.deleted[`${workspaceId}/${itemId}`]);
}
