/**
 * Ground for the journeys of the PDF viewer and of an Office file's original (UI M4a, ruling A2).
 *
 * - `buildPdf`: a real, small PDF written here at run time (no binary file is committed): its
 *   pages draw their text in Helvetica, which the PDF does not embed, so PDF.js asks for a
 *   standard font; a page may also write in a CID font with a predefined CMap (`UniGB-UCS2-H`),
 *   so PDF.js asks for that CMap. `padding` puts an unread stream in the middle of the file, to
 *   make a large PDF whose pages are all near its start.
 * - `useOriginalServer`: the built `dist/` served as the service serves it, every file with the
 *   page headers of `crates/server/src/assets.rs` (`PAGE_POLICY`), so the page and PDF.js's worker
 *   run under the real page policy; beside it the originals a journey offers for download, each
 *   at its `download_object` path with the headers `crates/server/src/transfers.rs` gives a
 *   download. Every path asked of it is recorded: what PDF.js fetched from the page's own origin.
 *   (A link that carries `download` is fetched by the browser as a download, which `page.route`
 *   does not see, as `support/exports.ts` found.)
 * - `routeSource`: the service's answers for one source (`routeApi` of `explorer.ts`, with
 *   `get_item` answering its appearance), and its bytes through `download_object` answered by
 *   `page.route`, a `Range` with 206 and that part, each read recorded.
 * - `watchPolicy`: every report of the page policy refusing something, from the page's console
 *   and its `securitypolicyviolation` events, and every page error.
 *
 * The specs need `bun --bun run build` first: they serve the built `dist/` themselves.
 */

import { readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, type Locator, type Page, test, type Worker } from '@playwright/test';
import { type Answers, HEAD, ok, readAnswer, routeApi, SOURCE, summary, WS } from './explorer';

const dist = fileURLToPath(new URL('../../../dist', import.meta.url));
const pdfjsPackage = fileURLToPath(new URL('../../../node_modules/pdfjs-dist/package.json', import.meta.url));

/** The installed PDF.js version, and where this build serves its files. */
export const PDFJS_VERSION: string = (
  JSON.parse(readFileSync(pdfjsPackage, 'utf8')) as { version: string }
).version;
export const PDFJS_FILES = `/pdfjs-${PDFJS_VERSION}/`;

export const OBJECT = 'd'.repeat(64);
export const OBJECT_PATH = `/api/workspaces/${WS}/items/${SOURCE}/revisions/${HEAD}/objects/${OBJECT}`;

/** The page policy the service sends with every page and file, with no sandbox origin configured. */
export const PAGE_POLICY =
  "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; " +
  "img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; " +
  "worker-src 'self'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; " +
  "form-action 'self'; object-src 'none'";

const PAGE_HEADERS = {
  'content-security-policy': PAGE_POLICY,
  'x-content-type-options': 'nosniff',
  'referrer-policy': 'no-referrer',
  'x-frame-options': 'DENY',
  'cross-origin-opener-policy': 'same-origin',
};

/** What a download carries, whatever was stored (`transfers.rs` `send`). */
const DOWNLOAD_HEADERS = {
  'content-disposition': 'attachment',
  'content-security-policy': "sandbox; default-src 'none'",
  'x-content-type-options': 'nosniff',
  'accept-ranges': 'bytes',
};

// ---------------------------------------------------------------------------------------------
// A PDF written at run time.

export interface PdfPageSpec {
  /** Lines of text drawn in Helvetica (not embedded). */
  lines: string[];
  /** A line drawn in a CID font through the predefined CMap `UniGB-UCS2-H`. */
  cjk?: string;
}

/** A string as the UTF-16BE hex string `UniGB-UCS2-H` reads. */
function ucs2(text: string): string {
  return [...text]
    .map((character) => (character.codePointAt(0) ?? 0).toString(16).padStart(4, '0'))
    .join('');
}

/** `(text)` as a PDF literal string. */
function literal(text: string): string {
  return `(${text.replace(/[\\()]/g, (character) => `\\${character}`)})`;
}

/**
 * A PDF of `pages`, US Letter, written object by object with its cross-reference table. With
 * `padding`, an unreferenced stream of that many bytes comes after the pages.
 */
