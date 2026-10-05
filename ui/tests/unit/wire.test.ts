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
});
