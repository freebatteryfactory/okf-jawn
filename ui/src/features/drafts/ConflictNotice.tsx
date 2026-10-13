/**
 * One item of a Snapshot that conflicted: what changed underneath the person's draft, and the two
 * ways out.
 *
 * The diff is the one the service sent with the conflict (`DraftConflictItem.changes`): for each
 * file of the item, the lines removed and added between the draft's base and the current
 * revision, with the unchanged lines around them. At most 2,000 of its lines are put on the page
 * and the rest are counted; a very long line is cut and says so. It is read-only text; added and removed lines
 * are `ins` and `del`, each also marked with a sign, so the difference does not rest on color.
 *
 * - "Keep mine" saves the draft again on the current revision (`save_draft` with `base_revision`
 *   set to `current_revision`, sent by the session's own save) and then takes the Snapshot of this
 *   item under the name the Snapshot had. If the item changed yet again, the service answers a new
 *   conflict, shown here.
 * - "Take theirs" discards the draft (`discard_draft`), after the person confirms in place. The
 *   newer version is someone else's head: it is shown once the workspace is refreshed.
 * - An item that was deleted cannot be kept: `save_draft` needs the item at the head. Its text
 *   stays in the editor to copy until the draft is discarded.
 *
 * Either way out ends with the notice gone; the caller says where keyboard focus goes. A failure
 * of either is shown here with its retry. No key is held here: a save's belongs to its session,
 * a discard's to its item and a Snapshot's to its workspace, so a retry is the same request under
 * the same key exactly while it says the same thing, wherever it is pressed from.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { MAX_DIFF_LINES, type PatchHunk, readPatch } from './patch';
import { DraftNotKept, discardSession, rebaseSession } from './saving';
import {
  type ConflictItem,
  type SnapshotConflict,
  useDraftSession,
  useDraftSessions,
} from './sessions';
import { draftConflictsOf, type Snapshotted, saySnapshotted, takeSnapshot } from './snapshot';

export interface ConflictNoticeProps {
  workspaceId: string;
  itemId: string;
  title: string;
  conflict: SnapshotConflict;
  /** "Keep mine" took the Snapshot of this item. */
  onSnapshotted: (snapshotted: Snapshotted) => void;
  /** The conflict is over, either way: the notice is about to go. */
  onResolved: () => void;
}

const KIND = { added: 'Added', modified: 'Changed', moved: 'Moved', removed: 'Deleted' } as const;

type FileChange = ConflictItem['changes'][number];

function paths(change: FileChange): string {
  const from = change.old_path ?? null;
  const to = change.new_path ?? null;
  if (from !== null && to !== null && from !== to) return `${from}, now ${to}`;
  return to ?? from ?? 'this item';
}

/** The mark after a line that was cut, in words: it is read out as well as seen. */
const CUT = ' … (a long line, cut here)';

function ChangedFile({ change, hunks }: { change: FileChange; hunks: PatchHunk[] }) {
  return (
    <div className="changed-file">
      <p>
        <strong>{KIND[change.kind]}:</strong> {paths(change)}
      </p>
      {change.binary ? (
        <p>This file is not text, so its lines cannot be compared.</p>
      ) : hunks.length === 0 ? (
        <p>No line of its text changed.</p>
      ) : (
        keyed(hunks, (hunk) => `${hunk.oldStart}:${hunk.newStart}`).map(({ key, value: hunk }) => (
          <div key={key} className="hunk">
            <p className="meta">From line {hunk.newStart} of the file</p>
            <pre className="patch-lines">
              {keyed(hunk.lines, (line) => `${line.kind}:${line.text}`).map(
                ({ key: lineKey, value: line }) =>
                  line.kind === 'added' ? (
                    <ins key={lineKey}>
                      <span className="sr-only">Added: </span>
                      <span aria-hidden="true">+ </span>
                      {line.text}
                      {line.cut ? CUT : ''}
                      {'\n'}
                    </ins>
                  ) : line.kind === 'removed' ? (
                    <del key={lineKey}>
                      <span className="sr-only">Removed: </span>
                      <span aria-hidden="true">− </span>
                      {line.text}
                      {line.cut ? CUT : ''}
                      {'\n'}
                    </del>
                  ) : (
                    <span key={lineKey}>
                      {'  '}
                      {line.text}
                      {line.cut ? CUT : ''}
                      {'\n'}
                    </span>
                  ),
              )}
            </pre>
          </div>
        ))
      )}
    </div>
  );
}