export function buildPdf(pages: PdfPageSpec[], padding = 0): Buffer {
  const objects: (string | Buffer)[] = [];
  const add = (body: string | Buffer) => {
    objects.push(body);
    return objects.length;
  };
  const catalog = add('');
  const tree = add('');
  const helvetica = add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>');
  const descriptor = add(
    '<< /Type /FontDescriptor /FontName /STSong-Light /Flags 6 /FontBBox [-25 -254 1000 880] ' +
      '/ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >>',
  );
  const cid = add(
    '<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light ' +
      `/CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /FontDescriptor ${descriptor} 0 R >>`,
  );
  const cjkFont = add(
    `<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [${cid} 0 R] >>`,
  );
  const kids: number[] = [];
  for (const spec of pages) {
    const drawn = spec.lines
      .map((line, index) => `BT /F1 ${index === 0 ? 28 : 16} Tf 72 ${700 - index * 40} Td ${literal(line)} Tj ET`)
      .concat(spec.cjk === undefined ? [] : [`BT /F2 20 Tf 72 300 Td <${ucs2(spec.cjk)}> Tj ET`])
      .join('\n');
    const content = add(`<< /Length ${Buffer.byteLength(drawn, 'latin1')} >>\nstream\n${drawn}\nendstream`);
    kids.push(
      add(
        `<< /Type /Page /Parent ${tree} 0 R /MediaBox [0 0 612 792] ` +
          `/Resources << /Font << /F1 ${helvetica} 0 R /F2 ${cjkFont} 0 R >> >> /Contents ${content} 0 R >>`,
      ),
    );
  }
  objects[catalog - 1] = `<< /Type /Catalog /Pages ${tree} 0 R >>`;
  objects[tree - 1] =
    `<< /Type /Pages /Kids [${kids.map((kid) => `${kid} 0 R`).join(' ')}] /Count ${kids.length} >>`;
  if (padding > 0) {
    add(
      Buffer.concat([
        Buffer.from(`<< /Length ${padding} >>\nstream\n`, 'latin1'),
        Buffer.alloc(padding, 0x20),
        Buffer.from('\nendstream', 'latin1'),
      ]),
    );
  }
  const parts: Buffer[] = [Buffer.from('%PDF-1.7\n%\xe2\xe3\xcf\xd3\n', 'latin1')];
  let offset = parts[0]?.length ?? 0;
  const offsets: number[] = [];
  objects.forEach((body, index) => {
    offsets.push(offset);
    const object = Buffer.concat([
      Buffer.from(`${index + 1} 0 obj\n`, 'latin1'),
      typeof body === 'string' ? Buffer.from(body, 'latin1') : body,
      Buffer.from('\nendobj\n', 'latin1'),
    ]);
    parts.push(object);
    offset += object.length;
  });
  const xref = [
    'xref',
    `0 ${objects.length + 1}`,
    '0000000000 65535 f ',
    ...offsets.map((at) => `${String(at).padStart(10, '0')} 00000 n `),
    'trailer',
    `<< /Size ${objects.length + 1} /Root ${catalog} 0 R >>`,
    'startxref',
    String(offset),
    '%%EOF',
    '',
  ].join('\n');
  parts.push(Buffer.from(xref, 'latin1'));
  return Buffer.concat(parts);
}

/** The PDF of the journeys: three pages, the third also in a CID font. */
export const REPORT = buildPdf([
  { lines: ['Annual report 2025', 'Page one: the year in figures.'] },
  { lines: ['Revenue', 'Page two: revenue grew by 12 percent.'] },
  { lines: ['Outlook', 'Page three: the plan for 2026.'], cjk: 'Page three' },
]);

// ---------------------------------------------------------------------------------------------
// The service's static files, as the service serves them.

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.json': 'application/json',
  '.map': 'application/json',
};

/** An original offered for download, at its `download_object` path. */
export interface Original {
  body: Buffer;
  /** The type the service sends it as: `application/octet-stream` unless it is inert. */
  contentType: string;
}

export interface OriginalServer {
  origin: () => string;
  /** Every path asked since the test began, in order. */
  asked: () => string[];
  /** Requests for one path since the test began. */
  count: (path: string) => number;
}

/**
 * Serve `dist/` with the page headers, and `originals` at their paths as downloads, for the specs
 * of the calling describe block, at the default 1440 x 900 viewport, with room for a cold browser
 * start on a shared machine.
 */
