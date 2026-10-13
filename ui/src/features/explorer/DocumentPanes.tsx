/**
 * The document area: tabs of open items that split into side-by-side resizable panes.
 *
 * Base UI Tabs give each pane a tab list with arrow-key movement and Enter/Space activation;
 * Delete closes the focused tab. Closing never drops keyboard focus to the page: it moves to the
 * tab after the closed one, else the one before; when a pane closes, to its neighbour's active
 * tab; when nothing is open, to the empty documents area. The close and split buttons sit beside
 * the tab list, not in it, so the tab list holds only tabs. react-resizable-panels separators resize panes with the
 * keyboard as well as the pointer. The focused pane is the one the URL describes; with several
 * panes its region name ends in ", focused", so assistive technology can tell which one it is.
 *
 * A tab's name is the item's title at the revision the tab shows: the one it cites, else the head
 * the workspace shows. So a tab opened from a pinned link has the title its text has, whatever the
 * item is called, or whether it exists, at the head. The name also carries how the item's draft
 * stands, in brackets after the title: "(draft)", "(unsaved)" or "(not saved)". The mark is the
 * item's, read at the head, so a tab that shows a cited revision has it too. The tab of an item the
 * person deleted in this page reads "(deleted)" after the title it had, and asks nothing at the
 * head.
 *
 * A tab also carries what is said of its item besides its name, in brackets after the title: for
 * a source that is anything but converted, its conversion mark at the revision the tab shows
 * ("(conversion pending)", "(format not supported)"), as its row of the tree has it, and how many
 * listed Attention warnings name it ("(2 warnings, advisory)"). These words are the tab's
 * description (`aria-describedby`), not part of its name: the name stays the item's title, and
 * the words are read after it.
 */
import { Tabs } from '@base-ui/react/tabs';
import { useQuery } from '@tanstack/react-query';
import { Fragment, useEffect, useRef } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import type { Revision } from '../../api/generated/types.gen';
import { attentionMark, useAttentionMarks } from '../attention/attention';
import { DocumentView, itemOptions } from '../documents/DocumentView';
import { conversionWord } from '../documents/describe';
import { useDraftMarks } from '../drafts/marks';
import type { Snapshotted } from '../drafts/snapshot';
import type { ItemSelection } from './search-params';
import {
  MAX_PANES,
  type Pane,
  type PaneLocation,
  type PanelLayout,
  tabLocation,
  useDeleted,
  useExplorerStore,
} from './store';

export interface DocumentPanesProps {
  workspaceId: string;
  head: Revision;
  panes: readonly Pane[];
  focusedPaneId: string;
  /** The item the URL names: what the focused pane shows, whatever tab it last had open. */
  urlItemId: string | null;
  /** The URL names an item id the contract rejects; nothing was requested for it. */
  invalidItemLink: boolean;
  /**
   * The URL's selection and cited revision, which apply to the focused pane's open item. Every
   * other pane shows its own, as its URL last named them.
   */
  selection: ItemSelection | undefined;
  revision: Revision | undefined;
  onActivate: (paneId: string, itemId: string) => void;
  onFocusPane: (paneId: string) => void;
  onClose: (paneId: string, itemId: string) => void;
  onSplit: (paneId: string, itemId: string) => void;
  /** Show another selection of an item, keeping the revision that pane reads. */
  onSelect: (itemId: string, location: PaneLocation) => void;
  /** The latest version was read: pin the pane to the revision the server resolved. */
  onReadLatest: (itemId: string, location: PaneLocation) => void;
  /** The person may write in this workspace. */
  canWrite: boolean;
  /** Open an item as the workspace shows it, without a cited revision. */
  onOpenCurrent: (itemId: string) => void;
  /** A Snapshot was taken from a pane ("keep mine"). */
  onSnapshotted: (snapshotted: Snapshotted) => void;
}

const PANES_GROUP = 'document-panes';

/** A remembered layout applies only while it names exactly the panels on screen. */
export function matchingLayout(
  layout: PanelLayout | undefined,
  ids: readonly string[],
): PanelLayout | undefined {
  if (layout === undefined) return undefined;
  const keys = Object.keys(layout);
  return keys.length === ids.length && ids.every((id) => keys.includes(id)) ? layout : undefined;
}