export function ConflictNotice({
  workspaceId,
  itemId,
  title,
  conflict,
  onSnapshotted,
  onResolved,
}: ConflictNoticeProps) {
  const client = useQueryClient();
  const heading = useId();
  // Input of the properties form that is held back is in the session, and goes with it.
  const session = useDraftSession(workspaceId, itemId);
  const heldBack = session !== undefined && session.heldBack !== null;
  const item = conflict.conflict;
  // The diff on the page is bounded: so many lines over all of the item's files, the rest counted.
  const files: Array<{ change: FileChange; hunks: PatchHunk[] }> = [];
  let room = MAX_DIFF_LINES;
  let more = 0;
  for (const change of item.changes) {
    const read = readPatch(change.patch, room);
    files.push({ change, hunks: read.hunks });
    room -= read.shown;
    more += read.more;
  }
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const theirsRef = useRef<HTMLButtonElement | null>(null);
  const keepDraftRef = useRef<HTMLButtonElement | null>(null);

  const keep = useMutation({
    mutationFn: async () => {
      // The draft is kept once the text on screen is saved on that revision: text typed
      // meanwhile is saved by it too, and a save that failed is its failure, said as such. The
      // Snapshot that follows in the same turn therefore finds nothing unsaved.
      await rebaseSession(client, workspaceId, itemId, item.current_revision);
      return takeSnapshot(client, workspaceId, [itemId], conflict.message);
    },
    onSuccess: (snapshotted) => {
      saySnapshotted(workspaceId, conflict.message, snapshotted);
      onResolved();
      onSnapshotted(snapshotted);
    },
  });
  // A new conflict for this item is shown as the conflict it is, not as a failure; a draft that
  // could not be kept without any request failing has its own words.
  const changedAgain = keep.isError && draftConflictsOf(keep.error) !== null;
  const notKept = keep.isError && keep.error instanceof DraftNotKept ? keep.error.reason : null;
  const keepFailed = keep.isError && !changedAgain && notKept === null;
  const keepFailure = useRetry({
    request: `${workspaceId}/${itemId}/${item.current_revision}`,
    failed: keepFailed,
    error: keep.error,
    run: () => keep.mutateAsync(),
    loaded: () => keepRef.current,
  });

  const [confirming, setConfirming] = useState(false);
  const focusAfterConfirm = useRef<'keep-draft' | 'theirs' | null>(null);
  useEffect(() => {
    const target = focusAfterConfirm.current;
    focusAfterConfirm.current = null;
    if (target === 'keep-draft') keepDraftRef.current?.focus();
    else if (target === 'theirs') theirsRef.current?.focus();
  });
  const theirs = useMutation({
    mutationFn: () => discardSession(client, workspaceId, itemId),
    onSuccess: () => {
      useDraftSessions
        .getState()
        .say(
          workspaceId,
          `Your draft of ${title} was discarded. The newer version is shown once the workspace is refreshed.`,
        );
      onResolved();
    },
  });
  const theirsFailure = useRetry({
    request: `${workspaceId}/${itemId}`,
    failed: theirs.isError,
    error: theirs.error,
    run: () => theirs.mutateAsync(),
    loaded: () => null,
  });
  const busy = keep.isPending || theirs.isPending;

  return (
    <section className="conflict notice" aria-labelledby={heading} tabIndex={-1} data-conflict>
      <h3 id={heading}>{title} changed since your draft was started</h3>
      <p>
        Your draft started from revision{' '}
        <code title={item.draft_base}>{short(item.draft_base)}</code>.{' '}
        {item.deleted ? (
          <>
            The item was deleted by revision{' '}
            <code title={item.current_revision}>{short(item.current_revision)}</code>.
          </>
        ) : (
          <>
            The item is now at revision{' '}
            <code title={item.current_revision}>{short(item.current_revision)}</code>.
          </>
        )}{' '}
        The Snapshot “{conflict.message}” did not include it.
      </p>
      <h4>What changed underneath</h4>
      {item.changes.length === 0 ? (
        <p>The service listed no changed file.</p>
      ) : (
        keyed(files, ({ change }) => JSON.stringify([change.old_path, change.new_path])).map(
          ({ key, value: file }) => (
            <ChangedFile key={key} change={file.change} hunks={file.hunks} />
          ),
        )
      )}
      {more > 0 && (
        <p className="meta">
          and {more.toLocaleString('en')} more {more === 1 ? 'line' : 'lines'}: this change is too
          long to show whole.
        </p>
      )}
      {item.deleted ? (
        <p>
          A draft of a deleted item cannot be snapshotted. Your text stays in the editor until you
          discard the draft: copy what you need first.
        </p>
      ) : null}
      {confirming ? (
        <fieldset className="confirm">
          <legend>Discard your draft and take the newer version?</legend>
          <p>
            {heldBack
              ? 'Your text here, and what you changed in the properties that is not saved, will be removed.'
              : 'Your text here will be removed.'}{' '}
            The newer version is shown once the workspace is refreshed.
          </p>
          <button
            type="button"
            ref={keepDraftRef}
            onClick={() => {
              focusAfterConfirm.current = 'theirs';
              setConfirming(false);
            }}
          >
            Keep my draft
          </button>{' '}
          <button
            type="button"
            aria-disabled={theirs.isPending}
            onClick={() => {
              if (!theirs.isPending) theirs.mutate();
            }}
          >
            {theirs.isPending ? 'Discarding…' : 'Discard my draft'}
          </button>
        </fieldset>
      ) : (
        <p className="conflict-actions">
          {item.deleted ? null : (
            <>
              {/* aria-disabled rather than disabled: each keeps focus while it works. */}
              <button
                type="button"
                ref={keepRef}
                aria-disabled={busy}
                onClick={() => {
                  if (!busy) keep.mutate();
                }}
              >
                {keep.isPending ? 'Keeping yours…' : 'Keep mine'}
              </button>{' '}
            </>
          )}
          <button
            type="button"
            ref={theirsRef}
            aria-disabled={busy}
            onClick={() => {
              if (busy) return;
              focusAfterConfirm.current = 'keep-draft';
              setConfirming(true);
            }}
          >
            Take theirs
          </button>
        </p>
      )}
      <p className="meta">
        {item.deleted
          ? 'Take theirs: your draft is discarded.'
          : 'Keep mine: your draft is saved on the newer version and snapshotted under the same name. Take theirs: your draft is discarded.'}
      </p>
      {notKept !== null && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>Your draft was not kept.</strong>
          </p>
          <p>
            {/* The pane takes this notice away while the item is refused, so the words for that
                are for a pane that draws it otherwise. */}
            {notKept === 'too-long'
              ? 'Its text is too long to save, and a draft has to be saved before it can be kept. Shorten the text, then keep it again.'
              : notKept === 'refused'
                ? 'This item is not a Note any more, so nothing of your draft of it is saved.'
                : 'This page has no draft of this item to keep: it may have been discarded or snapshotted elsewhere. Take theirs to close this notice.'}
          </p>
        </div>
      )}
      {keepFailure.shown && (
        <FailureNotice
          context="Keeping your draft"
          error={keepFailure.error}
          retry={keepFailure}
          workspaceId={workspaceId}
        />
      )}
      {theirsFailure.shown && (
        <FailureNotice
          context="Discarding your draft"
          error={theirsFailure.error}
          retry={theirsFailure}
          workspaceId={workspaceId}
        />
      )}
      {/* Present from the start, so each of these is announced. */}
      <p role="status" className="meta">
        {keep.isPending && !keepFailure.retrying ? 'Keeping your draft…' : ''}
        {theirs.isPending && !theirsFailure.retrying ? 'Discarding your draft…' : ''}
        {changedAgain && !keep.isPending
          ? 'The item changed again while your draft was being kept. Nothing was snapshotted; what is shown above is the newest change.'
          : ''}
      </p>
    </section>
  );
}
