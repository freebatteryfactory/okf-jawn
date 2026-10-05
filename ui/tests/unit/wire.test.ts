/** Wire-boundary omission of undefined optionals. */

import { describe, expect, it } from 'vitest';
import { omitUndefined } from '../../src/lib/wire';

describe('omitUndefined', () => {
  it('keeps present values including null', () => {
    const body = omitUndefined({
      workspace_id: 'ws_1',
      selection: null,
      max_bytes: 65536,
    });
    expect(body).toEqual({
      workspace_id: 'ws_1',
      selection: null,
      max_bytes: 65536,
    });
    expect(Object.hasOwn(body, 'selection')).toBe(true);
  });

  it('omits keys whose value is undefined (absent optional)', () => {
    const body = omitUndefined({
      workspace_id: 'ws_1',
      selection: undefined,
      max_bytes: 65536,
    });
    expect(body).toEqual({ workspace_id: 'ws_1', max_bytes: 65536 });
    expect(Object.hasOwn(body, 'selection')).toBe(false);
  });

  it('omits explicit undefined when the property is assigned', () => {
    const input: Record<string, unknown> = { path: 'a.md', view: 'text' };
    input.view = undefined;
    const body = omitUndefined(input);
    expect(body).toEqual({ path: 'a.md' });
    expect(Object.hasOwn(body, 'view')).toBe(false);
  });

  it('preserves empty string and zero', () => {
    const body = omitUndefined({ name: '', count: 0, flag: false });
    expect(body).toEqual({ name: '', count: 0, flag: false });
  });

  it('does not invent missing keys', () => {
    const body = omitUndefined({ workspace_id: 'ws_1' });
    expect(Object.keys(body)).toEqual(['workspace_id']);
    expect(Object.hasOwn(body, 'selection')).toBe(false);
    expect(Object.hasOwn(body, 'cursor')).toBe(false);
  });

  it('builds a generated-style request object field-by-field', () => {
    const selection: unknown = undefined;
    const cursor: unknown = null;
    const body = omitUndefined({
      workspace_id: '11111111-1111-4111-8111-111111111111',
      item_id: '22222222-2222-4222-8222-222222222222',
      at: { kind: 'latest' },
      view: 'text',
      selection,
      cursor,
      max_bytes: 4096,
      max_images: 0,
    });
    expect(body).toEqual({
      workspace_id: '11111111-1111-4111-8111-111111111111',
      item_id: '22222222-2222-4222-8222-222222222222',
      at: { kind: 'latest' },
      view: 'text',
      cursor: null,
      max_bytes: 4096,
      max_images: 0,
    });
    expect(Object.hasOwn(body, 'selection')).toBe(false);
  });
});
