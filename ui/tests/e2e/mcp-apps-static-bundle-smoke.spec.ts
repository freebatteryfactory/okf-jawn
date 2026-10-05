/** Static-server smoke: bundled HTML paints without a real MCP host (not a host-render check). */

import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';

const distApps = join(fileURLToPath(new URL('../../dist-apps', import.meta.url)));
const names = ['source', 'changes', 'timeline', 'present'] as const;

async function startStaticServer() {
  const server = createServer(async (request, response) => {
    const url = request.url ?? '/';
    const name = url.replace(/^\//, '').replace(/\.html$/, '');
    if (!names.includes(name as (typeof names)[number])) {
      response.writeHead(404);
      response.end('not found');
      return;
    }
    const body = await readFile(join(distApps, `${name}.html`));
    response.writeHead(200, {
      'content-type': 'text/html; charset=utf-8',
      'content-length': body.byteLength,
    });
    response.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('server address unavailable');
  return { server, port: address.port };
}

test.describe('MCP App static bundle smoke', () => {
  test('each resource paints waiting/failure status without a host', async ({ page }, testInfo) => {
    const { server, port } = await startStaticServer();
    try {
      for (const name of names) {
        await page.goto(`http://127.0.0.1:${port}/${name}.html`, {
          waitUntil: 'domcontentloaded',
        });
        const root = page.locator('#root');
        await expect(root).not.toBeEmpty({ timeout: 15_000 });
        // Without a real host, useApp surfaces connection failure or waiting status —
        // either proves the bundled App executed and painted.
        const status = page.getByRole('alert').or(page.getByRole('status'));
        await expect(status.first()).toBeVisible({ timeout: 15_000 });
        const text = await status.first().innerText();
        expect(text.length).toBeGreaterThan(0);
        const axe = await new AxeBuilder({ page }).analyze();
        const serious = axe.violations.filter(
          (violation) => violation.impact === 'serious' || violation.impact === 'critical',
        );
        expect(serious, `${name}: ${JSON.stringify(serious)}`).toEqual([]);
        await page.screenshot({
          path: join(
            fileURLToPath(new URL('../../../.artifacts/qualification/mcp-apps', import.meta.url)),
            `render-${name}.png`,
          ),
          fullPage: true,
        });
        testInfo.annotations.push({
          type: 'bundle-render',
          description: `${name}: painted "${text.slice(0, 120)}"`,
        });
      }
    } finally {
      server.close();
    }
  });
});
