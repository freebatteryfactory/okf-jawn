/**
 * An item's reviews, beside it: what the recorded reviews cover at the revision the tab reads,
 * the claims its file brought with it, and Verify.
 *
 * Coverage is said exactly as `list_reviews` computed it for that revision (SPEC section 8: "A
 * review covers the exact displayed content and revision. Later changes display previous coverage
 * accurately instead of extending the badge"):
 * - "Verified at this revision" only when a review covers it (`current`), with who and when;
 * - a review of an earlier revision is said to cover that earlier revision only (`changed`), never
 *   the content shown;
 * - a review a purge invalidated is said to be invalidated;
 * - a claim the file makes in its own OKF `verified` (`imported`) is the file's words as written,
 *   labelled as imported with the file and never as Verified ("Imported human-looking metadata is
 *   not authenticated review evidence").
 *
 * Verify (`confirm.ts`) asks through the confirmation dialog, which shows the revision, the
 * digest the service gave for it and the item's exact content there (its properties and text, as
 * `get_item` answered them at that revision); it is offered once that answer is here. It needs
 * permission to verify: without it, the section says why; in an archived workspace the button
 * stays, says why it cannot run, and opens nothing. When the review is recorded, the dialog
 * closes, focus goes back to "Verify…", and a polite line says so.
 *
 * The reviews are kept current by the workspace's events (`feed.ts`): a review recorded
 * elsewhere is shown when its event comes. A read that fails then replaces nothing on screen:
 * the coverage last read stays, with the failure beside it.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useId, useRef, useState } from 'react';
import { listReviewsOptions } from '../../api/generated/@tanstack/react-query.gen';
import type {
  ImportedClaim,
  ListReviewsResponse,
  Review,
  ReviewCoverage,
  Revision,
} from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { itemOptions } from '../documents/DocumentView';
import { short } from '../documents/describe';
import { PropertyList } from '../documents/Properties';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowed, archivedWords, Missing } from '../explorer/rights';
import { useWorkspaceChanges } from '../tree/context';
import { ConfirmDialog } from './ConfirmDialog';
import { type Confirmed, confirmedFor, type VerifyValues, verifyItem } from './confirm';
import { useReviewFeed } from './feed';

export interface ItemReviewsProps {
  workspaceId: string;
  itemId: string;
  /** The revision the tab reads. */
  revision: Revision;
}

/** What one recorded review covers, in words. */
const COVERAGE_WORDS: Record<ReviewCoverage, string> = {
  current: 'Covers this revision',
  changed: 'Covers an earlier revision only; the item has changed since',
  invalidated: 'Invalidated by a purge',
  imported: 'A claim imported with the file, not a review recorded here',
  unreviewed: 'Does not cover this revision',
};

/** The one line that says how the item stands, from what the service computed. */
function Standing({ answer }: { answer: ListReviewsResponse }) {
  const current = answer.items.find((review) => review.coverage === 'current');
  if (current !== undefined) {
    return (
      <p className="review-standing">
        <strong className="review-badge">Verified at this revision</strong>{' '}
        <span className="meta">
          by {current.reviewer_subject} on <Instant value={current.reviewed_at} />
        </span>
      </p>
    );
  }
  if (answer.items.some((review) => review.coverage === 'changed')) {
    return (
      <p className="review-standing">
        Not verified at this revision. An earlier revision was verified; the item has changed since,
        so that review does not cover what is shown.
      </p>
    );
  }
  if ((answer.imported ?? []).length > 0) {
    return (
      <p className="review-standing">
        Not verified here. The file carries a claim that it was verified, imported with it; no
        review of it is recorded here.
      </p>
    );
  }
  if (answer.items.length > 0) {
    return <p className="review-standing">Not verified at this revision.</p>;
  }
  return <p className="review-standing">Not verified: no review of this item is recorded.</p>;
}

function ReviewRow({ review }: { review: Review }) {
  return (
    <li>
      <strong>{COVERAGE_WORDS[review.coverage] ?? 'A coverage this page does not know'}</strong>:
      verified by {review.reviewer_subject} on <Instant value={review.reviewed_at} />, revision{' '}
      <code title={review.source.revision}>{short(review.source.revision)}</code>
    </li>
  );
}

function ClaimRow({ claim }: { claim: ImportedClaim }) {
  return (
    <li>
      The file says it was verified by {claim.by ?? 'someone it does not name'}
      {claim.at ? `, at “${claim.at}” (as written in the file)` : ''}: imported with the file, not a
      review recorded here.
    </li>
  );
}

