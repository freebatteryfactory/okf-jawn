/**
 * Contract-valid answers for review: confirmations, reviews and their coverage, and an item's
 * links, over the workspace of `workspace.tsx`.
 */
import type { Review, ReviewCoverage } from '../../../src/api/generated/types.gen';
import type { Json } from './api';
import { DIGEST, HEAD, NOTE, noteSummary, WS } from './workspace';

export const CONFIRM = '/api/reviews/create-confirmation';
export const REVIEW = '/api/reviews/create-review';
export const REVIEWS = '/api/reviews/list-reviews';
export const LINKS = '/api/search/get-links';
export const GRAPH = '/api/search/get-graph';

export const CONFIRMATION_ID = '66666666-6666-4666-8666-666666666666';
export const REVIEW_ID = '77777777-7777-4777-8777-777777777777';
export const REVIEWED_AT = '2026-01-01T10:15:00.000Z';

/** A recorded review of the note at `revision`, with the coverage the service computed. */
export function reviewOf(coverage: ReviewCoverage, revision = HEAD, reviewer = 'grace'): Review {
  return {
    id: REVIEW_ID,
    source: {
      workspace_id: WS,
      item_id: NOTE,
      path: noteSummary.path,
      revision,
      selection: { kind: 'all' },
      locations: [],
    },
    content_digest: DIGEST,
    reviewer_subject: reviewer,
    reviewed_at: REVIEWED_AT,
    coverage,
  };
}

/** A `list_reviews` answer. */
export function reviewsAnswer(items: Review[], imported: Json[] = []): Json {
  return { items, imported };
}

/** A `create_confirmation` answer bound to the request's revision. */
export function confirmationFor(request: Json): Json {
  return {
    id: CONFIRMATION_ID,
    revision: String(request.revision),
    expires_at: '2026-01-01T10:25:00.000Z',
  };
}
