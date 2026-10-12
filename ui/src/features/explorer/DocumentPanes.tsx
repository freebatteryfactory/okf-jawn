/**
 * The document area: tabs of open items that split into side-by-side resizable panes.
 *
 * Base UI Tabs give each pane a tab list with arrow-key movement and Enter/Space activation;
 * Delete closes the focused tab. The close and split buttons sit beside the tab list, not in it,
 * so the tab list holds only tabs. react-resizable-panels separators resize panes with the
 * keyboard as well as the pointer. The focused pane is the one the URL describes.
 */
import { Tabs } from '@base-ui/react/tabs';
import { useQuery } from '@tanstack/react-query';
import { Fragment } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import type { Revision } from '../../api/generated/types.gen';
import { DocumentView, itemOptions } from '../documents/DocumentView';
import type { ItemSelection } from './search-params';
import { MAX_PANES, type Pane, type PanelLayout, useExplorerStore } from './store';

export interface DocumentPanesProps {
  workspaceId: string;
  head: Revision;
  panes: readonly Pane[];
  focusedPaneId: string;
  /** The item the URL names: what the focused pane shows, whatever tab it last had open. */
  urlItemId: string | null;
  /** The URL names an item id the contract rejects; nothing was requested for it. */
  invalidItemLink: boolean;
  /** The URL's selection and cited revision, which apply to the focused pane's open item. */
  selection: ItemSelection | undefined;
  revision: Revision | undefined;
  onActivate: (paneId: string, itemId: string) => void;
  onFocusPane: (paneId: string) => void;
  onClose: (paneId: string, itemId: string) => void;
  onSplit: (paneId: string, itemId: string) => void;
  onSelect: (itemId: string, selection: ItemSelection | undefined) => void;
  onReadLatest: (itemId: string) => void;
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
}: DocumentPanesProps & { pane: Pane; index: number }) {
  const focused = pane.id === focusedPaneId;
  const label = panes.length > 1 ? `Documents, pane ${index + 1} of ${panes.length}` : 'Documents';
  const active = focused ? urlItemId : pane.active;
  return (
    <section
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
        <p className="empty-pane">Open an item from the folders or the search results.</p>
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
                  className="tab"
                  onKeyDown={(event) => {
                    if (event.key !== 'Delete') return;
                    event.preventDefault();
                    onClose(pane.id, itemId);
                  }}
                >
                  <TabLabel workspaceId={workspaceId} itemId={itemId} head={head} />
                </Tabs.Tab>
              ))}
            </Tabs.List>
            <div className="pane-actions">
              <button
                type="button"
                disabled={active === null || panes.length >= MAX_PANES}
                onClick={() => {
                  if (active !== null) onSplit(pane.id, active);
                }}
              >
                Split right
              </button>
              <button
                type="button"
                disabled={active === null}
                onClick={() => {
                  if (active !== null) onClose(pane.id, active);
                }}
              >
                Close tab
              </button>
            </div>
          </div>
          {active === null ? (
            <p className="empty-pane">Choose one of the open items, or open another.</p>
          ) : (
            <Tabs.Panel value={active} className="tab-panel">
              <DocumentView
                key={active}
                workspaceId={workspaceId}
                itemId={active}
                head={head}
                revision={focused && revision !== undefined ? revision : head}
                selection={focused ? selection : undefined}
                onSelect={(next) => onSelect(active, next)}
                onReadLatest={() => onReadLatest(active)}
              />
            </Tabs.Panel>
          )}
        </Tabs.Root>
      )}
    </section>
  );
}

/** A tab's name is the item's title from the cache; until it is known, a short identity. */
function TabLabel({
  workspaceId,
  itemId,
  head,
}: {
  workspaceId: string;
  itemId: string;
  head: Revision;
}) {
  const item = useQuery(itemOptions(workspaceId, itemId, head));
  const summary = item.data?.summary;
  const name = summary ? summary.title || summary.path : `Item ${itemId.slice(0, 8)}`;
  return <span className="tab-label">{name}</span>;
}
