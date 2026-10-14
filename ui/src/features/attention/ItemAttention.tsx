/**
 * The Attention warnings of one item, in the reader: listed when the workspace's list names the
 * item, each by its kind and the service's explanation, with what such warnings are (advisory,
 * false alarms expected) and the way to the whole list.
 *
 * Nothing is shown for an item no listed warning names. That absence says nothing about the item:
 * the list may hold more than is listed, and a check that finds nothing is not a finding that the
 * item is safe (the Attention page says both).
 *
 * The warnings are those of the revision the workspace shows, whatever revision the pane reads;
 * the notice says which.
 */
import { Link } from '@tanstack/react-router';
import type { Revision } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { attentionKind, useAttention, warningsFor, warningsOf } from './attention';

export interface ItemAttentionProps {
  workspaceId: string;
  itemId: string;
  /** The revision the workspace shows: where the warnings were looked for. */
  head: Revision;
}

export function ItemAttention({ workspaceId, itemId, head }: ItemAttentionProps) {
  const attention = useAttention(workspaceId, head);
  const mine = warningsFor(warningsOf(attention.data?.pages), itemId);
  if (mine.length === 0) return null;
  return (
    <div role="note" className="notice item-attention">
      <p>
        <strong>Attention:</strong>{' '}
        {mine.length === 1 ? '1 advisory warning' : `${mine.length} advisory warnings`} for this
        item at revision <code title={head}>{short(head)}</code>. False alarms are expected.
      </p>
      <ul>
        {keyed(mine, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
          <li key={key}>
            <strong>{attentionKind(warning.kind)}:</strong> {warning.message}
          </li>
        ))}
      </ul>
      <p>
        <Link to="/w/$workspaceId/attention" params={{ workspaceId }}>
          All warnings of this workspace
        </Link>
      </p>
    </div>
  );
}
