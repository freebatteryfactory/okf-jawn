/**
 * An item's own history, on its page: its Timeline (`TimelineView` for this item: each version's
 * Changes, a way to read it in the tab, Rewind of this item alone) and Who changed each line
 * (`Who.tsx`), at the revision the tab reads.
 *
 * Each is shown on demand, by a button that keeps keyboard focus and says which it does
 * (`aria-expanded`); nothing is asked of the service until it is shown.
 */
import { useId, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { lineCount } from './history';
import { TimelineView } from './TimelineView';
import { Who } from './Who';

export interface ItemHistoryProps {
  workspaceId: string;
  itemId: string;
  title: string;
  /** The revision the tab reads. */
  revision: Revision;
  /** The item's text at that revision, without its properties header. */
  body: string;
}

export function ItemHistory({ workspaceId, itemId, title, revision, body }: ItemHistoryProps) {
  const timelineId = useId();
  const whoId = useId();
  const [timeline, setTimeline] = useState(false);
  const [who, setWho] = useState(false);
  return (
    <div className="item-history">
      <p className="document-actions">
        <button
          type="button"
          aria-expanded={timeline}
          aria-controls={timelineId}
          onClick={() => setTimeline((now) => !now)}
        >
          {timeline ? 'Hide this item’s Timeline' : 'Show this item’s Timeline'}
        </button>{' '}
        <button
          type="button"
          aria-expanded={who}
          aria-controls={whoId}
          onClick={() => setWho((now) => !now)}
        >
          {who ? 'Hide who changed each line' : 'Show who changed each line'}
        </button>
      </p>
      <div id={timelineId}>
        {timeline && (
          <TimelineView
            workspaceId={workspaceId}
            revision={revision}
            item={{ id: itemId, title }}
            title={`Timeline of ${title}`}
            level={3}
          />
        )}
      </div>
      <div id={whoId}>
        {who && (
          <Who
            workspaceId={workspaceId}
            itemId={itemId}
            revision={revision}
            lines={lineCount(body)}
          />
        )}
      </div>
    </div>
  );
}
