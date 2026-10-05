/** SDK MIME constant used by MCP App resource bundles. */

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { RESOURCE_MIME_TYPE } from '@modelcontextprotocol/ext-apps';
import { describe, expect, it } from 'vitest';

describe('MCP Apps RESOURCE_MIME_TYPE', () => {
  it('exports the installed UI resource MIME profile', () => {
    expect(RESOURCE_MIME_TYPE).toBe('text/html;profile=mcp-app');
  });

  it('matches the mimeType declared in api/mcp-apps.json', () => {
    const declarationPath = join(
      dirname(fileURLToPath(import.meta.url)),
      '../../../api/mcp-apps.json',
    );
    const declaration = JSON.parse(readFileSync(declarationPath, 'utf8')) as {
      resources?: Array<{ uri?: string; mimeType?: string }>;
    };
    expect(declaration.resources).toHaveLength(1);
    expect(declaration.resources?.[0]?.uri).toBe('ui://okf-jawn/app.html');
    expect(declaration.resources?.[0]?.mimeType).toBe(RESOURCE_MIME_TYPE);
  });
});
