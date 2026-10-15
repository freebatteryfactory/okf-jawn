/**
 * The one way the workspace shows a failed operation: what happened, in plain words, the
 * server's own explanation, and the single action the failure allows.
 *
 * The way out of a `not_found` is a router navigation, never a page load, so the open tabs and
 * panes survive it: inside a workspace it goes to that workspace, otherwise to the workspace list.
 *
 * A retry is offered only through `useRetry`, and only where the caller gave the hook a way to
 * repeat the operation. The hook keeps this notice on screen while the operation is repeated:
 * pressing the button does not unmount it. Meanwhile the button reads "Trying again…" with
 * `aria-disabled` (a `disabled` button would lose keyboard focus), and a polite status says so.
 * The button and that status sit beside the alert, not inside it, so a change of the button's
 * label is not announced as an alert; the words after a retry that failed are a new alert.
 *
 * Given the hook's result, the notice is followed by the hook whatever its failure allows: its
 * root carries the hook's ref and takes focus from script (`tabindex="-1"`), and what holds its
 * way out carries a ref of the hook too. So keyboard focus on the retry button or on the way out
 * is seen when the notice, the button or the link leaves, and the notice itself can take focus
 * when the failure it then shows has neither. A failure that offers no retry here is shown with
 * no retry button and no status. Without the hook's result the notice is the alert alone, and
 * nothing follows focus out of it.
 *
 * A failure whose detail names the head the request was compared with (`stale_base`) offers to show
 * that version (`show-head`): the workspace then shows it, and the head checks run again. That needs
 * the workspace (`workspaceId`); without it, the words say what happened and nothing is offered.
 *
 * A caller may give the notice words of its own for a failure (`words`), with the action they
 * allow; the general words of `failure.ts` are used wherever it gives none. `said` is the same
 * thing under the name another package gave it, and is asked first; no caller passes both.
 */
import { useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { adoptHead, checkHeadsAgain } from '../explorer/head';
import { classifyFailure, type FailureMessage, messageFor } from './failure';
import type { FailureRetry } from './retry';

export interface FailureNoticeProps {
  /** What the person was doing, e.g. "Reading this item". */
  context: string;
  /** Whatever the operation threw. */
  error: unknown;
  /**
   * The operation's failure as `useRetry` holds it. Without it no retry is offered, and keyboard
   * focus in the notice is not followed when the notice leaves.
   */
  retry?: FailureRetry;
  /** The workspace the failed operation belonged to, when the workspace itself is still there. */
  workspaceId?: string;
  /**
   * What to say of this failure in place of the words for its code, when the caller knows what
   * the failure means where it happened. The service's own words are still shown under it.
   */
  said?: FailureMessage;
  /**
   * Words of this place for this failure, where the general ones would mislead: `too_large` for an
   * upload is not "too large to show", and a failure the page found by itself is not the service
   * being out of reach. The service's own message and reference are still shown.
   */
  words?: FailureMessage | undefined;
}

export function FailureNotice({
  context,
  error,
  retry,
  workspaceId,
  said,
  words: own,
}: FailureNoticeProps) {
  const failure = classifyFailure(error);
  const message = said ?? own ?? messageFor(failure);
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
        // Watched by the hook: focus on the link is followed when the link goes.
        <span ref={retry?.wayOutRef}>
          {workspaceId === undefined ? (
            <Link to="/">{message.action.label}</Link>
          ) : (
            <Link to="/w/$workspaceId" params={{ workspaceId }}>
              Go to this workspace
            </Link>
          )}
        </span>
      ) : null}
      {message.action.kind === 'show-head' && workspaceId !== undefined ? (
        <ShowHead
          workspaceId={workspaceId}
          head={message.action.head}
          label={message.action.label}
        />
      ) : null}
    </>
  );
  if (retry === undefined) {
    return (
      <div role="alert" className="failure-notice">
        {words}
      </div>
    );
  }
  // No retry of its own where the caller gave the hook no way to repeat the operation.
  const retryLabel =
    retry.start !== undefined && message.action.kind === 'retry' ? message.action.label : null;
  return (
    // The hook's ref on the root whatever the failure allows, and focusable from script only:
    // where keyboard focus rests when the failure now shown allows no retry (a retry ended in it,
    // or it replaced the one shown), so the button it was on is gone.
    <div ref={retry.noticeRef} tabIndex={-1} className="failure-notice">
      <div key={retry.settled} role="alert">
        {words}
      </div>
      {retryLabel !== null && (
        <>
          <button
            type="button"
            ref={retry.buttonRef}
            aria-disabled={retry.retrying}
            onClick={retry.start}
          >
            {retry.retrying ? 'Trying again…' : retryLabel}
          </button>
          {/* Present with the button, before a retry starts, so the retry is announced when it does. */}
          <p role="status" className="sr-only">
            {retry.retrying ? `${context}: trying again…` : ''}
          </p>
        </>
      )}
    </div>
  );
}

/**
 * Show the workspace at `head`, the version a refusal named. The button stays, and a polite line
 * says what it did; nothing is sent again by itself.
 */
function ShowHead({
  workspaceId,
  head,
  label,
}: {
  workspaceId: string;
  head: Revision;
  label: string;
}) {
  const client = useQueryClient();
  const [shown, setShown] = useState(false);
  return (
    <p>
      <button
        type="button"
        onClick={() => {
          adoptHead(client, workspaceId, head);
          checkHeadsAgain(client, workspaceId);
          setShown(true);
        }}
      >
        {label}
      </button>{' '}
      <span role="status">
        {shown
          ? `The workspace now shows revision ${head.slice(0, 10)}. Nothing was sent again.`
          : ''}
      </span>
    </p>
  );
}
