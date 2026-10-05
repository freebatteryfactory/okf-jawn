/** SDK MIME constant used by MCP App resource bundles. */

import { RESOURCE_MIME_TYPE } from '@modelcontextprotocol/ext-apps';
import { describe, expect, it } from 'vitest';

describe('MCP Apps RESOURCE_MIME_TYPE', () => {
  it('exports the installed 2.0.3 UI resource MIME profile', () => {
    expect(RESOURCE_MIME_TYPE).toBe('text/html;profile=mcp-app');
  });

  it('is the value bundle-app.mjs must write into dist-apps/manifest.json', () => {
    expect(typeof RESOURCE_MIME_TYPE).toBe('string');
    expect(RESOURCE_MIME_TYPE.includes('mcp-app')).toBe(true);
    expect(RESOURCE_MIME_TYPE.startsWith('text/html')).toBe(true);
  });
});
