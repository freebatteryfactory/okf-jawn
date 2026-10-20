/**
 * One proposal: what it is and how it stands, what it proposes (its changes in words, and what
 * they change against the version it was made on, through the one diff reader), its discussion,
 * and Approve and Decline.
 *
 * A proposal that would store a View shows each such View drawn as proposed, beside what it changes
 * and before Approve (`documents/ProposedView.tsx`). What is drawn there, or said of a file that
 * does not read as a View, never holds Approve back: the service checks the View again at accept.
 *
 * The status is the service's (ruling R10): read again when the workspace's events say the
 * proposal changed, and after the person's own Approve or Decline, whatever its outcome. A read
 * that fails then replaces nothing on screen: the proposal last read stays, with the failure
 * beside it.
 *
 * Approve and Decline are offered for an open proposal to a person who may approve; without that
 * permission the page says why; in an archived workspace they stay, say why they cannot run, and
 * do nothing. Approve asks through the confirmation dialog (`reviews/ConfirmDialog.tsx`), which
 * shows the proposal's revision, the digest of its exact change set, its changes and what they
 * change; it is offered once that comparison has answered. Confirming issues a confirmation for
 * that revision and accepts it on the head the workspace shows (`proposals.ts`, `accept`); a retry
 * repeats both under their keys, so the person is never asked twice. A refusal because the head
 * moved names the head and offers to show it (`failure.ts`, `stale_base`). Decline opens in place,
 * asks for the reason the service records, and sends nothing without one.
 *
 * Focus. The dialog takes focus and gives it back to "Approve…". When the actions leave because
 * the proposal is no longer open (it was accepted or declined here or elsewhere) with focus in
 * them, focus goes to the line that says what was done, or to the page's heading.
 */
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { type KeyboardEvent, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Proposal } from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';
import { ProposedViews } from '../documents/ProposedView';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowed, archivedWords, Missing } from '../explorer/rights';
import { Changes } from '../history/Changes';
import { diffOptions } from '../history/history';
import { ConfirmDialog } from '../reviews/ConfirmDialog';
import { type Confirmed, confirmedFor } from '../reviews/confirm';
import { useReviewFeed } from '../reviews/feed';
import { Committed } from '../tree/Committed';
import { type Attempt, attemptFor } from '../tree/changes';
import { useWorkspaceChanges } from '../tree/context';
import { routeWords } from './ProposalsPage';
import {
  type Accepted,
  type AcceptValues,
  accept,
  changeWords,
  comment,
  commentsOptions,
  decline,
  itemsOf,
  KIND_WORDS,
  proposalOptions,
  STATUS_WORDS,
} from './proposals';

export interface ProposalPageProps {
  workspaceId: string;
  proposalId: string;
}

export function ProposalPage({ workspaceId, proposalId }: ProposalPageProps) {
  useReviewFeed(workspaceId);
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const read = useQuery(proposalOptions(workspaceId, proposalId));
  const proposal = read.data;
  const nothingRead = proposal === undefined;
  const firstFailure = useRetry({
    request: proposalId,
    failed: read.isError && nothingRead,
    error: read.error,
    run: () => read.refetch(),
    loaded: () => headingRef.current,
  });
  const laterFailure = useRetry({
    request: proposalId,
    failed: read.isError && !nothingRead,
    error: read.error,
    run: () => read.refetch(),
    loaded: () => headingRef.current,
  });
  return (
    <section className="workspace-page proposal" aria-labelledby={headingId}>
      <p className="meta">
        <Link to="/w/$workspaceId/proposals" params={{ workspaceId }}>
          All proposals
        </Link>
      </p>
      <h2 id={headingId} ref={headingRef} tabIndex={-1} className="page-heading">
        {proposal === undefined ? 'Proposal' : proposal.title || '(untitled proposal)'}
      </h2>
      {read.isPending && !firstFailure.retrying && <p role="status">Reading the proposal…</p>}
      {firstFailure.shown && (
        <FailureNotice
          context="Reading this proposal"
          error={firstFailure.error}
          retry={firstFailure}
          workspaceId={workspaceId}
        />
      )}
      {proposal !== undefined && (
        <>
          <ProposalBody workspaceId={workspaceId} proposal={proposal} />
          {laterFailure.shown && (
            <FailureNotice
              context="Reading this proposal again"
              error={laterFailure.error}
              retry={laterFailure}
              workspaceId={workspaceId}
            />
          )}
          <ProposalActions workspaceId={workspaceId} proposal={proposal} />
          <Discussion workspaceId={workspaceId} proposalId={proposal.id} />
        </>
      )}
    </section>
  );
}

