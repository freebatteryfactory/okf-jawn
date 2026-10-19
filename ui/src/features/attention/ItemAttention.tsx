/**
 * The Attention warnings of one item, in the reader: listed when the workspace's list names the
 * item, each by its kind and the service's explanation, with what such warnings are (advisory,
 * false alarms expected) and the way to the whole list.
 *
 * Nothing is shown for an item no current listed warning names. A retained previous check or a
 * failed check is labelled even when the previous answer was empty. Absence says nothing about the item:
 * the list may hold more than is listed, and a check that finds nothing is not a finding that the
 * item is safe (the Attention page says both).
 *
 * The warnings are those of the revision the workspace shows, whatever revision the pane reads;
 * the notice says which.
 */
import { Link } from '@tanstack/react-router';
import { useRef } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { attentionKind, useAttention, warningsFor, warningsOf } from './attention';
import { InstructionFindings } from './InstructionFindings';
import { PreviousCheck } from './PreviousCheck';

export interface ItemAttentionProps {
  workspaceId: string;
  itemId: string;
  /** The revision the workspace shows: where the warnings were looked for. */
  head: Revision;
}

export function ItemAttention({ workspaceId, itemId, head }: ItemAttentionProps) {
  const attention = useAttention(workspaceId, head);
  const warnings = warningsOf(attention.data?.pages);
  const mine = warningsFor(warnings, itemId);
  const retained = attention.data !== undefined;
  const previous = retained && (attention.isFetching || attention.isError || attention.isStale);
  const notice = useRef<HTMLDivElement | null>(null);
  const failure = useRetry({
    request: `${workspaceId} ${head} ${itemId}`,
    failed: attention.isError,
    error: attention.error,
    run: () => attention.refetch(),
    loaded: () => notice.current,
  });
  if (mine.length === 0 && !previous && !failure.shown) return null;
  return (
    <div role="note" className="notice item-attention" ref={notice} tabIndex={-1}>
      <p>
        <strong>Attention:</strong>{' '}
        {!retained ? (
          'Warnings could not be checked'
        ) : (
          <>
            {previous ? 'Previous check: ' : ''}
            {mine.length === 1 ? '1 advisory warning' : `${mine.length} advisory warnings`}
          </>
        )}{' '}
        for this item at revision <code title={head}>{short(head)}</code>. False alarms are
        expected.
      </p>
      {previous && <PreviousCheck failed={attention.isError} empty={warnings.length === 0} />}
      {previous && warnings.length > 0 && mine.length === 0 && (
        <p>The previous check listed no warnings for this item.</p>
      )}
      <ul>
        {keyed(mine, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
          <li key={key}>
            <strong>{attentionKind(warning.kind)}:</strong> {warning.message}
            {warning.instruction_like && (
              <InstructionFindings evidence={warning.instruction_like} />
            )}
          </li>
        ))}
      </ul>
      {failure.shown && (
        <FailureNotice
          context="Checking this item's warnings"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      <p>
        <Link to="/w/$workspaceId/attention" params={{ workspaceId }}>
          All warnings of this workspace
        </Link>
      </p>
    </div>
  );
}
