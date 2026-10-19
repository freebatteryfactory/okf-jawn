/**
 * A mocked fetch for the generated client: each operation path answers from a handler.
 *
 * Every request body must survive a round trip through the operation's generated request
 * schema (so no field the contract lacks, and no flat guessed signature), and every successful
 * answer must parse with its generated response schema, so no test is driven by an answer the
 * server could not give. An operation without a handler answers `not_implemented`, as the real
 * service does for unbuilt operations. A handler may answer later (a promise), so a test can look
 * at the page while a request is in flight. A request aborted meanwhile rejects, as `fetch` does:
 * a cancelled query's request, or one that outlived the client's bounded wait, does not go on.
 *
 * An answer that does not fit its schema is still answered as it is (a service newer than this
 * build can send one) and is recorded in `violations`: a test that means to send one says so
 * with `fromNewerService`, and every other test asserts that none was seen.
 */
import { vi } from 'vitest';
import type { ZodType } from 'zod';
import type { Draft, ItemSummary } from '../../../src/api/generated/types.gen';
import {
  zAcceptProposalRequest,
  zAddCommentRequest,
  zApplyNamesRequest,
  zBlameRequest,
  zBlameResponse,
  zComment,
  zCommitRequest,
  zConfirmation,
  zCreateConfirmationRequest,
  zCreatedItem,
  zCreateFolderRequest,
  zCreateItemRequest,
  zCreateReviewRequest,
  zDeclineProposalRequest,
  zDeleteItemRequest,
  zDiffRequest,
  zDiffResponse,
  zDiscardDraftRequest,
  zDraft,
  zEmpty,
  zGetGraphRequest,
  zGetGraphResponse,
  zGetItemRequest,
  zGetLinksRequest,
  zGetLinksResponse,
  zGetProposalRequest,
  zGetReceiptRequest,
  zGetRulesRequest,
  zGetRulesResponse,
  zGetSourcesRequest,
  zGetSourcesResponse,
  zItemDocument,
  zListCommentsRequest,
  zListCommentsResponse,
  zListDraftsRequest,
  zListDraftsResponse,
  zListItemsRequest,
  zListItemsResponse,
  zListProposalsRequest,
  zListProposalsResponse,
  zListReviewsRequest,
  zListReviewsResponse,
  zListTypesRequest,
  zListTypesResponse,
  zListWorkspacesRequest,
  zListWorkspacesResponse,
  zLogRequest,
  zLogResponse,
  zMoveItemRequest,
  zMutationResult,
  zOpenProposalRequest,
  zOpenWorkspaceRequest,
  zPreviewNamesRequest,
  zProposal,
  zReadItemRequest,
  zReadItemResponse,
  zReceipt,
  zRenamePlan,
  zRestoreRequest,
  zReview,
  zSaveDraftRequest,
  zSearchRequest,
  zSearchResponse,
  zSessionResponse,
  zSetLifecycleRequest,
  zSetRulesRequest,
  zWorkspace,
} from '../../../src/api/generated/zod.gen';

export type Json = Record<string, unknown>;
export interface Reply {
  status: number;
  body: unknown;
}
export type Handler = (body: Json) => Reply | Promise<Reply>;
export interface Call {
  path: string;
  body: Json;
  /** The request's headers, names in lower case. */
  headers: Record<string, string>;
}

/**
 * The request and response schema of each operation path a test may answer. Support modules of
 * other features add their operations to it (`imports.ts`).
 */
export const CONTRACT: Record<string, { request: ZodType; response: ZodType }> = {
  '/api/workspaces/list-workspaces': {
    request: zListWorkspacesRequest,
    response: zListWorkspacesResponse,
  },
  '/api/workspaces/open-workspace': { request: zOpenWorkspaceRequest, response: zWorkspace },
  '/api/items/list-items': { request: zListItemsRequest, response: zListItemsResponse },
  '/api/items/get-item': { request: zGetItemRequest, response: zItemDocument },
  '/api/items/save-draft': { request: zSaveDraftRequest, response: zDraft },
  '/api/items/list-drafts': { request: zListDraftsRequest, response: zListDraftsResponse },
  '/api/items/discard-draft': { request: zDiscardDraftRequest, response: zDraft },
  '/api/history/commit-items': { request: zCommitRequest, response: zMutationResult },
  '/api/items/create-item': { request: zCreateItemRequest, response: zCreatedItem },
  '/api/items/create-folder': { request: zCreateFolderRequest, response: zMutationResult },
  '/api/items/move-item': { request: zMoveItemRequest, response: zMutationResult },
  '/api/items/delete-item': { request: zDeleteItemRequest, response: zMutationResult },
  '/api/items/set-lifecycle': { request: zSetLifecycleRequest, response: zMutationResult },
  '/api/conventions/get-rules': { request: zGetRulesRequest, response: zGetRulesResponse },
  '/api/conventions/set-rules': { request: zSetRulesRequest, response: zMutationResult },
  '/api/conventions/preview-names': { request: zPreviewNamesRequest, response: zRenamePlan },
  '/api/conventions/apply-names': { request: zApplyNamesRequest, response: zMutationResult },
  '/api/types/list-types': { request: zListTypesRequest, response: zListTypesResponse },
  '/api/reads/read-item': { request: zReadItemRequest, response: zReadItemResponse },
  '/api/reads/get-sources': { request: zGetSourcesRequest, response: zGetSourcesResponse },
  '/api/search/search-items': { request: zSearchRequest, response: zSearchResponse },
  '/api/events/get-receipt': { request: zGetReceiptRequest, response: zReceipt },
  '/api/session/get-session': { request: zEmpty, response: zSessionResponse },
  // History, proposals and review (UI M3).
  '/api/history/log-items': { request: zLogRequest, response: zLogResponse },
  '/api/history/diff-items': { request: zDiffRequest, response: zDiffResponse },
  '/api/history/blame-item': { request: zBlameRequest, response: zBlameResponse },
  '/api/history/restore-items': { request: zRestoreRequest, response: zMutationResult },
  '/api/proposals/open-proposal': { request: zOpenProposalRequest, response: zProposal },
  '/api/proposals/list-proposals': {
    request: zListProposalsRequest,
    response: zListProposalsResponse,
  },
  '/api/proposals/get-proposal': { request: zGetProposalRequest, response: zProposal },
  '/api/proposals/accept-proposal': { request: zAcceptProposalRequest, response: zMutationResult },
  '/api/proposals/decline-proposal': { request: zDeclineProposalRequest, response: zProposal },
  '/api/proposals/add-comment': { request: zAddCommentRequest, response: zComment },
  '/api/proposals/list-comments': {
    request: zListCommentsRequest,
    response: zListCommentsResponse,
  },
  '/api/reviews/create-confirmation': {
    request: zCreateConfirmationRequest,
    response: zConfirmation,
  },
  '/api/reviews/create-review': { request: zCreateReviewRequest, response: zReview },
  '/api/reviews/list-reviews': { request: zListReviewsRequest, response: zListReviewsResponse },
  '/api/search/get-graph': { request: zGetGraphRequest, response: zGetGraphResponse },
  '/api/search/get-links': { request: zGetLinksRequest, response: zGetLinksResponse },
};

