/**
 * The one way the workspace shows a failed operation: what happened, in plain words, the
 * server's own explanation, and the single action the failure allows.
 */
import { classifyFailure, messageFor } from './failure';

export interface FailureNoticeProps {
  /** What the person was doing, e.g. "Reading this item". */
  context: string;
  /** Whatever the operation threw. */
  error: unknown;
  /** Repeat the failed operation; offered only when the failure allows a retry. */
  onRetry?: () => void;
}

export function FailureNotice({ context, error, onRetry }: FailureNoticeProps) {
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
      {message.action.kind === 'home' ? <a href="/">{message.action.label}</a> : null}
    </div>
  );
}