export function useOriginalServer(originals: Record<string, Original> = {}): OriginalServer {
  let origin = '';
  let close: () => void = () => {};
  let asked: string[] = [];
  test.describe.configure({ timeout: 90_000 });
  test.use({ viewport: { width: 1440, height: 900 } });
  test.beforeEach(() => {
    asked = [];
  });
  test.beforeAll(async () => {
    const server = createServer(async (request, response) => {
      const path = decodeURIComponent(new URL(request.url ?? '/', 'http://local').pathname);
      asked.push(path);
      const original = originals[path];
      if (original !== undefined) {
        response.writeHead(200, {
          ...DOWNLOAD_HEADERS,
          'content-type': original.contentType,
          'content-length': String(original.body.length),
        });
        response.end(original.body);
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
          ...PAGE_HEADERS,
          'content-type': TYPES[extname(file)] ?? 'application/octet-stream',
        });
        response.end(body);
      } catch {
        response.writeHead(404, PAGE_HEADERS).end('not found');
      }
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()));
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('server address unavailable');
    origin = `http://127.0.0.1:${address.port}`;
    close = () => server.close();
  });
  test.afterAll(() => close());
  return {
    origin: () => origin,
    asked: () => [...asked],
    count: (path) => asked.filter((seen) => seen === path).length,
  };
}

// ---------------------------------------------------------------------------------------------
// One source, its answers and its bytes.

export interface SourceOptions {
  mediaType: string;
  bytes: Buffer;
  /** The file the original was supplied as. */
  filename: string;
  /** How its text stands; converted by default. */
  extraction?: Record<string, unknown>;
  /** More answers, over the Explorer's. */
  extra?: Answers;
}

/** A byte read the page made, as `page.route` saw it. */
export interface ByteRead {
  range: string | null;
  /** Bytes answered. */
  length: number;
}

/** The source the journeys open, as its listing row shows it. */
export function sourceRow(mediaType: string) {
  return {
    ...summary(SOURCE, 'reports/annual-report.md', 'Annual report 2025', 'report', 'source'),
    extraction: { status: 'completed', text_origin: 'converter', corrected: false },
    media_type: mediaType,
  };
}

/** Answer the Explorer with one source of `mediaType`, and its bytes; returns the byte reads. */
export async function routeSource(page: Page, options: SourceOptions): Promise<ByteRead[]> {
  const row = sourceRow(options.mediaType);
  const appearance = {
    extraction: {
      corrected: false,
      outcome: { status: 'completed' },
      text_origin: 'converter',
      warnings: [],
      ...options.extraction,
    },
    media_type: options.mediaType,
    metadata: {},
    names: [
      {
        filename: options.filename,
        folder: 'reports',
        observed_at: '2026-01-01T09:00:00.000Z',
        supplied_by: 'ada',
      },
    ],
    object: OBJECT,
    size: String(options.bytes.length),
  };
  await routeApi(page, {
    '/api/items/list-items': () =>
      ok({ folders: ['notes'], items: [row], revision: HEAD, next_cursor: null }),
    '/api/items/get-item': () =>
      ok({
        summary: row,
        body: '# Annual report 2025\n\nThe year in figures.',
        content_digest: 'c'.repeat(64),
        properties: { type: 'report', okf_jawn: { id: SOURCE } },
        source: appearance,
      }),
    // The converted text opens with no heading of its own: with one, the reader's "Original" h3
    // (DocumentView.tsx, the reader's file) follows the text's h1, and axe reports heading-order
    // there. That is the reader's to settle (requested in the report), not this viewer's.
    '/api/reads/read-item': (body) =>
      ok(
        readAnswer(row, body, {
          markdown: 'The year in figures, as converted from the original.\n\nRevenue grew by 12 percent.',
        }),
      ),
    ...options.extra,
  });
  const reads: ByteRead[] = [];
  // Registered after `routeApi`, so it answers the object's path first.
  await page.route(`**${OBJECT_PATH}`, async (route) => {
    const range = route.request().headers().range ?? null;
    const asked = /^bytes=(\d+)-(\d+)$/.exec(range ?? '');
    const total = options.bytes.length;
    const start = asked === null ? 0 : Number(asked[1]);
    const end = asked === null ? total - 1 : Math.min(Number(asked[2]), total - 1);
    const body = options.bytes.subarray(start, end + 1);
    reads.push({ range, length: body.length });
    await route.fulfill({
      status: asked === null ? 200 : 206,
      headers: {
        ...DOWNLOAD_HEADERS,
        'content-type': options.mediaType === 'application/pdf' ? 'application/pdf' : 'application/octet-stream',
        ...(asked === null ? {} : { 'content-range': `bytes ${start}-${end}/${total}` }),
      },
      body,
    });
  });
  return reads;
}

