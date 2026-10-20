/**
 * What a commit answered beside its revision, said where the change itself is said: the warnings
 * of the check that ran on it, each as a note (a Snapshot's are said the same way), and its
 * receipt, the service's durable record of the operation, by its identity, with a way to read it.
 *
 * The receipt is read only when the person asks (`get_receipt`); what it says is shown as the
 * service recorded it: the operation, who it acted for, when, through which route, and for whom
 * its content was. Its failure is a notice with its retry (`useRetry`), and an unbuilt operation
 * reads "not available in this build" there. A repeat of a request whose first answer was stored
 * before its receipt was recorded names no receipt (`CreatedItem.receipt_id` absent): that is said,
 * not hidden.
 *
 * The button keeps keyboard focus while the receipt opens and closes; it says which it does
 * (`aria-expanded`).
 */
import { useQuery } from '@tanstack/react-query';
import { useId, useRef, useState } from 'react';
import { getReceiptOptions } from '../../api/generated/@tanstack/react-query.gen';
import type {
  AccessRoute,
  ReceiptAudience,
  ReceiptId,
  Warning,
} from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';

/** How a request reached the service, in words. */
export const ROUTE_WORDS: Partial<Record<AccessRoute, string>> = {
  browser_session: 'a signed-in browser',
  mcp_delegation: 'an agent connected through MCP',
  local_owner: 'the local owner of this installation',
  service: 'a service identity',
};

/** For whom the content of an operation was, in words. */
const AUDIENCE_WORDS: Partial<Record<ReceiptAudience, string>> = {
  human_display: 'shown to a person',
  agent_context: 'given to an agent',
};

export interface CommittedProps {
  workspaceId: string;
  warnings: readonly Warning[];
  /** The receipt the answer names; null when it names none. */
  receiptId: ReceiptId | null;
}

export function Committed({ workspaceId, warnings, receiptId }: CommittedProps) {
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const receipt = useQuery({
    ...getReceiptOptions({ body: { workspace_id: workspaceId, receipt_id: receiptId ?? '' } }),
    enabled: open && receiptId !== null,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const failure = useRetry({
    request: `${workspaceId}/${receiptId ?? ''}`,
    failed: open && receipt.isError,
    error: receipt.error,
    run: () => receipt.refetch(),
    loaded: () => buttonRef.current,
  });
  return (
    <div className="committed">
      {keyed(warnings, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}
      {receiptId === null ? (
        <p className="meta">
          This answer repeats the first one given for the same request, which was stored before
          receipts were recorded: it names no receipt.
        </p>
      ) : (
        <p className="meta">
          The service recorded this change as receipt <code title={receiptId}>{receiptId}</code>.{' '}
          <button
            type="button"
            ref={buttonRef}
            aria-expanded={open}
            aria-controls={detailsId}
            onClick={() => setOpen((now) => !now)}
          >
            {open ? 'Hide the receipt' : 'Show the receipt'}
          </button>
        </p>
      )}
      <div id={detailsId}>
        {open && receiptId !== null && (
          <>
            {receipt.isPending && !failure.retrying && <p role="status">Reading the receipt…</p>}
            {failure.shown && (
              <FailureNotice
                context="Reading the receipt"
                error={failure.error}
                retry={failure}
                workspaceId={workspaceId}
              />
            )}
            {receipt.data !== undefined && (
              <dl className="receipt">
                <dt>Operation</dt>
                <dd>
                  <code>{receipt.data.operation_id}</code>
                </dd>
                <dt>Acted for</dt>
                <dd>{receipt.data.principal_subject}</dd>
                <dt>When</dt>
                <dd>
                  <Instant value={receipt.data.returned_at} />
                </dd>
                <dt>Through</dt>
                <dd>
                  {ROUTE_WORDS[receipt.data.route] ??
                    `${receipt.data.route} (a route this page does not know)`}
                </dd>
                <dt>Its content was</dt>
                <dd>
                  {AUDIENCE_WORDS[receipt.data.audience] ??
                    `${receipt.data.audience} (not known to this page)`}
                </dd>
              </dl>
            )}
          </>
        )}
      </div>
    </div>
  );
}
