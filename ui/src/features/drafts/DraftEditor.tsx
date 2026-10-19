/**
 * The editor of one item's draft: its Markdown text, how its save stands, and discarding it.
 *
 * What is shown here is the person's own draft and is marked as one; it is not the committed
 * text, which the reader shows. The save state is always on screen and is a polite live region
 * (unsaved, saving, saved as a draft with the time, not saved, and that input of the properties
 * form is held back and in no save); the text box is described by it. A
 * failed save keeps the text in the editor and offers its retry through the failure notice, which
 * stays while the save is tried again. The next change's own save can cure it too; keyboard focus
 * that was on the retry button then moves to the text box (`useRetry`). A save that has been
 * under way for 30 seconds says "Still sending…" and offers "Stop", which aborts it: what the
 * service did with it is then unknown, so the text is "not saved" and its notice offers the
 * retry. "Stop" leaves when it is pressed or the save answers; focus on it goes to the text box.
 * Leaving the editor
 * (another tab, another item, reading the committed text, closing the tab) saves at once and keeps
 * the text in its session. After "Stop" it only keeps the text: nothing is sent until the person
 * types again or presses a control that saves.
 *
 * Discarding asks first, in place (`DiscardDraft`). After a discard the pane reads the committed
 * text again and the caller says where focus goes.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useLayoutEffect, useRef } from 'react';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import type { FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { DiscardDraft } from './DiscardDraft';
import { MarkdownEditor } from './MarkdownEditor';
import {
  editSession,
  flushSession,
  headerBehind,
  leaveSession,
  MAX_BODY_BYTES,
  stopSaving,
} from './saving';
import { type DraftSession, saveStateOf } from './sessions';

export interface DraftEditorProps {
  session: DraftSession;
  /** The item's title, for the names of the editor's controls. */
  title: string;
  /** The draft is gone: the pane reads the committed text again. */
  onDiscarded: () => void;
  /**
   * The item is a Source: its text changes only through corrections, so the draft changes its
   * properties alone and no text box is drawn.
   */
  textFixed?: boolean;
}

const MEBIBYTE = 1024 * 1024;

/**
 * Said of a save the service refused at the application's header: the page shows the item as it
 * was before someone else archived, unarchived, moved or renamed it. Not "the request was not
 * valid": nothing the person entered is wrong, and Refresh is the way on.
 */
const ITEM_CHANGED_ELSEWHERE: FailureMessage = {
  title: 'This item was changed in the workspace since this page was refreshed.',
  advice:
    'Someone archived, unarchived, moved or renamed it, and a draft is saved only on the item as it is now. Nothing was saved; your text is kept here. Press “Refresh” at the top of the page, then “Try again” here: the save then goes on.',
  action: { kind: 'retry', label: 'Try again' },
};

function megabytes(bytes: number): string {
  return (bytes / MEBIBYTE).toFixed(1);
}

function SaveState({ session }: { session: DraftSession }) {
  const state = saveStateOf(session);
  if (state === 'saving') return session.stillSending ? <>Still sending…</> : <>Saving…</>;
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

export function DraftEditor({ session, title, onDiscarded, textFixed = false }: DraftEditorProps) {
  const client = useQueryClient();
  const { workspaceId, itemId } = session;
  const stateId = useId();
  const sectionRef = useRef<HTMLElement | null>(null);
  const textBox = () => sectionRef.current?.querySelector<HTMLElement>('[role="textbox"]');

  // Leaving the editor saves at once, unless the person stopped the last save and has not acted
  // since; the text stays in its session whatever the outcome.
  useEffect(
    () => () => void leaveSession(client, workspaceId, itemId),
    [client, workspaceId, itemId],
  );

  const saveFailure = useRetry({
    request: `${workspaceId}/${itemId}`,
    failed: session.failure !== null,
    error: session.failure,
    run: () => flushSession(client, workspaceId, itemId),
    loaded: textBox,
  });

  // "Stop" is there only while a long save is in flight. It leaves when it is pressed and when
  // that save answers by itself; keyboard focus that was on it goes to the text box.
  const heldStop = useRef<HTMLElement | null>(null);
  const [, stopRef] = useWatchedPart<HTMLButtonElement>(heldStop);
  useLayoutEffect(() => {
    if (takeHeld(heldStop)) textBox()?.focus();
  });
  // The text box leaves when the item is read as a Source with the draft's text unchanged: the
  // draft goes on with its properties alone. Focus that was in it goes to the draft's first
  // control, or to the page's heading.
  const heldText = useRef<HTMLElement | null>(null);
  const [, textRef] = useWatchedPart<HTMLDivElement>(heldText);
  useLayoutEffect(() => {
    if (!takeHeld(heldText)) return;
    (sectionRef.current?.querySelector<HTMLElement>('button') ?? pageHeading())?.focus();
  });

  return (
    <section ref={sectionRef} className="draft-editor" aria-label={`Draft of ${title}`}>
      <p role="note" className="notice">
        <strong>Draft.</strong>{' '}
        {textFixed
          ? 'These are your own properties, not the committed ones.'
          : 'This is your own text, not the committed text.'}{' '}
        It becomes part of the workspace when you take a Snapshot. It started from revision{' '}
        <code title={session.baseRevision}>{short(session.baseRevision)}</code>.
      </p>
      {textFixed ? (
        <p className="meta">
          A Source’s text changes only through corrections, so this draft changes its properties
          alone; its text stays as committed.
        </p>
      ) : (
        <div ref={textRef}>
          <MarkdownEditor
            label={`Text of ${title}`}
            initialText={session.body}
            describedBy={stateId}
            onChange={(body) => editSession(client, workspaceId, itemId, { body })}
          />
        </div>
      )}
      {/* Present from the start, so each change of state is announced. */}
      <p id={stateId} role="status" className="meta save-state">
        <SaveState session={session} />
      </p>
      {session.saving && session.stillSending && (
        <p className="draft-actions">
          <button type="button" ref={stopRef} onClick={() => stopSaving(workspaceId, itemId)}>
            Stop
          </button>{' '}
          <span className="meta">
            Stopping does not undo what was already sent: the text stays here, not saved, until it
            is saved again.
          </span>
        </p>
      )}
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
          {...(headerBehind(saveFailure.error) ? { said: ITEM_CHANGED_ELSEWHERE } : {})}
        />
      )}
      <DiscardDraft session={session} onDiscarded={onDiscarded} />
    </section>
  );
}