/** What the proposal is, how it stands, and what it proposes. */
function ProposalBody({ workspaceId, proposal }: { workspaceId: string; proposal: Proposal }) {
  const listId = useId();
  return (
    <>
      <p>
        <strong>{STATUS_WORDS[proposal.status] ?? proposal.status}</strong>
      </p>
      <p className="meta">
        {KIND_WORDS[proposal.kind] ?? proposal.kind}, proposed by {proposal.created_by} through{' '}
        {routeWords(proposal)} on <Instant value={proposal.created_at} />. It was made on revision{' '}
        <code title={proposal.base_revision}>{short(proposal.base_revision)}</code>; its proposed
        content is revision{' '}
        <code title={proposal.proposal_revision}>{short(proposal.proposal_revision)}</code>.
      </p>
      {proposal.description.length > 0 && (
        <p className="proposal-description">{proposal.description}</p>
      )}
      {keyed(proposal.warnings ?? [], (warning) => JSON.stringify(warning)).map(
        ({ key, value: warning }) => (
          <p key={key} role="note" className="notice">
            {warning.message}
            {warning.location ? ` (${warning.location})` : ''}
          </p>
        ),
      )}
      <h3 id={listId}>What it proposes</h3>
      <ul aria-labelledby={listId}>
        {keyed(proposal.changes, (change) => JSON.stringify(change)).map(
          ({ key, value: change }) => (
            <li key={key}>{changeWords(change)}</li>
          ),
        )}
      </ul>
      {/* Each View it would store, drawn as proposed, beside what it changes and before Approve:
          side by side where the column is wide enough, else the View first, since the change of a
          View file is its whole block. For a proposal that stores none, its place takes no room. */}
      <div className="@container">
        <div className="grid items-start gap-x-6 @5xl:grid-cols-2">
          <div className="min-w-0 empty:hidden">
            <ProposedViews workspaceId={workspaceId} proposal={proposal} />
          </div>
          <div className="min-w-0">
            <ProposedChanges workspaceId={workspaceId} proposal={proposal} level={3} />
          </div>
        </div>
      </div>
    </>
  );
}

