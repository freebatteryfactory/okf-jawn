/**
 * The one way the workspace shows a failed operation: what happened, in plain words, the
 * server's own explanation, and the single action the failure allows.
 *
 * The way out of a `not_found` is a router navigation, never a page load, so the open tabs and
 * panes survive it: inside a workspace it goes to that workspace, otherwise to the workspace list.
 */
import { Link } from '@tanstack/react-router';
import { classifyFailure, messageFor } from './failure';

export interface FailureNoticeProps {
  /** What the person was doing, e.g. "Reading this item". */
  context: string;
  /** Whatever the operation threw. */
  error: unknown;
  /** Repeat the failed operation; offered only when the failure allows a retry. */
  onRetry?: () => void;
  /** The workspace the failed operation belonged to, when the workspace itself is still there. */
  workspaceId?: string;
}

export function FailureNotice({ context, error, onRetry, workspaceId }: FailureNoticeProps) {
  const failure = classifyFailure(error);
  const message = messageFor(failure);
  const api = failure.kind === 'api' ? failure.error : null;
  return (
    <div role="alert" className="failure-notice">
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
      {message.action.kind === 'retry' && onRetry ? (
        <button type="button" onClick={onRetry}>
          {message.action.label}
        </button>
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
    </div>
  );
}
