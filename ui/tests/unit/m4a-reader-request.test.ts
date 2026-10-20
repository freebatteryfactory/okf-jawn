/** A1: exact read identity, inclusive selections and budgets, from SPEC section 7 and the wire. */
import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';
import type { Selection } from '../../src/api/generated/types.gen';
import { zReadItemRequest } from '../../src/api/generated/zod.gen';
import { readKey, readOptions, readRequest } from '../../src/features/reader/request';
import { objectBytesKey } from '../../src/features/viewers/image/bytes';

const base = {
  workspaceId: '11111111-1111-4111-8111-111111111111',
  itemId: '22222222-2222-4222-8222-222222222222',
  revision: 'a'.repeat(40),
  view: 'text' as const,
  selection: { kind: 'all' } as Selection,
};

describe('read cache identity (A1, SPEC section 7)', () => {
  it.each([
    ['workspace', { workspaceId: '33333333-3333-4333-8333-333333333333' }],
    ['source', { itemId: '44444444-4444-4444-8444-444444444444' }],
    ['revision', { revision: 'b'.repeat(40) }],
    ['representation', { view: 'outline' as const }],
    ['selection', { selection: { kind: 'lines', range: { start: 2, end: 3 } } as Selection }],
  ])('isolates a changed %s at the same response budgets', (_name, change) => {
    const client = new QueryClient();
    const first = readRequest(base);
    const other = readRequest({ ...base, ...change });
    // A cached answer of the first request must never answer the other identity dimension.
    client.setQueryData(readKey(first), 'the first reading only');
    expect(other.max_bytes).toBe(65_536);
    expect(other.max_images).toBe(0);
    expect(client.getQueryData(readKey(other))).toBeUndefined();
    expect(client.getQueryData(readKey(readRequest({ ...base })))).toBe('the first reading only');
    client.clear();
  });

  it('separates original-byte authorization by workspace, source and revision, even for identical bytes', () => {
    const address = { ...base, digest: 'c'.repeat(64) };
    const key = objectBytesKey(address);
    for (const change of [
      { workspaceId: '33333333-3333-4333-8333-333333333333' },
      { itemId: '44444444-4444-4444-8444-444444444444' },
      { revision: 'b'.repeat(40) },
      { digest: 'd'.repeat(64) },
    ]) {
      expect(objectBytesKey({ ...address, ...change })).not.toEqual(key);
    }
  });
});

describe('bounded selected reading (A1)', () => {
  const selections: Selection[] = [
    { kind: 'section', heading: 'Budget' },
    { kind: 'pages', range: { start: 2, end: 4 } },
    { kind: 'lines', range: { start: 3, end: 7 } },
    {
      kind: 'cells',
      range: { sheet: 'Accounts', row_start: 2, row_end: 5, column_start: 1, column_end: 3 },
    },
  ];

  it.each(selections)('preserves the inclusive $kind selection and exact revision', (selection) => {
    const request = readRequest({ ...base, selection });
    expect(request).toEqual({
      workspace_id: base.workspaceId,
      item_id: base.itemId,
      at: { kind: 'revision', revision: base.revision },
      view: 'text',
      selection,
      cursor: null,
      max_bytes: 65_536,
      max_images: 0,
    });
    expect(zReadItemRequest.safeParse(request).success).toBe(true);
  });

  it('requests eight pictures/pages, never more than the contract sixteen, and no media in other views', () => {
    for (const view of ['multimodal', 'pages'] as const) {
      expect(readRequest({ ...base, view }).max_images).toBe(8);
      expect(readRequest({ ...base, view, maxImages: 25 }).max_images).toBe(16);
      expect(readRequest({ ...base, view, maxImages: -1 }).max_images).toBe(0);
    }
    for (const view of ['text', 'outline', 'original'] as const) {
      expect(readRequest({ ...base, view, maxImages: 16 }).max_images).toBe(0);
    }
  });

  it('starts from the full pinned request and continues only with the service cursor', () => {
    const request = readRequest({
      ...base,
      view: 'multimodal',
      selection: selections[1] ?? base.selection,
    });
    const options = readOptions(request);
    expect(options.initialPageParam).toEqual({ body: request });
    expect(options.getNextPageParam({ next_cursor: 'opaque-continuation' })).toBe(
      'opaque-continuation',
    );
    expect(options.getNextPageParam({ next_cursor: null })).toBeUndefined();
  });
});
