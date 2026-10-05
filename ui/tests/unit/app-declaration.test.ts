/** The bundle manifest comes from the generated declaration and fails on disagreement. */

import { RESOURCE_MIME_TYPE } from '@modelcontextprotocol/ext-apps';
import { describe, expect, it } from 'vitest';
import { manifestEntry, parseAppResource } from '../../scripts/app-declaration';

const resource = {
  uri: 'ui://okf-jawn/app.html',
  name: 'app',
  mimeType: RESOURCE_MIME_TYPE,
  _meta: { ui: { csp: { connectDomains: [], resourceDomains: [] } } },
};
const declaration = { resources: [resource] };

function withResource(change: Record<string, unknown>) {
  return { resources: [{ ...resource, ...change }] };
}

describe('parseAppResource', () => {
  it('returns the declared resource and builds the manifest entry from it', () => {
    const parsed = parseAppResource(declaration, 'mcp-apps.json', RESOURCE_MIME_TYPE);
    expect(manifestEntry(parsed, 12, 'ab'.repeat(32))).toEqual({
      ...resource,
      byteLength: 12,
      sha256: 'ab'.repeat(32),
    });
  });

  it('fails when the uri is not the file the build writes', () => {
    expect(() =>
      parseAppResource(
        withResource({ uri: 'ui://okf-jawn/other.html' }),
        'mcp-apps.json',
        RESOURCE_MIME_TYPE,
      ),
    ).toThrow(/uri ui:\/\/okf-jawn\/other\.html !== built resource ui:\/\/okf-jawn\/app\.html/);
  });

  it('fails when the mimeType is not the installed SDK profile', () => {
    expect(() =>
      parseAppResource(
        withResource({ mimeType: 'text/html' }),
        'mcp-apps.json',
        RESOURCE_MIME_TYPE,
      ),
    ).toThrow(/mimeType text\/html !== SDK/);
  });

  it('fails when csp is a string instead of the declared domain lists', () => {
    expect(() =>
      parseAppResource(
        withResource({ _meta: { ui: { csp: "default-src 'none'" } } }),
        'mcp-apps.json',
        RESOURCE_MIME_TYPE,
      ),
    ).toThrow(/not an MCP App declaration/);
  });

  it('fails when the resource has no name', () => {
    const { name: _name, ...unnamed } = resource;
    expect(() =>
      parseAppResource({ resources: [unnamed] }, 'mcp-apps.json', RESOURCE_MIME_TYPE),
    ).toThrow(/not an MCP App declaration/);
  });
});
