/**
 * The person's draft of an item they deleted: its text and properties, read-only, and the way to
 * discard it.
 *
 * A deleted item cannot be edited or snapshotted, so its draft is not in an editor. It is in a
 * read-only text box instead: keyboard focus can enter it, its text can be selected and copied,
 * and nothing typed changes it. What is shown is the text this page holds when the item was being
 * edited here (the last words typed, saved or not), else the draft the service holds.
 *
 * Discarding asks first, in place, like the editor's own discard: "Keep the draft" takes keyboard
 * focus, and choosing it gives focus back to "Discard draft". After the discard this part leaves
 * the page; the caller watches it and places focus.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import { discardSession } from '../drafts/saving';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { Properties } from './Properties';

export interface LeftDraftProps {
  workspaceId: string;
  itemId: string;
  /** The title the item had. */
  title: string;
  body: string;
  properties: Record<string, unknown>;
  /** Some of this text is held by this page only: the service has no draft of it, or an older one. */
  onPageOnly: boolean;
}

export function LeftDraft({
  workspaceId,
  itemId,
  title,
  body,
  properties,
  onPageOnly,
}: LeftDraftProps) {
  const client = useQueryClient();
  const textId = useId();
  const wordsId = useId();
  const [confirming, setConfirming] = useState(false);
  const discardRef = useRef<HTMLButtonElement | null>(null);
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const focusNext = useRef<'keep' | 'discard' | null>(null);
  useLayoutEffect(() => {
    const next = focusNext.current;
    focusNext.current = null;
    if (next === 'keep') keepRef.current?.focus();
    else if (next === 'discard') discardRef.current?.focus();
  });
  const discard = useMutation({
    mutationFn: () => discardSession(client, workspaceId, itemId),
  });
  const failure = useRetry({
    request: `${workspaceId}/${itemId}`,
    failed: discard.isError,
    error: discard.error,
    run: () => discard.mutateAsync(),
    loaded: () => discardRef.current,
  });

  return (
    <section className="draft-editor" aria-label={`Your draft of ${title}`}>
      <p id={wordsId} role="note" className="notice">
        <strong>Your draft of the deleted item.</strong>{' '}
        {onPageOnly
          ? 'This is the text you had typed. Not all of it is saved as a draft, and it can no longer be saved: it is kept on this page only, until you discard it or leave the page.'
          : 'This is the draft the service holds for you.'}{' '}
        It cannot become part of the workspace through a Snapshot. It is read-only: select and copy
        what you want to keep, then discard it.
      </p>
      <label htmlFor={textId}>Text of your draft of “{title}” (read-only)</label>
      <textarea
        id={textId}
        className="left-draft"
        readOnly
        rows={12}
        value={body}
        aria-describedby={wordsId}
      />
      <Properties properties={properties} />
      {confirming ? (
        <fieldset className="confirm">
          <legend>Discard this draft?</legend>
          <p>Your text here will be removed. Nothing else changes: the item stays deleted.</p>
          <button
            type="button"
            ref={keepRef}
            onClick={() => {
              focusNext.current = 'discard';
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
            onClick={() => {
              focusNext.current = 'keep';
              setConfirming(true);
            }}
          >
            Discard draft
          </button>
        </p>
      )}
      {failure.shown && (
        <FailureNotice
          context="Discarding this draft"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      <p role="status" className="sr-only">
        {discard.isPending && !failure.retrying ? 'Discarding this draft…' : ''}
      </p>
    </section>
  );
}
