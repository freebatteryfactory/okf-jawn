/**
 * Proposals in the cache, their words, and the requests that open, accept, decline and discuss
 * them (SPEC section 8: "An explicitly allowed drafter may create proposals. ... An authorized
 * person can accept or decline the exact proposal shown. Stale preconditions require
 * reconciliation").
 *
 * A proposal's status is shown exactly as the service answers it; the page adds no account of how
 * it came to be (ruling R10). A proposal and its comments are asked again when the workspace's
 * events say it changed (`reviews/feed.ts`).
 *
 * Accepting is a confirmed action (`reviews/confirm.ts`): a confirmation bound to the proposal's
 * revision and the digest of its exact change set, then `accept_proposal` naming the head the
 * workspace shows (`expected_head`). The revision it answers becomes the head the workspace shows
 * when it still shows that head, as every change of the person's own does. Declining needs no
 * confirmation: its reason is recorded as a comment by the service. Every write carries its key,
 * kept while the request says the same thing.
 *
 * The changes that may store a View (`viewCandidates`) are the ones the service checks as Views:
 * the proposal page draws each as proposed (`documents/ProposedView.tsx`).
 */
import { infiniteQueryOptions, type QueryClient } from '@tanstack/react-query';
import {
  getProposalOptions,
  listCommentsQueryKey,
  listProposalsQueryKey,
} from '../../api/generated/@tanstack/react-query.gen';
import {
  acceptProposal,
  addComment,
  createConfirmation,
  declineProposal,
  listComments,
  listProposals,
  openProposal,
} from '../../api/generated/sdk.gen';
import type {
  Change,
  Comment,
  Proposal,
  ProposalKind,
  ProposalStatus,
  ReceiptId,
  Revision,
  Warning,
} from '../../api/generated/types.gen';
import { adoptHeadFrom, checkHeadsAgain, type HeadChange } from '../explorer/head';
import type { Confirmed } from '../reviews/confirm';
import { proposalsChanged } from '../reviews/feed';
import type { Attempt } from '../tree/changes';
import type { ViewFile } from '../views/view-file';

/** Proposals asked for per page. */
export const PROPOSALS_PAGE = 50;
/** Comments asked for per page. */
export const COMMENTS_PAGE = 50;

