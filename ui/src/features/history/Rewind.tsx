/**
 * Rewind, in place: what it does in plain words, what it will change (the comparison of the head
 * the workspace shows with the version chosen, through the one diff reader), a description, and
 * the button.
 *
 * Rewind makes one item, or the whole workspace, as it was at the version chosen, as a new
 * version on top of the head the workspace shows: no version is erased (SPEC section 8). The
 * panel says so before and after. It is offered only once its comparison has answered and lists
 * a change: nothing is rewound that the person has not been shown, and a version that differs in
 * nothing from the head is said to have nothing to rewind (the service refuses it on `/target`).
 *
 * The request is the tree's kind (`history.ts`, `rewind`): on the head shown, repeated under its
 * key while it says the same thing. Its answer becomes the head the workspace shows, the tabs
 * that cited the old head follow (`onChanged`), and the panel says what was done with the
 * commit's warnings and receipt (`tree/Committed.tsx`). An item the person had deleted in this
 * page and that the Rewind brings back is forgotten as deleted once the workspace shows the head
 * the Rewind made (`explorer/restored.ts` asks for it there).
 *
 * In an archived workspace the button stays and says why it cannot run, and nothing is sent.
 *
 * Focus. The panel takes keyboard focus on its heading when it opens, so its words and its
 * comparison are read first. Escape and Cancel close it, and the caller gives focus back to what
 * opened it. When the Rewind is done, the form leaves and focus goes to the words that say what
 * was done.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { type KeyboardEvent, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowed, archivedWords } from '../explorer/rights';
import { Committed } from '../tree/Committed';
import { type Attempt, attemptFor } from '../tree/changes';
import { useWorkspaceChanges } from '../tree/context';
import { Changes } from './Changes';
import { diffOptions, type RewindValues, type Rewound, rewind } from './history';

export interface RewindPanelProps {
  workspaceId: string;
  /** The version to bring back. */
  target: Revision;
  /** One item, or the whole workspace when null. */
  item: { id: string; title: string } | null;
  /** The heading's level. */
  level: 3 | 4;
  /** Leave the panel; the caller gives keyboard focus back to what opened it. */
  onClose: () => void;
}

