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
 *   set to `current_revision`) and then takes the Snapshot of this item under the name the
 *   Snapshot had. If the item changed yet again, the service answers a new conflict, shown here.
 * - "Take theirs" discards the draft (`discard_draft`), after the person confirms in place. The
 *   newer version is someone else's head: it is shown once the workspace is refreshed.
 * - An item that was deleted cannot be kept: `save_draft` needs the item at the head. Its text
 *   stays in the editor to copy until the draft is discarded.
 *
 * Either way out ends with the notice gone; the caller says where keyboard focus goes. A failure
 * of either is shown here with its retry, which repeats the same requests under the same keys.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { MAX_DIFF_LINES, type PatchHunk, readPatch } from './patch';
import { discardSession } from './saving';
import {
  type ConflictItem,
  type DraftText,
  type SnapshotConflict,
  useDraftSessions,
} from './sessions';
import {
  draftConflictsOf,
  rebaseDraft,
  type Snapshotted,
  saySnapshotted,
  takeSnapshot,
  UnsavedSelection,
} from './snapshot';

export interface ConflictNoticeProps {
  workspaceId: string;
  itemId: string;
  title: string;
  conflict: SnapshotConflict;
  /** The draft's content, when it is known: the editor's, or what `get_item` carried. */
  text: DraftText | null;
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
  text,
  onSnapshotted,
  onResolved,
}: ConflictNoticeProps) {
  const client = useQueryClient();
  const heading = useId();
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

  // One pair of keys for every attempt to keep this draft on this revision.
  const keys = useRef<{ revision: string; rebase: string; commit: string } | null>(null);
  const keep = useMutation({
    mutationFn: async () => {
      if (text === null) throw new UnsavedSelection([itemId]);
      const held =
        keys.current !== null && keys.current.revision === item.current_revision
          ? keys.current
          : {
              revision: item.current_revision,
              rebase: crypto.randomUUID(),
              commit: crypto.randomUUID(),
            };
      keys.current = held;
      await rebaseDraft(client, workspaceId, itemId, item.current_revision, text, held.rebase);
      return takeSnapshot(client, workspaceId, {
        key: held.commit,
        itemIds: [itemId],
        message: conflict.message,
      });
    },
    onSuccess: (snapshotted) => {
      keys.current = null;
      saySnapshotted(workspaceId, conflict.message, snapshotted);
      onResolved();
      onSnapshotted(snapshotted);
    },
  });
  // A new conflict for this item is shown as the conflict it is, not as a failure; text that is
  // not saved has its own words.
  const changedAgain = keep.isError && draftConflictsOf(keep.error) !== null;
  const notSaved = keep.isError && keep.error instanceof UnsavedSelection;
  const keepFailed = keep.isError && !changedAgain && !notSaved;
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
  const discardKey = useRef<string | null>(null);
  const theirs = useMutation({
    mutationFn: async () => {
      const key = discardKey.current ?? crypto.randomUUID();
      discardKey.current = key;
      await discardSession(client, workspaceId, itemId, key);
    },
    onSuccess: () => {
      discardKey.current = null;
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
            Your text here will be removed. The newer version is shown once the workspace is
            refreshed.
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
      {notSaved && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>Your text is not saved yet.</strong>
          </p>
          <p>It has to be saved before it can be kept. The editor below says why.</p>
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
