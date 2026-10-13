/**
 * "Discard draft" for the session of one item, wherever its text is shown: under the editor, and
 * under the kept text of a session that is refused.
 *
 * Discarding asks first, in place: "Keep the draft" takes keyboard focus, and choosing it gives
 * focus back to "Discard draft". The question says what goes: the text, and with it what was
 * changed in the properties and is held back, when there is any. The discard is the item's own
 * (`discardSession` in `saving.ts`); a failure of it is shown here with its retry, and the session
 * stays. With nothing to discard the button is there and does nothing (`aria-disabled`). After a
 * discard the caller says what the pane shows and where focus goes.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { discardSession } from './saving';
import { type DraftSession, saveStateOf } from './sessions';

export interface DiscardDraftProps {
  session: DraftSession;
  /** The draft is gone: for a caller that has something of its own to do then. */
  onDiscarded?: () => void;
}

export function DiscardDraft({ session, onDiscarded }: DiscardDraftProps) {
  const client = useQueryClient();
  const { workspaceId, itemId } = session;
  const [confirming, setConfirming] = useState(false);
  const discardRef = useRef<HTMLButtonElement | null>(null);
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const focusAfterConfirm = useRef<'keep' | 'discard' | null>(null);
  useEffect(() => {
    const target = focusAfterConfirm.current;
    focusAfterConfirm.current = null;
    if (target === 'keep') keepRef.current?.focus();
    else if (target === 'discard') discardRef.current?.focus();
  });
  const discard = useMutation({
    mutationFn: () => discardSession(client, workspaceId, itemId),
    onSuccess: () => onDiscarded?.(),
  });
  const discardFailure = useRetry({
    request: `${workspaceId}/${itemId}`,
    failed: discard.isError,
    error: discard.error,
    run: () => discard.mutateAsync(),
    loaded: () => null,
  });
  const nothingToDiscard = saveStateOf(session) === 'unchanged';

  return (
    <>
      {confirming ? (
        <fieldset className="confirm">
          <legend>Discard this draft?</legend>
          <p>
            {session.heldBack === null
              ? 'Your text here will be removed.'
              : 'Your text here, and what you changed in the properties that is not saved, will be removed.'}{' '}
            The committed text stays as it is.
          </p>
          <button
            type="button"
            ref={keepRef}
            onClick={() => {
              focusAfterConfirm.current = 'discard';
              setConfirming(false);
            }}
          >
            Keep the draft
          </button>{' '}
          {/* aria-disabled rather than disabled: the button keeps focus while it works. */}
          <button
            type="button"
            aria-disabled={discard.isPending}
            onClick={() => {
              if (!discard.isPending) discard.mutate();
            }}
          >
            {discard.isPending ? 'Discarding…' : 'Discard the draft'}
          </button>
        </fieldset>
      ) : (
        <p className="draft-actions">
          <button
            type="button"
            ref={discardRef}
            aria-disabled={nothingToDiscard}
            onClick={() => {
              if (nothingToDiscard) return;
              focusAfterConfirm.current = 'keep';
              setConfirming(true);
            }}
          >
            Discard draft
          </button>
        </p>
      )}
      {discardFailure.shown && (
        <FailureNotice
          context="Discarding this draft"
          error={discardFailure.error}
          retry={discardFailure}
          workspaceId={workspaceId}
        />
      )}
      <p role="status" className="sr-only">
        {discard.isPending && !discardFailure.retrying ? 'Discarding this draft…' : ''}
      </p>
    </>
  );
}