/**
 * A value of a newer service: one the generated types of this build do not name, typed as
 * whatever the place it is put in expects. With it a test answers a state, a kind or a status
 * this build does not know, as a service newer than the client would. The mock answers it as it
 * is and records the contract violation that it is, so such a test expects that violation
 * (`MockedApi.violations`) and no other.
 */
export function fromNewerService<T extends string>(value: string): T {
  return value as T;
}

/** What a listing finds of an item: its summary, or undefined when it is not there. */
export type ItemsFound = (itemId: string) => ItemSummary | undefined;

let found: ItemsFound = () => undefined;

/** Name the items a listing finds when a test does not say: `support/workspace.tsx` names its own. */
export function knowItems(items: ItemsFound) {
  found = items;
}

/**
 * A `list_drafts` answer: each draft a row with its item's summary as the listing read it
 * (`items`: by default the workspace's own items), without one for an item that is not there, at
 * a revision no test reads.
 */
export function draftsPage(
  drafts: readonly Draft[],
  nextCursor: string | null = null,
  items: ItemsFound = found,
): Json {
  return {
    revision: '0'.repeat(40),
    items: drafts.map((draft) => {
      const item = items(draft.item_id);
      return item === undefined ? { draft } : { draft, item };
    }),
    next_cursor: nextCursor,
  };
}

/** A `get_rules` answer holding `rules`, read at the revision the request asked for. */
export function rulesAnswer(rules: unknown, request: Json): Json {
  const at = request.at as { revision?: unknown } | undefined;
  const revision = typeof at?.revision === 'string' ? at.revision : '0'.repeat(40);
  return { revision, rules };
}

export const ok = (body: unknown): Reply => ({ status: 200, body });
export const fail = (status: number, code: string, message: string): Reply => ({
  status,
  body: { code, message },
});
export const notImplemented = (operation: string): Reply =>
  fail(501, 'not_implemented', `${operation} is not implemented in this build`);

function sorted(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sorted);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, entry]) => [key, sorted(entry)]),
    );
  }
  return value;
}

export interface MockedApi {
  calls: Call[];
  callsTo: (path: string) => Json[];
  /** Contract violations seen; a test asserts this stays empty. */
  violations: string[];
}

export function mockApi(handlers: Record<string, Handler>): MockedApi {
  const calls: Call[] = [];
  const violations: string[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init);
    const path = new URL(request.url).pathname;
    const text = await request.text();
    const parsed: unknown = text.length > 0 ? JSON.parse(text) : {};
    const body: Json =
      parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed)
        ? Object.fromEntries(Object.entries(parsed))
        : {};
    const headers: Record<string, string> = {};
    request.headers.forEach((value, name) => {
      headers[name.toLowerCase()] = value;
    });
    calls.push({ path, body, headers });
    const contract = CONTRACT[path];
    if (contract !== undefined) {
      const checked = contract.request.safeParse(body);
      if (!checked.success) violations.push(`${path} request: ${checked.error.message}`);
      else if (JSON.stringify(sorted(checked.data)) !== JSON.stringify(sorted(body))) {
        violations.push(`${path} request carries a field the contract does not have`);
      }
    }
    const handler = handlers[path];
    // Like `fetch`, a request that is aborted while it waits for its answer rejects.
    const signal = init?.signal ?? request.signal;
    const aborted = new Promise<never>((_resolve, reject) => {
      const abort = () => reject(new DOMException('The request was aborted', 'AbortError'));
      if (signal.aborted) abort();
      else signal.addEventListener('abort', abort, { once: true });
    });
    // An abort that comes once the answer is given (or for an operation with no handler, which
    // is answered at once) changes nothing, as with `fetch`: it is not left as a rejection
    // nobody handles.
    aborted.catch(() => undefined);
    const reply =
      handler === undefined ? notImplemented(path) : await Promise.race([handler(body), aborted]);
    if (reply.status === 200 && contract !== undefined) {
      const checked = contract.response.safeParse(reply.body);
      if (!checked.success) violations.push(`${path} response: ${checked.error.message}`);
    }
    return new Response(JSON.stringify(reply.body), {
      status: reply.status,
      headers: { 'Content-Type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return {
    calls,
    violations,
    callsTo: (path) => calls.filter((call) => call.path === path).map((call) => call.body),
  };
}