/** The reader's address for the source, with a selection when given. */
export function sourceAddress(origin: string, selection?: unknown): string {
  const search =
    selection === undefined ? '' : `?sel=${encodeURIComponent(JSON.stringify(selection))}`;
  return `${origin}/w/${WS}/i/${SOURCE}${search}`;
}

// ---------------------------------------------------------------------------------------------
// The page policy, the workers.

export interface PolicyWatch {
  /** Everything the page policy refused, and every page error, as reported. */
  refusals: () => Promise<string[]>;
  /** PDF.js's own warnings on the console, for the report. */
  warnings: () => string[];
}

/** Watch the page for refusals of its policy (console and `securitypolicyviolation`) and errors. */
export async function watchPolicy(page: Page): Promise<PolicyWatch> {
  const seen: string[] = [];
  const warnings: string[] = [];
  page.on('console', (message) => {
    const text = message.text();
    if (/Content[- ]Security[- ]Policy|Refused to /i.test(text)) seen.push(`console: ${text}`);
    else if (/^Warning:/.test(text)) warnings.push(text);
  });
  page.on('pageerror', (error) => seen.push(`page error: ${error.message}`));
  await page.addInitScript(() => {
    const found: string[] = [];
    Object.assign(window, { okfPolicyRefusals: found });
    document.addEventListener('securitypolicyviolation', (event) => {
      found.push(
        `${event.violatedDirective} refused ${event.blockedURI || "(inline)"} at ${event.sourceFile}:${event.lineNumber}:${event.columnNumber}`,
      );
    });
  });
  return {
    refusals: async () => {
      const fromPage = await page.evaluate(() => {
        const found = (window as { okfPolicyRefusals?: unknown }).okfPolicyRefusals;
        return Array.isArray(found) ? found.map(String) : [];
      });
      return [...seen, ...fromPage];
    },
    warnings: () => [...warnings],
  };
}

/** The dedicated workers the page starts, and whether each has ended. */
export function watchWorkers(page: Page): { started: () => string[]; ended: () => string[] } {
  const started: string[] = [];
  const ended: string[] = [];
  page.on('worker', (worker: Worker) => {
    started.push(worker.url());
    worker.on('close', () => ended.push(worker.url()));
  });
  return { started: () => [...started], ended: () => [...ended] };
}

/** Whether a canvas has anything dark drawn on it: text in black on the white page. */
export async function inked(canvas: Locator): Promise<boolean> {
  return canvas.evaluate((element) => {
    if (!(element instanceof HTMLCanvasElement)) return false;
    const context = element.getContext('2d');
    if (context === null) return false;
    const { data } = context.getImageData(0, 0, element.width, element.height);
    for (let index = 0; index < data.length; index += 4) {
      const value = (data[index] ?? 255) + (data[index + 1] ?? 255) + (data[index + 2] ?? 255);
      if (value < 200 && (data[index + 3] ?? 0) > 0) return true;
    }
    return false;
  });
}

/** The drawn page, by its text alternative, once its text layer is laid. */
export function pageImage(region: Locator, pageNumber: number, of = 3, name = 'annual-report.pdf') {
  return region.getByRole('img', {
    name: `Page ${pageNumber} of ${of} of ${name}. Its text is laid over the drawing, to select and to read aloud.`,
  });
}

/** Wait until page `pageNumber` is drawn: its text alternative, its status, and ink. */
export async function expectDrawn(region: Locator, pageNumber: number, of = 3) {
  const image = pageImage(region, pageNumber, of);
  await expect(image).toBeVisible();
  await expect(region.getByRole('status')).toHaveText(`Page ${pageNumber} of ${of}`);
  await expect.poll(() => inked(image)).toBe(true);
}