export function ItemReviews({ workspaceId, itemId, revision }: ItemReviewsProps) {
  const client = useQueryClient();
  const { rights } = useWorkspaceChanges();
  useReviewFeed(workspaceId);
  const headingId = useId();
  const archivedId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const verifyRef = useRef<HTMLButtonElement | null>(null);
  const reviews = useQuery(
    listReviewsOptions({
      body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
    }),
  );
  const item = useQuery(itemOptions(workspaceId, itemId, revision));
  const document = item.data;
  const answer = reviews.data;

  const nothingRead = answer === undefined;
  const firstFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: reviews.isError && nothingRead,
    error: reviews.error,
    run: () => reviews.refetch(),
    loaded: () => headingRef.current,
  });
  const laterFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: reviews.isError && !nothingRead,
    error: reviews.error,
    run: () => reviews.refetch(),
    loaded: () => headingRef.current,
  });

  const can = allowed(rights, 'review');
  const [open, setOpen] = useState(false);
  const [said, setSaid] = useState('');
  const last = useRef<Confirmed<VerifyValues> | null>(null);
  const verify = useMutation({
    mutationFn: (attempt: Confirmed<VerifyValues>) => verifyItem(client, workspaceId, attempt),
    onSuccess: (_review, attempt) => {
      setOpen(false);
      setSaid(`Your review of revision ${short(attempt.values.revision)} is recorded.`);
    },
  });
  const run = () => {
    if (document === undefined) return Promise.resolve();
    const attempt = confirmedFor(last.current, {
      itemId,
      path: document.summary.path,
      revision,
      contentDigest: document.content_digest,
    });
    last.current = attempt;
    return verify.mutateAsync(attempt);
  };
  const verifyFailure = useRetry({
    request: `${itemId} ${revision}`,
    failed: verify.isError,
    error: verify.error,
    run,
    loaded: () => verifyRef.current,
  });
  const ready = can === 'yes' && document !== undefined;

  return (
    <section className="reviews" aria-labelledby={headingId}>
      <h2 id={headingId} ref={headingRef} tabIndex={-1} className="panel-heading">
        Reviews
      </h2>
      {reviews.isPending && !firstFailure.retrying && <p role="status">Reading the reviews…</p>}
      {firstFailure.shown && (
        <FailureNotice
          context="Reading this item’s reviews"
          error={firstFailure.error}
          retry={firstFailure}
          workspaceId={workspaceId}
        />
      )}
      {answer !== undefined && (
        <>
          <Standing answer={answer} />
          {answer.items.length + (answer.imported ?? []).length > 0 && (
            <ul className="review-list">
              {keyed(answer.items, (review) => review.id).map(({ key, value: review }) => (
                <ReviewRow key={key} review={review} />
              ))}
              {keyed(answer.imported ?? [], (claim) => JSON.stringify(claim)).map(
                ({ key, value: claim }) => (
                  <ClaimRow key={key} claim={claim} />
                ),
              )}
            </ul>
          )}
        </>
      )}
      {laterFailure.shown && (
        <FailureNotice
          context="Reading this item’s reviews again"
          error={laterFailure.error}
          retry={laterFailure}
          workspaceId={workspaceId}
        />
      )}
      {can === 'no-permission' ? (
        <Missing action="Verifying this item" permission="review" />
      ) : (
        <p>
          {/* aria-disabled, never disabled: the button keeps focus, and says why it cannot run. */}
          <button
            type="button"
            ref={verifyRef}
            aria-disabled={!ready}
            {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
            onClick={() => {
              if (ready) setOpen(true);
            }}
          >
            Verify…
          </button>
        </p>
      )}
      {can === 'archived' && rights.archivedAt !== null && (
        <p id={archivedId} className="meta">
          {archivedWords(rights.archivedAt)}
        </p>
      )}
      <p role="status" className="meta">
        {said}
      </p>
      {document !== undefined && (
        <ConfirmDialog
          open={open}
          onClose={() => setOpen(false)}
          title={`Verify ${document.summary.title || document.summary.path} at revision ${short(revision)}?`}
          description={
            <p>
              Verifying records that you reviewed exactly this content at revision{' '}
              <code title={revision}>{short(revision)}</code>, whose content the service names{' '}
              <code title={document.content_digest}>{short(document.content_digest)}</code>. It
              covers this revision only: a later change is shown as not covered.
            </p>
          }
          confirmLabel="Verify this revision"
          busyLabel="Verifying…"
          busy={verify.isPending}
          onConfirm={() => void run().catch(() => undefined)}
          returnFocus={verifyRef}
          failure={
            verifyFailure.shown ? (
              <FailureNotice
                context="Verifying"
                error={verifyFailure.error}
                retry={verifyFailure}
                workspaceId={workspaceId}
              />
            ) : null
          }
        >
          <h3>Its properties</h3>
          {Object.keys(document.properties).length === 0 ? (
            <p>It has no properties.</p>
          ) : (
            <PropertyList properties={document.properties} />
          )}
          <h3>Its text</h3>
          {/* Read-only and selectable: a box that scrolls from the keyboard and holds the text as it is. */}
          <textarea
            className="confirm-text"
            aria-label="The text you verify"
            readOnly
            rows={12}
            value={document.body}
          />
        </ConfirmDialog>
      )}
    </section>
  );
}
