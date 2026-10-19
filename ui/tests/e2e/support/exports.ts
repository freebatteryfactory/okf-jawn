/**
 * A static server for the export specs: the built `dist/`, as `useExplorerServer` serves it, and
 * beside it the archives a spec offers for download, each at its application-relative download
 * path on the same origin as the page.
 *
 * Why a server of its own: a browser fetches a link's target that carries `download` itself, as a
 * download, and `page.route` does not see that request in either engine (it reached the static
 * server, which answered the app's `index.html`). So an archive is served here, for real, with the
 * headers the service sends it with, and each request for it is counted.
 */

import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';

const dist = fileURLToPath(new URL('../../../dist', import.meta.url));

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.json': 'application/json',
  '.map': 'application/json',
};

/** An archive served at its download path. */
export interface Archive {
  body: Buffer;
  /** The name the service gives it (`Content-Disposition`). */
  filename: string;
}

export interface DownloadServer {
  origin: () => string;
  /** Requests for each download path since the test began. */
  downloads: (path: string) => number;
}

/**
 * Serve `dist/` and `archives` (by download path) for the specs of the calling describe block, at
 * the default 1440 x 900 viewport, with room for a cold browser start on a shared machine.
 */
export function useDownloadServer(archives: Record<string, Archive>): DownloadServer {
  let origin = '';
  let close: () => void = () => {};
  const asked = new Map<string, number>();
  test.describe.configure({ timeout: 90_000 });
  test.use({ viewport: { width: 1440, height: 900 } });
  test.beforeEach(() => asked.clear());
  test.beforeAll(async () => {
    const server = createServer(async (request, response) => {
      const path = decodeURIComponent(new URL(request.url ?? '/', 'http://local').pathname);
      const archive = archives[path];
      if (archive !== undefined) {
        asked.set(path, (asked.get(path) ?? 0) + 1);
        response.writeHead(200, {
          'content-type': 'application/zip',
          'content-length': String(archive.body.length),
          'content-disposition': `attachment; filename="${archive.filename}"`,
        });
        response.end(archive.body);
        return;
      }
      const file = extname(path) ? normalize(join(dist, path)) : join(dist, 'index.html');
      if (!file.startsWith(dist)) {
        response.writeHead(403).end();
        return;
      }
      try {
        const body = await readFile(file);
        response.writeHead(200, {
          'content-type': TYPES[extname(file)] ?? 'application/octet-stream',
        });
        response.end(body);
      } catch {
        response.writeHead(404).end('not found');
      }
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('server address unavailable');
    origin = `http://127.0.0.1:${address.port}`;
    close = () => server.close();
  });
  test.afterAll(() => close());
  return { origin: () => origin, downloads: (path) => asked.get(path) ?? 0 };
}
