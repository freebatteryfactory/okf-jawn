/** SDK MIME constant used by MCP App resource bundles. */

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { RESOURCE_MIME_TYPE } from '@modelcontextprotocol/ext-apps';
import { describe, expect, it } from 'vitest';

const manifestPath = join(
  dirname(fileURLToPath(import.meta.url)),
  '../../dist-apps/manifest.json',
);

describe('MCP Apps RESOURCE_MIME_TYPE', () => {
  it('exports the installed UI resource MIME profile', () => {
    expect(RESOURCE_MIME_TYPE).toBe('text/html;profile=mcp-app');
  });

  it('matches the mimeType written into dist-apps/manifest.json', () => {
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
      resources?: Array<{ mimeType?: string }>;
      mimeType?: string;
    };
    const mimeTypes = Array.isArray(manifest.resources)
      ? manifest.resources.map((entry) => entry.mimeType)
      : [manifest.mimeType];
    expect(mimeTypes.length).toBeGreaterThan(0);
    for (const mimeType of mimeTypes) {
      expect(mimeType).toBe(RESOURCE_MIME_TYPE);
    }
  });
});
