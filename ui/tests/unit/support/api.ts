/**
 * A mocked fetch for the generated client: each operation path answers from a handler.
 *
 * Every request body must survive a round trip through the operation's generated request
 * schema (so no field the contract lacks, and no flat guessed signature), and every successful
 * answer must parse with its generated response schema, so no test is driven by an answer the
 * server could not give. An operation without a handler answers `not_implemented`, as the real
 * service does for unbuilt operations. A handler may answer later (a promise), so a test can look
 * at the page while a request is in flight.
 */
import { vi } from 'vitest';
import type { ZodType } from 'zod';
import {
  zGetItemRequest,
  zGetSourcesRequest,
  zGetSourcesResponse,
  zItemDocument,
  zListItemsRequest,
  zListItemsResponse,
  zListWorkspacesRequest,
  zListWorkspacesResponse,
  zOpenWorkspaceRequest,
  zReadItemRequest,
  zReadItemResponse,
  zSearchRequest,
  zSearchResponse,
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

const CONTRACT: Record<string, { request: ZodType; response: ZodType }> = {
  '/api/workspaces/list-workspaces': {
    request: zListWorkspacesRequest,
    response: zListWorkspacesResponse,
  },
  '/api/workspaces/open-workspace': { request: zOpenWorkspaceRequest, response: zWorkspace },
  '/api/items/list-items': { request: zListItemsRequest, response: zListItemsResponse },
  '/api/items/get-item': { request: zGetItemRequest, response: zItemDocument },
  '/api/reads/read-item': { request: zReadItemRequest, response: zReadItemResponse },
  '/api/reads/get-sources': { request: zGetSourcesRequest, response: zGetSourcesResponse },
  '/api/search/search-items': { request: zSearchRequest, response: zSearchResponse },
};

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
    const reply = handler === undefined ? notImplemented(path) : await handler(body);
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
