/**
 * The editor of one item's draft: its Markdown text, how its save stands, and discarding it.
 *
 * What is shown here is the person's own draft and is marked as one; it is not the committed
 * text, which the reader shows. The save state is always on screen and is a polite live region
 * (unsaved, saving, saved as a draft with the time, not saved, and that input of the properties
 * form is held back and in no save); the text box is described by it. A
 * failed save keeps the text in the editor and offers its retry through the failure notice, which
 * stays while the save is tried again. The next change's own save can cure it too; keyboard focus
 * that was on the retry button then moves to the text box (`useRetry`). Leaving the editor
 * (another tab, another item, reading the committed text, closing the tab) saves at once and keeps
 * the text in its session.
 *
 * Discarding asks first, in place: "Keep the draft" takes keyboard focus, and choosing it gives
 * focus back to "Discard draft". After a discard the pane reads the committed text again and the
 * caller says where focus goes.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { Instant } from '../../lib/instant';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { MarkdownEditor } from './MarkdownEditor';
import { discardSession, editSession, flushSession, MAX_BODY_BYTES } from './saving';
import { type DraftSession, saveStateOf } from './sessions';

export interface DraftEditorProps {
  session: DraftSession;
  /** The item's title, for the names of the editor's controls. */
  title: string;
  /** The draft is gone: the pane reads the committed text again. */
  onDiscarded: () => void;
}

const MEBIBYTE = 1024 * 1024;

function megabytes(bytes: number): string {
  return (bytes / MEBIBYTE).toFixed(1);
}

function SaveState({ session }: { session: DraftSession }) {
  const state = saveStateOf(session);
  if (state === 'saving') return <>Saving…</>;
  if (state === 'unsaved') return <>Unsaved changes.</>;
  if (state === 'failed') return <>Not saved. Your text is kept here.</>;
  if (state === 'too-long') return <>Not saved: the text is too long.</>;
  if (state === 'held-back') {
    // The text is saved or unchanged; what was entered in the properties form is in no save.
    return session.confirmed === null ? (
      <>Not saved: what you changed in the properties is held back.</>
    ) : (
      <>
        Saved as a draft at <Instant value={session.confirmed.draft.saved_at} />, without what you
        changed in the properties: that is held back.
      </>
    );
  }
  if (state === 'unchanged' || session.confirmed === null) return <>No changes to save.</>;
  return (
    <>
      Saved as a draft at <Instant value={session.confirmed.draft.saved_at} />.
    </>
  );
}

export function DraftEditor({ session, title, onDiscarded }: DraftEditorProps) {
  const client = useQueryClient();
  const { workspaceId, itemId } = session;
  const stateId = useId();
  const sectionRef = useRef<HTMLElement | null>(null);
  const textBox = () => sectionRef.current?.querySelector<HTMLElement>('[role="textbox"]');

  // Leaving the editor saves at once; the text stays in its session whatever the outcome.
  useEffect(
    () => () => void flushSession(client, workspaceId, itemId),
    [client, workspaceId, itemId],
  );

  const saveFailure = useRetry({
    request: `${workspaceId}/${itemId}`,
    failed: session.failure !== null,
    error: session.failure,
    run: () => flushSession(client, workspaceId, itemId),
    loaded: textBox,
  });

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
    onSuccess: onDiscarded,
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
    <section ref={sectionRef} className="draft-editor" aria-label={`Draft of ${title}`}>
      <p role="note" className="notice">
        <strong>Draft.</strong> This is your own text, not the committed text. It becomes part of
        the workspace when you take a Snapshot. It started from revision{' '}
        <code title={session.baseRevision}>{short(session.baseRevision)}</code>.
      </p>
      <MarkdownEditor
        label={`Text of ${title}`}
        initialText={session.body}
        describedBy={stateId}
        onChange={(body) => editSession(client, workspaceId, itemId, { body })}
      />
      {/* Present from the start, so each change of state is announced. */}
      <p id={stateId} role="status" className="meta save-state">
        <SaveState session={session} />
      </p>
      {session.tooLong !== null && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>This text is too long to save.</strong>
          </p>
          <p>
            It is {megabytes(session.tooLong)} MiB and a draft holds at most{' '}
            {megabytes(MAX_BODY_BYTES)} MiB. Nothing was sent. Shorten it and it is saved again.
          </p>
        </div>
      )}
      {saveFailure.shown && (
        <FailureNotice
          context="Saving this draft"
          error={saveFailure.error}
          retry={saveFailure}
          workspaceId={workspaceId}
        />
      )}
      {confirming ? (
        <fieldset className="confirm">
          <legend>Discard this draft?</legend>
          <p>Your text here will be removed. The committed text stays as it is.</p>
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
    </section>
  );
}
