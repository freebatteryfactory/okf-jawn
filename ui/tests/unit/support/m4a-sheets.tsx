/**
 * Ground for the spreadsheet, text and side-by-side viewers (UI M4a, ruling A2): a Source's
 * appearance as `get_item` answers it, the original's bytes answered through `download_object`
 * beside the mocked client, and a renderer of one viewer with the query cache and a router (the
 * failure notice's way out is a router link).
 *
 * The bytes are answered as the service answers a ranged read: 206 with exactly the asked part
 * (`Range: bytes=a-b`, inclusive), 200 with the whole object when no range is asked. Every other
 * request goes on to the mocked client (`support/api.ts`), so its contract checks still hold.
 */
import { QueryClientProvider } from '@tanstack/react-query';
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { render } from '@testing-library/react';
import type { ReactNode } from 'react';
import { vi } from 'vitest';
import type {
  Extraction,
  ReadItemResponse,
  Selection,
  SourceAppearance,
} from '../../../src/api/generated/types.gen';
import { createQueryClient } from '../../../src/app/query-client';
import type { OriginalProps } from '../../../src/features/viewers/types';
import { DIGEST, HEAD, OBJECT, RECEIPT, SOURCE, WS } from './workspace';

export const READ_ITEM = '/api/reads/read-item';
export const OBJECT_PATH = `/api/workspaces/${WS}/items/${SOURCE}/revisions/${HEAD}/objects/${OBJECT}`;

/** A converted source's extraction: the converter's text, nothing corrected. */
export function extractionOf(overrides: Partial<Extraction> = {}): Extraction {
  return {
    corrected: false,
    digest: DIGEST,
    outcome: { status: 'completed' },
    text_origin: 'converter',
    warnings: [],
    ...overrides,
  };
}

/** A Source's bytes as `get_item` describes them. */
export function appearanceOf(
  mediaType: string,
  size: number,
  extraction: Extraction = extractionOf(),
): SourceAppearance {
  return {
    extraction,
    media_type: mediaType,
    metadata: {},
    names: [],
    object: OBJECT,
    size: String(size),
  };
}

/** What a viewer of the Source `path` is given, at the head, reading the whole item. */
export function viewerProps(
  path: string,
  appearance: SourceAppearance,
  selection: Selection = { kind: 'all' },
): OriginalProps {
  return { workspaceId: WS, itemId: SOURCE, revision: HEAD, path, appearance, selection };
}

/** A `read_item` answer of the Source's text. */
export function sourceRead(
  path: string,
  markdown: string,
  extra: Partial<ReadItemResponse> = {},
  selection: Selection = { kind: 'all' },
): ReadItemResponse {
  return {
    markdown,
    media: [],
    outline: [],
    receipt_id: RECEIPT,
    source: {
      workspace_id: WS,
      item_id: SOURCE,
      path,
      revision: HEAD,
      digest: DIGEST,
      selection,
      locations: [],
    },
    truncated: false,
    view: 'text',
    warnings: [],
    extraction: { status: 'completed', text_origin: 'converter', corrected: false },
    ...extra,
  };
}

export interface ByteRead {
  /** The `Range` header asked, or null for the whole object. */
  range: string | null;
  method: string;
}

export interface Bytes {
  /** Every read of the object, in order. */
  reads: ByteRead[];
  /** Answer the reads from now on with this status and message instead of the bytes. */
  refuse: (status: number, code: string, message: string) => void;
  /** Answer the reads from now on with the bytes again. */
  answer: () => void;
}

/**
 * Answer `download_object` for the Source's original with `bytes`, after `mockApi` has stubbed
 * `fetch`: a read of the object goes here, every other request to the mocked client.
 */
export function answerBytes(bytes: Uint8Array<ArrayBuffer>, path: string = OBJECT_PATH): Bytes {
  const reads: ByteRead[] = [];
  let refusal: { status: number; code: string; message: string } | null = null;
  const client = globalThis.fetch;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      if (new URL(request.url).pathname !== path) return client(input, init);
      const range = request.headers.get('Range');
      reads.push({ range, method: request.method });
      if (refusal !== null) {
        return new Response(JSON.stringify({ code: refusal.code, message: refusal.message }), {
          status: refusal.status,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      const asked = /^bytes=(\d+)-(\d+)$/.exec(range ?? '');
      if (asked === null) {
        return new Response(bytes, {
          status: 200,
          headers: { 'Content-Type': 'application/octet-stream' },
        });
      }
      const first = Number(asked[1]);
      const last = Math.min(Number(asked[2]), bytes.length - 1);
      return new Response(bytes.slice(first, last + 1), {
        status: 206,
        headers: {
          'Content-Type': 'application/octet-stream',
          'Content-Range': `bytes ${first}-${last}/${bytes.length}`,
        },
      });
    }),
  );
  return {
    reads,
    refuse: (status, code, message) => {
      refusal = { status, code, message };
    },
    answer: () => {
      refusal = null;
    },
  };
}

/** UTF-8 bytes of a text. */
export const utf8 = (text: string): Uint8Array<ArrayBuffer> => new TextEncoder().encode(text);

/** Render one viewer with the app's query cache and a router around it. */
export function renderViewer(viewer: ReactNode) {
  const queryClient = createQueryClient();
  const router = createRouter({
    routeTree: createRootRoute({ component: () => viewer }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { queryClient, ...view };
}