/** What the proposal changes against the version it was made on. */
function ProposedChanges({
  workspaceId,
  proposal,
  level,
}: {
  workspaceId: string;
  proposal: Proposal;
  level: 3 | 4;
}) {
  const diff = useQuery(
    diffOptions(workspaceId, proposal.base_revision, proposal.proposal_revision, null),
  );
  const failure = useRetry({
    request: proposal.proposal_revision,
    failed: diff.isError,
    error: diff.error,
    run: () => diff.refetch(),
    loaded: () => null,
  });
  return (
    <>
      {diff.isPending && !failure.retrying && <p role="status">Comparing it with its version…</p>}
      {failure.shown && (
        <FailureNotice
          context="Comparing the proposal with the version it was made on"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {diff.data !== undefined && (
        <Changes result={diff.data} title="What it changes" level={level} />
      )}
    </>
  );
}

/** Approve and Decline, for an open proposal. */
function ProposalActions({ workspaceId, proposal }: { workspaceId: string; proposal: Proposal }) {
  const client = useQueryClient();
  const { head, rights, onChanged } = useWorkspaceChanges();
  const archivedId = useId();
  const approveRef = useRef<HTMLButtonElement | null>(null);
  const declineRef = useRef<HTMLButtonElement | null>(null);
  const doneRef = useRef<HTMLParagraphElement | null>(null);
  const can = allowed(rights, 'approve');
  const open = proposal.status === 'open';
  const diff = useQuery(
    diffOptions(workspaceId, proposal.base_revision, proposal.proposal_revision, null),
  );

  const [asking, setAsking] = useState(false);
  const [accepted, setAccepted] = useState<Accepted | null>(null);
  const last = useRef<Confirmed<AcceptValues> | null>(null);
  const accepting = useMutation({
    mutationFn: (attempt: Confirmed<AcceptValues>) => accept(client, workspaceId, attempt),
    onSuccess: (done) => {
      setAsking(false);
      setAccepted(done);
      onChanged(done);
    },
  });
  const runAccept = () => {
    const attempt = confirmedFor(last.current, {
      proposalId: proposal.id,
      proposalRevision: proposal.proposal_revision,
      contentDigest: proposal.content_digest,
      expectedHead: head,
      itemIds: itemsOf(proposal),
    });
    last.current = attempt;
    return accepting.mutateAsync(attempt);
  };
  const acceptFailure = useRetry({
    request: `${proposal.id} ${proposal.proposal_revision} ${head}`,
    failed: accepting.isError,
    error: accepting.error,
    run: runAccept,
    loaded: () => approveRef.current ?? doneRef.current,
  });

  // Decline, in place: the reason, then the request.
  const [declining, setDeclining] = useState(false);
  const [declined, setDeclined] = useState(false);
  const [reason, setReason] = useState('');
  const [reasonMissing, setReasonMissing] = useState(false);
  const reasonRef = useRef<HTMLTextAreaElement | null>(null);
  const lastDecline = useRef<Attempt<{ proposalId: string; reason: string }> | null>(null);
  const declineSend = useMutation({
    mutationFn: (attempt: Attempt<{ proposalId: string; reason: string }>) =>
      decline(client, workspaceId, attempt),
    onSuccess: () => {
      setDeclined(true);
      setDeclining(false);
    },
  });
  const runDecline = () => {
    const attempt = attemptFor(lastDecline.current, head, {
      proposalId: proposal.id,
      reason: reason.trim(),
    });
    lastDecline.current = attempt;
    return declineSend.mutateAsync(attempt);
  };
  const declineFailure = useRetry({
    request: proposal.id,
    failed: declineSend.isError,
    error: declineSend.error,
    run: runDecline,
    loaded: () => declineRef.current ?? doneRef.current,
  });
  const giveBack = useRef(false);
  useLayoutEffect(() => {
    if (declining) reasonRef.current?.focus();
  }, [declining]);
  useLayoutEffect(() => {
    if (!giveBack.current || declining) return;
    giveBack.current = false;
    declineRef.current?.focus();
  });
  const closeDecline = () => {
    giveBack.current = true;
    setDeclining(false);
  };

  // The decline form leaves when it is done or left: focus that was in it goes to the button that
  // opened it. The actions leave when the proposal is no longer open: focus that was in them goes
  // to the line that says what was done, else to the page's heading.
  const heldForm = useRef<HTMLElement | null>(null);
  const [, formRef] = useWatchedPart<HTMLDivElement>(heldForm);
  const heldActions = useRef<HTMLElement | null>(null);
  const [, actionsRef] = useWatchedPart<HTMLDivElement>(heldActions);
  useLayoutEffect(() => {
    if (takeHeld(heldForm)) (declineRef.current ?? doneRef.current ?? pageHeading())?.focus();
    if (takeHeld(heldActions)) (doneRef.current ?? pageHeading())?.focus();
  });

  const ready = can === 'yes' && open && diff.data !== undefined && !accepting.isPending;
  const onEscape = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    closeDecline();
  };

  return (
    <>
      {declined && accepted === null && (
        <p ref={doneRef} tabIndex={-1} role="status">
          Declined: your reason is recorded in the discussion.
        </p>
      )}
      {accepted !== null && (
        <div className="committed-said">
          <p ref={doneRef} tabIndex={-1} role="status">
            Accepted: its changes are in a new version,{' '}
            <code title={accepted.revision}>{short(accepted.revision)}</code>.
            {accepted.adopted
              ? ' The workspace now shows it.'
              : ' This page still shows another version: refresh the workspace to see it.'}
          </p>
          <Committed
            workspaceId={workspaceId}
            warnings={accepted.warnings}
            receiptId={accepted.receiptId}
          />
        </div>
      )}
      {open && can === 'no-permission' && (
        <Missing action="Approving or declining a proposal" permission="approve" />
      )}
      {open && can !== 'no-permission' && (
        <div ref={actionsRef} className="proposal-actions">
          <p>
            {/* aria-disabled, never disabled: each keeps focus, and says why it cannot run. */}
            <button
              type="button"
              ref={approveRef}
              aria-disabled={!ready}
              {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
              onClick={() => {
                if (ready) setAsking(true);
              }}
            >
              Approve…
            </button>{' '}
            <button
              type="button"
              ref={declineRef}
              aria-disabled={can !== 'yes'}
              aria-expanded={declining}
              {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
              onClick={() => {
                if (can === 'yes') setDeclining((now) => !now);
              }}
            >
              Decline…
            </button>
          </p>
          {can === 'archived' && rights.archivedAt !== null && (
            <p id={archivedId} className="meta">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          {declining && (
            <div ref={formRef} className="change-form">
              <p className="change-field">
                <label>
                  Why it is declined{' '}
                  <textarea
                    ref={reasonRef}
                    value={reason}
                    rows={3}
                    aria-invalid={reasonMissing}
                    // Escape leaves the form, as Cancel does.
                    onKeyDown={onEscape}
                    onChange={(event) => {
                      setReason(event.target.value);
                      setReasonMissing(false);
                    }}
                  />
                </label>
              </p>
              {reasonMissing && <p className="field-error">Say why it is declined first.</p>}
              <p className="meta">The reason is recorded in the discussion.</p>
              <p className="change-buttons">
                <button
                  type="button"
                  aria-disabled={declineSend.isPending}
                  onClick={() => {
                    if (declineSend.isPending) return;
                    if (reason.trim().length === 0) {
                      setReasonMissing(true);
                      reasonRef.current?.focus();
                      return;
                    }
                    void runDecline().catch(() => undefined);
                  }}
                >
                  {declineSend.isPending ? 'Declining…' : 'Decline'}
                </button>{' '}
                <button type="button" onClick={closeDecline}>
                  Cancel
                </button>
              </p>
              {declineFailure.shown && (
                <FailureNotice
                  context="Declining"
                  error={declineFailure.error}
                  retry={declineFailure}
                  workspaceId={workspaceId}
                />
              )}
            </div>
          )}
        </div>
      )}
      <ConfirmDialog
        open={asking}
        onClose={() => setAsking(false)}
        title={`Accept “${proposal.title || 'this proposal'}”?`}
        description={
          <p>
            Accepting applies exactly this proposal, revision{' '}
            <code title={proposal.proposal_revision}>{short(proposal.proposal_revision)}</code> with
            the change set the service names{' '}
            <code title={proposal.content_digest}>{short(proposal.content_digest)}</code>, as a new
            version on top of revision <code title={head}>{short(head)}</code>, the one this
            workspace shows.
          </p>
        }
        confirmLabel="Accept this proposal"
        busyLabel="Accepting…"
        busy={accepting.isPending}
        onConfirm={() => void runAccept().catch(() => undefined)}
        // Once accepted, Approve is about to leave with the proposal no longer open: focus goes to
        // the line that says what was done.
        returnFocus={accepted !== null ? doneRef : approveRef}
        failure={
          acceptFailure.shown ? (
            <FailureNotice
              context="Accepting"
              error={acceptFailure.error}
              retry={acceptFailure}
              workspaceId={workspaceId}
            />
          ) : null
        }
      >
        <h3>What it proposes</h3>
        <ul>
          {keyed(proposal.changes, (change) => JSON.stringify(change)).map(
            ({ key, value: change }) => (
              <li key={key}>{changeWords(change)}</li>
            ),
          )}
        </ul>
        {diff.data !== undefined && (
          <Changes result={diff.data} title="What it changes" level={4} />
        )}
      </ConfirmDialog>
    </>
  );
}

/** The proposal's comments, oldest first, and a new one. */
function Discussion({ workspaceId, proposalId }: { workspaceId: string; proposalId: string }) {
  const client = useQueryClient();
  const { head, rights } = useWorkspaceChanges();
  const headingId = useId();
  const archivedId = useId();
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const boxRef = useRef<HTMLTextAreaElement | null>(null);
  const comments = useInfiniteQuery(commentsOptions(workspaceId, proposalId));
  const items = (comments.data?.pages ?? []).flatMap((page) => page.items);
  const rowAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>(':scope > li').item(index) ?? null;
  const nothingListed = comments.data === undefined;
  const firstFailure = useRetry({
    request: proposalId,
    failed: comments.isError && nothingListed,
    error: comments.error,
    run: () => comments.refetch(),
    loaded: () => rowAt(0),
  });
  const laterFailure = useRetry({
    request: proposalId,
    failed: comments.isError && !nothingListed,
    error: comments.error,
    run: () => (comments.isFetchNextPageError ? comments.fetchNextPage() : comments.refetch()),
    loaded: () => rowAt(items.length - 1),
  });
  const count = items.length;
  const focusFrom = useRef<number | null>(null);
  useEffect(() => {
    const from = focusFrom.current;
    if (from === null || comments.isFetchingNextPage) return;
    if (count <= from && !comments.isFetchNextPageError && comments.hasNextPage) return;
    focusFrom.current = null;
    (rowAt(from) ?? moreRef.current ?? rowAt(count - 1))?.focus();
  });

  const can = allowed(rights, 'write');
  const [text, setText] = useState('');
  const [empty, setEmpty] = useState(false);
  const last = useRef<Attempt<{ proposalId: string; text: string }> | null>(null);
  const send = useMutation({
    mutationFn: (attempt: Attempt<{ proposalId: string; text: string }>) =>
      comment(client, workspaceId, attempt),
    onSuccess: () => setText(''),
  });
  const run = () => {
    const attempt = attemptFor(last.current, head, { proposalId, text: text.trim() });
    last.current = attempt;
    return send.mutateAsync(attempt);
  };
  const failure = useRetry({
    request: proposalId,
    failed: send.isError,
    error: send.error,
    run,
    loaded: () => boxRef.current,
  });
  const ready = can === 'yes' && !send.isPending;

  return (
    <section className="discussion" aria-labelledby={headingId}>
      <h3 id={headingId}>Discussion</h3>
      {comments.isPending && !firstFailure.retrying && <p role="status">Reading the comments…</p>}
      {firstFailure.shown && (
        <FailureNotice
          context="Reading the comments"
          error={firstFailure.error}
          retry={firstFailure}
          workspaceId={workspaceId}
        />
      )}
      {comments.data !== undefined && items.length === 0 && <p>No comments yet.</p>}
      {items.length > 0 && (
        <ul ref={listRef} aria-labelledby={headingId} className="comment-list">
          {keyed(items, (entry) => entry.id).map(({ key, value: entry }) => (
            <li key={key} tabIndex={-1}>
              <p className="comment-text">{entry.text}</p>
              <p className="meta">
                {entry.author}, <Instant value={entry.created_at} />
                {entry.source
                  ? `, about ${entry.source.path} at revision ${short(entry.source.revision)}`
                  : ''}
                {entry.invalidated_by ? '. The place it was about was removed by a purge.' : ''}
              </p>
            </li>
          ))}
        </ul>
      )}
      {laterFailure.shown && (
        <FailureNotice
          context="Reading more comments"
          error={laterFailure.error}
          retry={laterFailure}
          workspaceId={workspaceId}
        />
      )}
      {comments.hasNextPage && (
        <button
          type="button"
          ref={moreRef}
          aria-disabled={comments.isFetchingNextPage}
          onClick={() => {
            if (comments.isFetchingNextPage) return;
            focusFrom.current = count;
            void comments.fetchNextPage();
          }}
        >
          {comments.isFetchingNextPage ? 'Reading more comments…' : 'Show more comments'}
        </button>
      )}
      {can === 'no-permission' ? (
        <Missing action="Commenting" permission="write" />
      ) : (
        <div className="change-form">
          <p className="change-field">
            <label>
              Your comment{' '}
              <textarea
                ref={boxRef}
                rows={3}
                value={text}
                aria-invalid={empty}
                onChange={(event) => {
                  setText(event.target.value);
                  setEmpty(false);
                }}
              />
            </label>
          </p>
          {empty && <p className="field-error">Write the comment first.</p>}
          <p className="meta">A comment is discussion: it does not verify or approve anything.</p>
          <p className="change-buttons">
            <button
              type="button"
              aria-disabled={!ready}
              {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
              onClick={() => {
                if (!ready) return;
                if (text.trim().length === 0) {
                  setEmpty(true);
                  boxRef.current?.focus();
                  return;
                }
                void run().catch(() => undefined);
              }}
            >
              {send.isPending ? 'Adding…' : 'Add the comment'}
            </button>
          </p>
          {can === 'archived' && rights.archivedAt !== null && (
            <p id={archivedId} className="meta">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          {failure.shown && (
            <FailureNotice
              context="Adding the comment"
              error={failure.error}
              retry={failure}
              workspaceId={workspaceId}
            />
          )}
        </div>
      )}
    </section>
  );
}
