/**
 * The one way the workspace shows a failed operation: what happened, in plain words, the
 * server's own explanation, and the single action the failure allows.
 *
 * The way out of a `not_found` is a router navigation, never a page load, so the open tabs and
 * panes survive it: inside a workspace it goes to that workspace, otherwise to the workspace list.
 *
 * A retry is offered only through `useRetry`, which keeps this notice on screen while the
 * operation is repeated: pressing the button never unmounts it. Meanwhile the button reads "Trying
 * again…" with `aria-disabled` (a `disabled` button would lose keyboard focus), and a polite status
 * says so. The button and that status sit beside the alert, not inside it, so a change of the
 * button's label is not announced as an alert; the words after a retry that failed are a new alert.
 */
import { Link } from '@tanstack/react-router';
import { classifyFailure, messageFor } from './failure';
import type { FailureRetry } from './retry';

export interface FailureNoticeProps {
  /** What the person was doing, e.g. "Reading this item". */
  context: string;
  /** Whatever the operation threw. */
  error: unknown;
  /** The operation's retry, from `useRetry`; without it no retry is offered. */
  retry?: FailureRetry;
  /** The workspace the failed operation belonged to, when the workspace itself is still there. */
  workspaceId?: string;
}

export function FailureNotice({ context, error, retry, workspaceId }: FailureNoticeProps) {
  const failure = classifyFailure(error);
  const message = messageFor(failure);
  const api = failure.kind === 'api' ? failure.error : null;
  const words = (
    <>
      <p>
        <strong>
          {context}: {message.title}
        </strong>
      </p>
      <p>{message.advice}</p>
      {api && api.message.length > 0 && (
        <p>
          The service said: <q>{api.message}</q>
          {api.field ? (
            <>
              {' '}
              (about <code>{api.field}</code>)
            </>
          ) : null}
        </p>
      )}
      {api?.request_id ? (
        <p>
          Reference: <code>{api.request_id}</code>
        </p>
      ) : null}
      {message.action.kind === 'home' ? (
        workspaceId === undefined ? (
          <Link to="/">{message.action.label}</Link>
        ) : (
          <Link to="/w/$workspaceId" params={{ workspaceId }}>
            Go to this workspace
          </Link>
        )
      ) : null}
    </>
  );
  // A failure that allows no retry, and is not the end of one, is the alert alone, as it always was.
  if (retry === undefined || (message.action.kind !== 'retry' && retry.settled === 0)) {
    return (
      <div role="alert" className="failure-notice">
        {words}
      </div>
    );
  }
  return (
    // Focusable from script only: where keyboard focus rests when a retry ends in a failure that
    // allows no further retry, so the button it was on is gone.
    <div ref={retry.noticeRef} tabIndex={-1} className="failure-notice">
      <div key={retry.settled} role="alert">
        {words}
      </div>
      {message.action.kind === 'retry' && (
        <button type="button" aria-disabled={retry.retrying} onClick={retry.start}>
          {retry.retrying ? 'Trying again…' : message.action.label}
        </button>
      )}
      {/* Present from the start, so the retry is announced when it begins. */}
      <p role="status" className="sr-only">
        {retry.retrying ? `${context}: trying again…` : ''}
      </p>
    </div>
  );
}
