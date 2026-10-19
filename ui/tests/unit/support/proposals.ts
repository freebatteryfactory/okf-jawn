/**
 * Contract-valid answers for proposals and their discussion, over the workspace of
 * `workspace.tsx`.
 */
import type { Comment, Proposal } from '../../../src/api/generated/types.gen';
import type { Json } from './api';
import { HEAD, NOTE, OLD, WS } from './workspace';

export const PROPOSALS = '/api/proposals/list-proposals';
export const PROPOSAL = '/api/proposals/get-proposal';
export const PROPOSE = '/api/proposals/open-proposal';
export const ACCEPT = '/api/proposals/accept-proposal';
export const DECLINE = '/api/proposals/decline-proposal';
export const COMMENT = '/api/proposals/add-comment';
export const COMMENTS = '/api/proposals/list-comments';

export const PROPOSAL_ID = '99999999-1111-4111-8111-111111111111';
export const SECOND_ID = '99999999-2222-4222-8222-222222222222';
/** The candidate revision a proposal's content is held at. */
export const CANDIDATE = '2'.repeat(40);
/** The digest of a proposal's exact change set. */
export const CHANGES_DIGEST = '3'.repeat(64);
export const PROPOSED_AT = '2026-01-01T09:30:00.000Z';

export function proposalOf(extra: Partial<Proposal> = {}): Proposal {
  return {
    id: PROPOSAL_ID,
    workspace_id: WS,
    base_revision: HEAD,
    proposal_revision: CANDIDATE,
    content_digest: CHANGES_DIGEST,
    title: 'Publish the budget',
    description: 'The board approved it on Monday.',
    changes: [
      {
        kind: 'edit',
        item_id: NOTE,
        body: '# Meeting notes\n\nThe budget is published.\n',
        properties: { type: 'meeting-note' },
      },
    ],
    kind: 'content',
    status: 'open',
    created_by: 'claude-agent',
    created_via: 'mcp_delegation',
    created_at: PROPOSED_AT,
    warnings: [],
    ...extra,
  };
}

/** A second proposal, accepted, made on an older version. */
export const acceptedProposal = proposalOf({
  id: SECOND_ID,
  title: 'Add the minutes',
  status: 'accepted',
  base_revision: OLD,
  created_by: 'ada',
  created_via: 'browser_session',
});

export function listOf(items: Proposal[], next: string | null = null): Json {
  return { items, next_cursor: next };
}

export function commentOf(id: string, text: string, author = 'ada'): Comment {
  return { id, author, text, created_at: '2026-01-01T09:45:00.000Z' };
}