export function RewindPanel({ workspaceId, target, item, level, onClose }: RewindPanelProps) {
  const client = useQueryClient();
  const { head, rights, onChanged } = useWorkspaceChanges();
  const headingId = useId();
  const archivedId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const doneRef = useRef<HTMLDivElement | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const itemId = item?.id ?? null;
  const [done, setDone] = useState<Rewound | null>(null);
  // Once done, the head is the one the Rewind made: nothing is compared with it here.
  const preview = useQuery({
    ...diffOptions(workspaceId, head, target, itemId),
    enabled: done === null,
  });
  const [message, setMessage] = useState(
    item === null
      ? `Rewind the workspace to revision ${short(target)}`
      : `Rewind ${item.title} to revision ${short(target)}`,
  );
  const last = useRef<Attempt<RewindValues> | null>(null);
  const send = useMutation({
    mutationFn: (attempt: Attempt<RewindValues>) => rewind(client, workspaceId, attempt),
    onSuccess: (rewound) => {
      setDone(rewound);
      onChanged(rewound);
    },
  });
  const run = () => {
    const attempt = attemptFor(last.current, head, { target, itemId, message });
    last.current = attempt;
    return send.mutateAsync(attempt);
  };
  const failure = useRetry({
    request: `${workspaceId}/${itemId ?? ''}/${target}`,
    failed: send.isError,
    error: send.error,
    run,
    loaded: () => doneRef.current,
  });
  const previewFailure = useRetry({
    request: `${workspaceId}/${head}/${target}/${itemId ?? ''}`,
    failed: preview.isError,
    error: preview.error,
    run: () => preview.refetch(),
    loaded: () => buttonRef.current ?? headingRef.current,
  });

  useLayoutEffect(() => {
    headingRef.current?.focus();
  }, []);
  // The form leaves when the Rewind is done: focus that was in it goes to the words saying so.
  const heldForm = useRef<HTMLElement | null>(null);
  const [, formRef] = useWatchedPart<HTMLDivElement>(heldForm);
  useLayoutEffect(() => {
    if (takeHeld(heldForm)) (doneRef.current ?? pageHeading())?.focus();
  });

  const can = allowed(rights, 'write');
  const changes = preview.data?.changes.length ?? null;
  const ready = changes !== null && changes > 0 && can === 'yes' && !send.isPending;
  const Heading = level === 3 ? 'h3' : 'h4';
  const what = item === null ? 'the workspace' : item.title;
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    onClose();
  };

  return (
    <section className="rewind" aria-labelledby={headingId} onKeyDown={onKeyDown}>
      <Heading id={headingId} ref={headingRef} tabIndex={-1}>
        Rewind {what} to revision <code title={target}>{short(target)}</code>
      </Heading>
      {done === null ? (
        // Each part has its own key: it is another element, removed whole, so its ref is cleared
        // while keyboard focus is still in it (`useWatchedPart`).
        <div key="form" ref={formRef}>
          <p>
            Rewind makes {item === null ? 'the whole workspace' : 'this item'} as it was at revision{' '}
            <code title={target}>{short(target)}</code> again, as a new version on top of revision{' '}
            <code title={head}>{short(head)}</code>, the one this workspace shows. No version is
            erased: every one stays in the Timeline, the one shown now included.
          </p>
          {preview.isPending && !previewFailure.retrying && (
            <p role="status">Comparing the two versions…</p>
          )}
          {previewFailure.shown && (
            <FailureNotice
              context="Comparing the version shown with this one"
              error={previewFailure.error}
              retry={previewFailure}
              workspaceId={workspaceId}
            />
          )}
          {preview.data !== undefined &&
            (preview.data.changes.length === 0 ? (
              <p>
                Nothing differs between this version and the one this workspace shows: there is
                nothing to rewind.
              </p>
            ) : (
              <Changes
                result={preview.data}
                title="What Rewind will change"
                level={level === 3 ? 4 : 5}
              />
            ))}
          <p className="change-field">
            <label>
              Describe this Rewind{' '}
              <input
                type="text"
                value={message}
                onChange={(event) => setMessage(event.target.value)}
              />
            </label>
          </p>
          <p className="change-buttons">
            {/* aria-disabled, never disabled: the button keeps focus while it works. */}
            <button
              type="button"
              ref={buttonRef}
              aria-disabled={!ready}
              {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
              onClick={() => {
                if (ready) void run().catch(() => undefined);
              }}
            >
              {send.isPending ? 'Rewinding…' : 'Rewind'}
            </button>{' '}
            <button type="button" onClick={onClose}>
              Cancel
            </button>
          </p>
          {can === 'archived' && rights.archivedAt !== null && (
            <p id={archivedId} className="meta">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          {failure.shown && (
            <FailureNotice
              context="Rewinding"
              error={failure.error}
              retry={failure}
              workspaceId={workspaceId}
            />
          )}
        </div>
      ) : (
        <div key="done" ref={doneRef} tabIndex={-1}>
          <p role="status">
            Rewound: {item === null ? 'the workspace' : item.title} is as it was at revision{' '}
            <code title={target}>{short(target)}</code>, in a new version,{' '}
            <code title={done.revision}>{short(done.revision)}</code>. Every earlier version is
            kept.
            {done.adopted
              ? ' The workspace now shows the new version.'
              : ' This page still shows another version: refresh the workspace to see the new one.'}
          </p>
          <Committed
            workspaceId={workspaceId}
            warnings={done.warnings}
            receiptId={done.receiptId}
          />
          <button type="button" onClick={onClose}>
            Close
          </button>
        </div>
      )}
    </section>
  );
}
