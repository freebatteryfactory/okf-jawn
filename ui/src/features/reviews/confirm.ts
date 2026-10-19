/**
 * A person's confirmed action: a confirmation bound to the exact revision and content they were
 * shown (`create_confirmation`), then the action that consumes it (SPEC section 8: "Human
 * confirmation requires an explicit revision-bound browser action and application checks").
 *
 * The two requests are one attempt with two keys, one for each. The attempt is kept while it says
 * the same thing (the same target, revision and digest), so a retry after either step failed
 * sends both again under the same keys: the confirmation's key answers the confirmation the first
 * try issued, and the action's key resumes the action, which "may re-consume successfully" its
 * own confirmation. A crash between the two therefore never asks the person to confirm twice.
 * Anything else shown is another attempt, with new keys.
 *
 * Verify (`create_review`) covers the whole item at that revision: its source names the item's
 * path there and the selection `all`, and the digest is the one `get_item` answered
 * (`ItemDocument.content_digest`: "a client never computes it").
 */
import type { QueryClient } from '@tanstack/react-query';
import { createConfirmation, createReview } from '../../api/generated/sdk.gen';
import type { Review, Revision } from '../../api/generated/types.gen';
import { reviewsChanged } from './feed';

/** One confirmed action, with the key of each of its two requests. */
export interface Confirmed<T> {
  values: T;
  confirmKey: string;
  actionKey: string;
}

/** The attempt for these values: the last one again while it says the same. */
export function confirmedFor<T>(last: Confirmed<T> | null, values: T): Confirmed<T> {
  if (last !== null && JSON.stringify(last.values) === JSON.stringify(values)) return last;
  return { values, confirmKey: crypto.randomUUID(), actionKey: crypto.randomUUID() };
}

/** What a Verify covers: one item, whole, at one revision, with the digest the person was shown. */
export interface VerifyValues {
  itemId: string;
  path: string;
  revision: Revision;
  contentDigest: string;
}

/** Confirm, then record the review; the item's reviews are asked again. */
export async function verifyItem(
  client: QueryClient,
  workspaceId: string,
  attempt: Confirmed<VerifyValues>,
): Promise<Review> {
  const { itemId, path, revision, contentDigest } = attempt.values;
  const { data: confirmation } = await createConfirmation({
    body: {
      workspace_id: workspaceId,
      action: 'review',
      target: { kind: 'item', item_id: itemId },
      revision,
      content_digest: contentDigest,
      idempotency_key: attempt.confirmKey,
    },
    throwOnError: true,
  });
  const { data } = await createReview({
    body: {
      source: {
        workspace_id: workspaceId,
        item_id: itemId,
        path,
        revision,
        selection: { kind: 'all' },
      },
      content_digest: contentDigest,
      confirmation_id: confirmation.id,
      idempotency_key: attempt.actionKey,
    },
    throwOnError: true,
  });
  reviewsChanged(client, workspaceId, itemId);
  return data;
}