export function DocumentPanes(props: DocumentPanesProps) {
  const { panes, workspaceId } = props;
  const layouts = useExplorerStore((state) => state.layouts);
  const setLayout = useExplorerStore((state) => state.setLayout);
  const group = `${PANES_GROUP}:${workspaceId}`;
  const layout = matchingLayout(
    layouts[group],
    panes.map((pane) => pane.id),
  );
  return (
    <Group
      orientation="horizontal"
      id={group}
      className="pane-group"
      {...(layout === undefined ? {} : { defaultLayout: layout })}
      onLayoutChanged={(next) => setLayout(group, next)}
    >
      {panes.map((pane, index) => (
        <Fragment key={pane.id}>
          {index > 0 && (
            <Separator
              className="separator"
              aria-label={`Resize panes ${index} and ${index + 1}`}
            />
          )}
          <Panel id={pane.id} minSize="15%">
            <DocumentPane {...props} pane={pane} index={index} />
          </Panel>
        </Fragment>
      ))}
    </Group>
  );
}

function DocumentPane({
  workspaceId,
  head,
  panes,
  focusedPaneId,
  urlItemId,
  invalidItemLink,
  selection,
  revision,
  pane,
  index,
  onActivate,
  onFocusPane,
  onClose,
  onSplit,
  onSelect,
  onReadLatest,
  canWrite,
  onOpenCurrent,
  onSnapshotted,
}: DocumentPanesProps & { pane: Pane; index: number }) {
  const focused = pane.id === focusedPaneId;
  // With several panes, the name says which one the URL describes, not only the inset line.
  const label =
    panes.length > 1
      ? `Documents, pane ${index + 1} of ${panes.length}${focused ? ', focused' : ''}`
      : 'Documents';
  const active = focused ? urlItemId : pane.active;
  // The focused pane shows what the URL names; another pane keeps showing what it showed.
  const shownSelection = focused ? selection : tabLocation(pane, pane.active).sel;
  const citedRevision = focused ? revision : tabLocation(pane, pane.active).rev;
  const canSplit = active !== null && panes.length < MAX_PANES;

  // After a tab or pane closes, keyboard focus goes to the tab or empty area chosen for it,
  // once that element is on screen (the URL may still be catching up).
  const sectionRef = useRef<HTMLElement | null>(null);
  const focusRequest = useExplorerStore((state) => state.focusRequest);
  const clearFocusRequest = useExplorerStore((state) => state.clearFocusRequest);
  useEffect(() => {
    if (focusRequest === null || focusRequest.paneId !== pane.id) return;
    const selector =
      focusRequest.kind === 'tab'
        ? `[role="tab"][data-item-id="${focusRequest.itemId}"]`
        : '[data-empty-documents]';
    const target = sectionRef.current?.querySelector(selector);
    if (target instanceof HTMLElement) {
      target.focus();
      clearFocusRequest();
    }
  });

  return (
    <section
      ref={sectionRef}
      aria-label={label}
      className={focused ? 'pane pane-focused' : 'pane'}
      onFocusCapture={() => {
        if (!focused) onFocusPane(pane.id);
      }}
    >
      {focused && invalidItemLink && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>This link is not valid.</strong>
          </p>
          <p>
            The address does not name an item in this workspace, so nothing was requested. Open an
            item from the folders or the search results.
          </p>
        </div>
      )}
      {pane.tabs.length === 0 && active === null ? (
        <p className="empty-pane" tabIndex={-1} data-empty-documents>
          Open an item from the folders or the search results.
        </p>
      ) : (
        <Tabs.Root
          value={active}
          onValueChange={(value) => {
            if (typeof value === 'string') onActivate(pane.id, value);
          }}
          className="tabs"
        >
          <div className="tab-bar">
            <Tabs.List aria-label="Open items" className="tab-list">
              {pane.tabs.map((itemId) => (
                <Tabs.Tab
                  key={itemId}
                  value={itemId}
                  data-item-id={itemId}
                  aria-describedby={tabStateId(pane.id, itemId)}
                  className="tab"
                  onKeyDown={(event) => {
                    if (event.key !== 'Delete') return;
                    event.preventDefault();
                    onClose(pane.id, itemId);
                  }}
                >
                  <TabLabel
                    workspaceId={workspaceId}
                    itemId={itemId}
                    head={head}
                    stateId={tabStateId(pane.id, itemId)}
                    // The focused pane's open tab cites what the URL does; every other tab, what
                    // its own link last named.
                    cited={focused && itemId === active ? revision : tabLocation(pane, itemId).rev}
                  />
                </Tabs.Tab>
              ))}
            </Tabs.List>
            {/* aria-disabled rather than disabled: a press can make these unavailable while they
                have focus (the third pane makes "Split right" unavailable; going back to no
                item, "Close tab"), and Chromium and WebKit move focus to the body within a frame
                of a focused button becoming disabled. Each click checks for itself. */}
            <div className="pane-actions">
              <button
                type="button"
                aria-disabled={!canSplit}
                onClick={() => {
                  if (canSplit && active !== null) onSplit(pane.id, active);
                }}
              >
                Split right
              </button>
              <button
                type="button"
                aria-disabled={active === null}
                onClick={() => {
                  if (active !== null) onClose(pane.id, active);
                }}
              >
                Close tab
              </button>
            </div>
          </div>
          {active === null ? (
            <p className="empty-pane" tabIndex={-1} data-empty-documents>
              Choose one of the open items, or open another.
            </p>
          ) : (
            <Tabs.Panel value={active} className="tab-panel">
              <DocumentView
                key={active}
                workspaceId={workspaceId}
                itemId={active}
                revision={citedRevision ?? head}
                pinned={citedRevision !== undefined}
                selection={shownSelection}
                onSelect={(next) =>
                  onSelect(active, {
                    ...(next === undefined ? {} : { sel: next }),
                    ...(citedRevision === undefined ? {} : { rev: citedRevision }),
                  })
                }
                onReadLatest={(resolved) =>
                  onReadLatest(active, {
                    ...(shownSelection === undefined ? {} : { sel: shownSelection }),
                    rev: resolved,
                  })
                }
                head={head}
                canWrite={canWrite}
                onOpenCurrent={() => onOpenCurrent(active)}
                onSnapshotted={onSnapshotted}
                onClose={() => onClose(pane.id, active)}
              />
            </Tabs.Panel>
          )}
        </Tabs.Root>
      )}
    </section>
  );
}