/** The proposals of a workspace, of one status when `status` is given, newest first. */
export function proposalsOptions(workspaceId: string, status: ProposalStatus | null) {
  const body = {
    workspace_id: workspaceId,
    ...(status === null ? {} : { status }),
  };
  return infiniteQueryOptions({
    queryKey: [
      {
        ...listProposalsQueryKey({ body: { ...body, page: { limit: PROPOSALS_PAGE } } })[0],
        _infinite: true,
      },
    ] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listProposals({
        body: {
          ...body,
          page:
            pageParam === null
              ? { limit: PROPOSALS_PAGE }
              : { limit: PROPOSALS_PAGE, cursor: pageParam },
        },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

export function proposalOptions(workspaceId: string, proposalId: string) {
  return getProposalOptions({ body: { workspace_id: workspaceId, proposal_id: proposalId } });
}

/** A proposal's comments, oldest first. */
export function commentsOptions(workspaceId: string, proposalId: string) {
  const body = { workspace_id: workspaceId, proposal_id: proposalId };
  return infiniteQueryOptions({
    queryKey: [
      {
        ...listCommentsQueryKey({ body: { ...body, page: { limit: COMMENTS_PAGE } } })[0],
        _infinite: true,
      },
    ] as const,
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listComments({
        body: {
          ...body,
          page:
            pageParam === null
              ? { limit: COMMENTS_PAGE }
              : { limit: COMMENTS_PAGE, cursor: pageParam },
        },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** A proposal's status, in words. */
export const STATUS_WORDS: Record<ProposalStatus, string> = {
  open: 'Open: waiting for someone who may approve it',
  accepted: 'Accepted',
  declined: 'Declined',
  conflict:
    'In conflict: the workspace changed underneath it, so it can no longer be accepted. A new proposal made on the current version is needed',
};

/** What a proposal does, in words; the service derives it from the changes. */
export const KIND_WORDS: Record<ProposalKind, string> = {
  content: 'A change of content',
  supply_extraction: 'Text supplied by an agent for a source the converter did not fully read',
};

/** One proposed change, in words. */
export function changeWords(change: Change): string {
  switch (change.kind) {
    case 'create':
      return `Create ${change.item_kind === 'view' ? 'a View' : 'a note'} at ${change.path}, of type ${change.type_name}`;
    case 'edit':
      return 'Change the text and properties of an item';
    case 'move':
      return `Move an item to ${change.destination}`;
    case 'archive':
      return 'Archive an item';
    case 'supply_extraction':
      return (change.pages ?? []).length === 0
        ? 'Supply the whole text of a source'
        : `Supply the text of a source for pages ${(change.pages ?? []).map((range) => (range.start === range.end ? `${range.start}` : `${range.start} to ${range.end}`)).join(', ')}`;
  }
}

/**
 * A change that may store a View, as the service checks one when the proposal is opened and again
 * when it is accepted (`check_proposed_view`): a create of a View, or an edit, which stores one
 * when the item is a View at the proposal's base (so its kind is read there). Its file is the
 * change's own properties and body: the exact proposed content.
 */
export type ViewCandidate = { key: string; file: ViewFile } & (
  | { kind: 'created'; path: string }
  | { kind: 'edited'; itemId: string }
);

/** The changes of a proposal that may store a View, in the order of its changes. */
export function viewCandidates(proposal: Proposal): ViewCandidate[] {
  return proposal.changes.flatMap((change, index): ViewCandidate[] => {
    const key = `${proposal.proposal_revision}/${index}`;
    if (change.kind === 'create' && change.item_kind === 'view')
      return [
        {
          key,
          kind: 'created',
          path: change.path,
          file: { properties: change.properties, body: change.body },
        },
      ];
    if (change.kind === 'edit')
      return [
        {
          key,
          kind: 'edited',
          itemId: change.item_id,
          file: { properties: change.properties, body: change.body },
        },
      ];
    return [];
  });
}

/** The items a proposal's changes name; a tab of one that cited the old head follows. */
export function itemsOf(proposal: Proposal): string[] {
  return proposal.changes.flatMap((change) => ('item_id' in change ? [change.item_id] : []));
}

/** Propose a new text for one item, on the head the workspace shows. */
export interface ProposedEdit {
  itemId: string;
  title: string;
  description: string;
  body: string;
  properties: Record<string, unknown>;
}

export async function proposeEdit(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<ProposedEdit>,
): Promise<Proposal> {
  const { itemId, title, description, body, properties } = attempt.values;
  const { data } = await openProposal({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      title,
      description,
      changes: [{ kind: 'edit', item_id: itemId, body, properties }],
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  proposalsChanged(client, workspaceId, data.id);
  return data;
}

/** What an acceptance accepts: the exact proposal shown, on the head shown. */
export interface AcceptValues {
  proposalId: string;
  proposalRevision: Revision;
  contentDigest: string;
  expectedHead: Revision;
  itemIds: string[];
}

/** An acceptance done: the head change, and what the commit answered beside its revision. */
export interface Accepted extends HeadChange {
  warnings: Warning[];
  receiptId: ReceiptId;
}

/** Confirm, then accept; the proposals are asked again whatever the outcome. */
export async function accept(
  client: QueryClient,
  workspaceId: string,
  attempt: Confirmed<AcceptValues>,
): Promise<Accepted> {
  const { proposalId, proposalRevision, contentDigest, expectedHead, itemIds } = attempt.values;
  try {
    const { data: confirmation } = await createConfirmation({
      body: {
        workspace_id: workspaceId,
        action: 'accept_proposal',
        target: { kind: 'proposal', proposal_id: proposalId },
        revision: proposalRevision,
        content_digest: contentDigest,
        idempotency_key: attempt.confirmKey,
      },
      throwOnError: true,
    });
    const { data } = await acceptProposal({
      body: {
        workspace_id: workspaceId,
        proposal_id: proposalId,
        expected_head: expectedHead,
        proposal_revision: proposalRevision,
        confirmation_id: confirmation.id,
        idempotency_key: attempt.actionKey,
      },
      throwOnError: true,
    });
    const { previousHead, adopted } = adoptHeadFrom(
      client,
      workspaceId,
      data.revision,
      expectedHead,
    );
    checkHeadsAgain(client, workspaceId);
    return {
      revision: data.revision,
      previousHead,
      itemIds,
      adopted,
      warnings: data.warnings,
      receiptId: data.receipt_id,
    };
  } finally {
    // Accepted, refused, put in conflict: what the service now holds is asked again.
    proposalsChanged(client, workspaceId, proposalId);
  }
}

/** Decline, with the reason the service records as a comment. */
export async function decline(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<{ proposalId: string; reason: string }>,
): Promise<Proposal> {
  const { proposalId, reason } = attempt.values;
  try {
    const { data } = await declineProposal({
      body: {
        workspace_id: workspaceId,
        proposal_id: proposalId,
        reason,
        idempotency_key: attempt.key,
      },
      throwOnError: true,
    });
    client.setQueryData(proposalOptions(workspaceId, proposalId).queryKey, data);
    return data;
  } finally {
    proposalsChanged(client, workspaceId, proposalId);
  }
}

/** Add a comment to a proposal's discussion. */
export async function comment(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<{ proposalId: string; text: string }>,
): Promise<Comment> {
  const { proposalId, text } = attempt.values;
  const { data } = await addComment({
    body: {
      workspace_id: workspaceId,
      proposal_id: proposalId,
      text,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  proposalsChanged(client, workspaceId, proposalId);
  return data;
}
