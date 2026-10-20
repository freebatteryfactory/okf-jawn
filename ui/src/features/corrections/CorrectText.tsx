/**
 * Correct text (`correct_digest`): a text box beside the text shown, started from it, saved on
 * the head the workspace shows with the item's digest there (`correction-requests.ts`).
 *
 * "Correct text" opens the editor in place and puts keyboard focus in the person's text box; the
 * button stays (`aria-expanded`), and pressing it again puts focus back in the text box. The text
 * box is described by what it started from: whose text that is (converter's, a correction,
 * supplied). Below it the page says, in words, where the person's text differs from the text
 * shown (lines of their text); "Save the correction" is described by that, and sends nothing while
 * nothing is changed. "Cancel" closes the editor without sending, and the person's text with it.
 *
 * One request per press, repeated under its key while it says the same thing on the same base
 * (`attemptFor`: the notice's "Try again" after a failure that may have been carried out), a new
 * key once the service has answered it, or for another text or base. What it answered becomes the
 * head the workspace shows (when it still shows the base), and the tabs that cited the old head
 * follow (`onChanged`): the reader then shows the corrected text, labelled as a correction. The
 * editor closes, and keyboard focus goes to the words that say what was done, with the commit's
 * warnings and receipt (`tree/Committed.tsx`).
 *
 * Refusals are said in words with the service's own (`FailureNotice`). A base that moved
 * (`conflict` with `stale_base`) is said as such: the person's text is kept, and the notice offers
 * to show the version the service compared with, which the reader then reads (it was read ahead,
 * so this section and its text box stay); the text beside the person's is then that version's,
 * and a save asks again on it, under a new key. A failure is shown only for the head it was asked
 * on. `not_implemented` reads "Not available in this build."
 *
 * Who may: `write`, at the version the workspace shows. In an archived workspace, or on a tab that
 * reads another revision, the button stays, `aria-disabled`, described by why (`blockedBy`), and
 * opens nothing.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import type { Extraction, Revision } from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, type FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { Committed } from '../tree/Committed';
import { type Attempt, attemptFor } from '../tree/changes';
import { useWorkspaceChanges } from '../tree/context';
import { CorrectionEditor } from './CorrectionEditor';
import {
  type Corrected,
  type Correction,
  changeWords,
  correct,
  type LineSpan,
  MAX_CORRECTION_BYTES,
  short,
  tooLong,
  whoseText,
} from './correction-requests';

export interface CorrectTextProps {
  workspaceId: string;
  itemId: string;
  /** The head the workspace shows: the correction's base. */
  head: Revision;
  /** The item's text at that head, as `get_item` answers it. */
  body: string;
  extraction: Extraction;
  /** The person may write here, at this version, now. */
  ready: boolean;
  /** Why the action cannot run here (archived, another revision), when it cannot: an element id. */
  blockedBy: string | null;
}

/** The words of a base that moved: the person's text is kept, and the version is shown on asking. */
function staleWords(error: unknown): FailureMessage | undefined {
  const failure = classifyFailure(error);
  if (failure.kind !== 'api' || failure.error.detail?.kind !== 'stale_base') return undefined;
  const head = failure.error.detail.head;
  return {
    title: 'The workspace changed since the version your correction was made on.',
    advice: `It now holds revision ${short(head)}. Your text is kept here. Show that version, compare it with your text, then save again.`,
    action: { kind: 'show-head', label: `Show revision ${short(head)} and keep my text`, head },
  };
}