/** The identity of the element that describes a tab: one for each tab of each pane. */
const tabStateId = (paneId: string, itemId: string) => `tab-state-${paneId}-${itemId}`;

/**
 * A tab's name is the item's title (until it is known, a short identity), then how the item's
 * draft stands. The title is read at the revision the tab shows: `cited` when the tab cites one,
 * else the head. The draft's mark is read at the head the workspace shows, whatever the tab reads.
 * For a tab that cites nothing the two are one read.
 *
 * The Attention mark counts the warnings found at the head, so only a tab that shows the head
 * carries it. A tab that cites another revision carries none: the count would be of a revision
 * the tab does not show. The reader's notice, which names the revision the warnings were found
 * at, stands for it there.
 */
function TabLabel({
  workspaceId,
  itemId,
  head,
  stateId,
  cited,
}: {
  workspaceId: string;
  itemId: string;
  head: Revision;
  /** The identity of the element that holds the tab's description. */
  stateId: string;
  cited: Revision | undefined;
}) {
  // An item the person deleted in this page is not at the head: its tab says so, by the title
  // it had, and nothing is asked at the head. A revision the tab cites is still read (the pane
  // shows it under the notice).
  const deleted = useDeleted(workspaceId, itemId);
  const shown = useQuery({
    ...itemOptions(workspaceId, itemId, cited ?? head),
    enabled: deleted === undefined || cited !== undefined,
  });
  const item = useQuery({
    ...itemOptions(workspaceId, itemId, head),
    enabled: deleted === undefined,
  });
  const summary = shown.data?.summary;
  const name =
    deleted !== undefined
      ? `${deleted} (deleted)`
      : summary
        ? summary.title || summary.path
        : `Item ${itemId.slice(0, 8)}`;
  // The service holds a draft when `get_item` carries one or `list_drafts` lists the item.
  const listed = useDraftMarks(workspaceId)(itemId);
  const mark = listed ?? ((item.data?.draft ?? null) !== null ? 'draft' : null);
  const conversion = conversionWord(summary?.extraction);
  const warnings = useAttentionMarks(workspaceId, head)(itemId);
  const attention = cited === undefined || cited === head ? attentionMark(warnings) : null;
  const said = [conversion, attention === null ? null : `${attention}, advisory`].filter(
    (word) => word !== null,
  );
  return (
    <span className="tab-label">
      {name}
      {mark === null ? null : <span className="tab-mark"> ({mark})</span>}
      {/* Always there, so the tab's description always names an element; hidden from the name. */}
      <span id={stateId} aria-hidden="true" className="tab-mark">
        {said.map((word) => ` (${word})`).join('')}
      </span>
    </span>
  );
}
