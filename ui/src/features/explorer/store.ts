/**
 * Ephemeral Explorer state: open tabs and panes, expanded folders, pane sizes.
 *
 * It holds item and folder references only. Titles, paths, revisions and content live in the
 * Query cache and are read from there; nothing the server owns is copied here. The router owns
 * which item the focused pane shows; this store only remembers the other tabs. Nothing here is
 * persisted: it lasts as long as the page.
 */
import { create } from 'zustand';

export interface Pane {
  id: string;
  /** Item identities, in tab order. */
  tabs: string[];
  active: string | null;
}

export interface WorkspaceView {
  panes: Pane[];
  focusedPaneId: string;
  /** Folder paths whose children are shown. */
  expanded: Record<string, true>;
  /** How many listing pages each folder shows. */
  folderPages: Record<string, number>;
}

/** Sizes of a resizable group, by panel id, as percentages. */
export type PanelLayout = Record<string, number>;

/** At most this many panes side by side. */
export const MAX_PANES = 3;

interface ExplorerState {
  views: Record<string, WorkspaceView>;
  layouts: Record<string, PanelLayout>;
  nextPane: number;
  /** Show `itemId` in the focused pane, adding a tab when it has none. */
  openItem: (workspaceId: string, itemId: string) => void;
  focusPane: (workspaceId: string, paneId: string) => void;
  /** Close a tab; returns the item that pane shows next, if any. */
  closeTab: (workspaceId: string, paneId: string, itemId: string) => string | null;
  /** Open `itemId` in a new pane to the right of `paneId` and focus it. */
  splitRight: (workspaceId: string, paneId: string, itemId: string) => void;
  setExpanded: (workspaceId: string, folder: string, open: boolean) => void;
  showMore: (workspaceId: string, folder: string) => void;
  setLayout: (group: string, layout: PanelLayout) => void;
}

const FIRST_PANE = 'pane-1';

export function emptyView(): WorkspaceView {
  return {
    panes: [{ id: FIRST_PANE, tabs: [], active: null }],
    focusedPaneId: FIRST_PANE,
    expanded: {},
    folderPages: {},
  };
}

function updatePane(view: WorkspaceView, paneId: string, change: (pane: Pane) => Pane) {
  return view.panes.map((pane) => (pane.id === paneId ? change(pane) : pane));
}

export const useExplorerStore = create<ExplorerState>()((set, get) => {
  const view = (workspaceId: string) => get().views[workspaceId] ?? emptyView();
  const write = (workspaceId: string, next: WorkspaceView) =>
    set((state) => ({ views: { ...state.views, [workspaceId]: next } }));
  return {
    views: {},
    layouts: {},
    nextPane: 2,
    openItem: (workspaceId, itemId) => {
      const current = view(workspaceId);
      const panes = updatePane(current, current.focusedPaneId, (pane) => ({
        ...pane,
        tabs: pane.tabs.includes(itemId) ? pane.tabs : [...pane.tabs, itemId],
        active: itemId,
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
      const pane = current.panes.find((candidate) => candidate.id === paneId);
      if (pane === undefined) return null;
      const index = pane.tabs.indexOf(itemId);
      const tabs = pane.tabs.filter((tab) => tab !== itemId);
      const active =
        pane.active === itemId ? (tabs[Math.min(index, tabs.length - 1)] ?? null) : pane.active;
      if (tabs.length === 0 && current.panes.length > 1) {
        const panes = current.panes.filter((candidate) => candidate.id !== paneId);
        const focused = current.focusedPaneId === paneId ? panes[0] : undefined;
        write(workspaceId, {
          ...current,
          panes,
          focusedPaneId: focused?.id ?? current.focusedPaneId,
        });
        return focused?.active ?? null;
      }
      write(workspaceId, {
        ...current,
        panes: updatePane(current, paneId, () => ({ id: paneId, tabs, active })),
      });
      return active;
    },
    splitRight: (workspaceId, paneId, itemId) => {
      const current = view(workspaceId);
      if (current.panes.length >= MAX_PANES) return;
      const at = current.panes.findIndex((pane) => pane.id === paneId);
      if (at < 0) return;
      const id = `pane-${get().nextPane}`;
      const panes = [...current.panes];
      panes.splice(at + 1, 0, { id, tabs: [itemId], active: itemId });
      set((state) => ({ nextPane: state.nextPane + 1 }));
      write(workspaceId, { ...current, panes, focusedPaneId: id });
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
  };
});

export function useWorkspaceView(workspaceId: string): WorkspaceView {
  return useExplorerStore((state) => state.views[workspaceId]) ?? EMPTY;
}

const EMPTY = emptyView();