export function CorrectText({
  workspaceId,
  itemId,
  head,
  body,
  extraction,
  ready,
  blockedBy,
}: CorrectTextProps) {
  const client = useQueryClient();
  const { onChanged } = useWorkspaceChanges();
  const panelId = useId();
  const headingId = useId();
  const startedId = useId();
  const changesId = useId();
  const openRef = useRef<HTMLButtonElement | null>(null);
  const saveRef = useRef<HTMLButtonElement | null>(null);
  const doneRef = useRef<HTMLDivElement | null>(null);
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(body);
  const [places, setPlaces] = useState<LineSpan[]>([]);
  const [overLimit, setOverLimit] = useState(false);
  const [done, setDone] = useState<Corrected | null>(null);
  const last = useRef<Attempt<Correction> | null>(null);
  const digest = extraction.digest ?? null;

  const send = useMutation({
    mutationFn: (attempt: Attempt<Correction>) => correct(client, workspaceId, itemId, attempt),
    onSuccess: (corrected) => {
      // The service answered: whatever is asked next is another request.
      last.current = null;
      setDone(corrected);
      setEditing(false);
      if (corrected.adopted) onChanged(corrected);
    },
  });
  const save = () => {
    if (digest === null) return Promise.resolve();
    if (tooLong(text)) {
      setOverLimit(true);
      return Promise.resolve();
    }
    setOverLimit(false);
    const attempt = attemptFor(last.current, head, { digest, text, adopts: null });
    last.current = attempt;
    return send.mutateAsync(attempt);
  };
  // A failure is about the head it was asked on; once another is shown, it is not this one's.
  const failure = useRetry({
    request: head,
    failed: send.isError && send.variables?.base === head,
    error: send.error,
    run: save,
    loaded: () => saveRef.current ?? doneRef.current,
  });

  // The editor leaves when the correction is saved or cancelled: focus that was in it goes to
  // the words saying what was done, else to "Correct text".
  const heldPanel = useRef<HTMLElement | null>(null);
  const [, panelRef] = useWatchedPart<HTMLFieldSetElement>(heldPanel);
  useLayoutEffect(() => {
    if (takeHeld(heldPanel)) (doneRef.current ?? openRef.current ?? pageHeading())?.focus();
  });

  if (digest === null) {
    return (
      <p className="meta">
        Correct text needs a conversion of this file to correct, and it has none: the converter has
        not made one.
      </p>
    );
  }
  const changed = text !== body;
  const canSave = ready && changed && !send.isPending;
  const open = () => {
    if (!ready) return;
    if (editing) {
      document
        .getElementById(panelId)
        ?.querySelector<HTMLElement>('.cm-merge-b .cm-content')
        ?.focus();
      return;
    }
    send.reset();
    setDone(null);
    setText(body);
    setPlaces([]);
    setOverLimit(false);
    setEditing(true);
  };
  const whose = whoseText(extraction);
  return (
    <div className="correct-text">
      <p>
        {/* aria-disabled, never disabled: the button keeps focus. */}
        <button
          type="button"
          ref={openRef}
          aria-expanded={editing}
          aria-controls={editing ? panelId : undefined}
          aria-disabled={!ready}
          {...(blockedBy === null ? {} : { 'aria-describedby': blockedBy })}
          onClick={open}
        >
          Correct text
        </button>
      </p>
      {editing && (
        <fieldset
          key="editor"
          id={panelId}
          ref={panelRef}
          aria-labelledby={headingId}
          className="grid gap-2 rounded border border-[color:GrayText] px-3 py-2"
        >
          <h4 id={headingId} className="font-bold">
            Correct the text
          </h4>
          <p>
            Your correction is kept beside the converter’s text, which stays as the converter made
            it. It is saved on revision <code title={head}>{short(head)}</code>, the version this
            workspace shows.
          </p>
          <p id={startedId} className="meta">
            It starts from the text shown: {whose}.
          </p>
          <div className="grid grid-cols-2 gap-2" aria-hidden="true">
            <p className="m-0 font-bold">The text shown now</p>
            <p className="m-0 font-bold">Your correction</p>
          </div>
          <CorrectionEditor
            shown={body}
            shownLabel="The text shown now"
            label="Your correction"
            describedBy={startedId}
            autoFocus
            onChange={(next, at) => {
              setText(next);
              setPlaces(at);
            }}
          />
          <p id={changesId} className="meta">
            {changed ? changeWords(places) : changeWords([])}
          </p>
          {overLimit && (
            <p role="alert">
              Your text is longer than a correction may be: at most{' '}
              {MAX_CORRECTION_BYTES.toLocaleString('en')} bytes. Nothing was sent.
            </p>
          )}
          <p className="flex flex-wrap gap-2">
            <button
              type="button"
              ref={saveRef}
              aria-disabled={!canSave}
              aria-describedby={[changesId, blockedBy].filter(Boolean).join(' ')}
              onClick={() => {
                if (canSave) void save().catch(() => undefined);
              }}
            >
              {send.isPending ? 'Saving the correction…' : 'Save the correction'}
            </button>
            <button
              type="button"
              onClick={() => {
                if (!send.isPending) setEditing(false);
              }}
            >
              Cancel
            </button>
          </p>
          {/* Present from the start, so what pressing did is announced. */}
          <p role="status" className="sr-only">
            {send.isPending && !failure.retrying ? 'Saving the correction…' : ''}
          </p>
          {failure.shown && (
            <FailureNotice
              context="Saving the correction"
              error={failure.error}
              retry={failure}
              workspaceId={workspaceId}
              words={staleWords(failure.error)}
            />
          )}
        </fieldset>
      )}
      {done !== null && (
        <div key="done" ref={doneRef} tabIndex={-1}>
          <p role="status">
            {done.adopted
              ? `Saved: your correction is revision ${short(done.revision)}. The workspace now shows it, and the text above is your correction.`
              : `Saved: your correction is revision ${short(done.revision)}. This page still shows another version: refresh the workspace to see it.`}
          </p>
          <Committed
            workspaceId={workspaceId}
            warnings={done.warnings}
            receiptId={done.receiptId}
          />
        </div>
      )}
    </div>
  );
}
