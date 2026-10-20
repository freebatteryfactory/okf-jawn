/**
 * The built UI under the server's page policy (UI M4a, ruling A5), in Chromium and WebKit.
 *
 * Every response of the built UI is served with the headers the server sends with its pages
 * (`crates/server/src/assets.rs` `page_headers`, through `support/m4a-sandbox.ts`
 * `underPagePolicy`): `Content-Security-Policy` (`script-src 'self'`, `style-src 'self'
 * 'unsafe-inline'`, `img-src 'self' data: blob:`, `font-src 'self' data:`, `connect-src 'self'`,
 * `worker-src 'self'`, `frame-src` the sandbox origin, …), `X-Content-Type-Options`,
 * `Referrer-Policy`, `X-Frame-Options` and `Cross-Origin-Opener-Policy`. A first check proves the
 * policy is in force in each engine (an inline script is refused and reported). Then the screens of
 * the existing journeys are visited: the workspace list, the Explorer, a document, a note being
 * edited, the Jobs page, Settings, both Administration pages, a saved View, and an original of
 * every kind the reader shows (PDF, image, HTML in the sandbox, spreadsheet, Office, text and
 * code, other), as the viewers exist in this build.
 *
 * On each screen the spec collects, from before the page's first script (`watchPolicy`):
 * - every Content-Security-Policy violation: each `securitypolicyviolation` event of the page, and
 *   each console line in which the browser refuses something under the policy;
 * - every `<style>` element a script puts in the page;
 * - every `blob:` or `data:` address the page makes or uses (object URLs made; addresses in
 *   attributes, computed styles and stylesheets), and every worker it starts.
 * The sandboxed frame's own refusals (its scripts, its pictures from elsewhere) are the sandbox
 * working (`m4a-sandbox.spec.ts`), under the sandbox origin's own policy, and are not the page's.
 *
 * The spec fails on anything it does not list below as known: a violation must be listed with the
 * request made for it (`KNOWN_VIOLATIONS`), and a `<style>` inserter or a `blob:`/`data:` use with
 * the library and the directive that allows it (`KNOWN_STYLES`, `KNOWN_URLS`), so a library added
 * later that needs more of the policy is reported, not passed over. What each screen did is
 * written to `.artifacts/ui/m4a-page-policy/` (untracked) for the report.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself. The API, the bytes
 * (`download_object`, as `transfers.rs` sends them) and the sandbox origin are played through
 * `page.route`.
 */

import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, type Page, test } from '@playwright/test';
import { adminWorkspace, localOwner, tenantLog, workspacePage } from './support/admin-security';
import {
  type Answers,
  NOTE,
  ok,
  routeApi,
  useExplorerServer,
  WS,
  writableWorkspace,
} from './support/explorer';
import {
  ORIGINALS,
  type Original,
  originalsAnswers,
  type PolicyFindings,
  pagePolicy,
  playSandbox,
  policyFindings,
  routeObjects,
  SANDBOX_ORIGIN,
  underPagePolicy,
  watchPolicy,
} from './support/m4a-sandbox';
import { openOriginal } from './support/reader';
import { VIEW, viewAnswers } from './support/views';

const findingsDir = fileURLToPath(
  new URL('../../../.artifacts/ui/m4a-page-policy', import.meta.url),
);

/** A violation the spec knows of, with the request made for it. */
interface KnownViolation {
  directive: RegExp;
  blocked: RegExp;
  /** Who needs it, and the request: file:line or package, and the change proposed. */
  request: string;
}

/** A `<style>` inserter the spec knows of: the library, and why the policy allows it. */
interface KnownStyle {
  text: RegExp;
  attributes: RegExp;
  library: string;
}

/** A `blob:` or `data:` use the spec knows of: the library, and the directive that allows it. */
interface KnownUrl {
  url: RegExp;
  where: RegExp;
  library: string;
}

/** No violation is known in this build. */
const KNOWN_VIOLATIONS: KnownViolation[] = [];

/** Installed-library styles allowed by the server's existing style-src 'unsafe-inline'. */
const KNOWN_STYLES: KnownStyle[] = [
  {
    text: /^\.\u037c[a-z0-9]+\.cm-focused \{outline:/,
    attributes: /^$/,
    library: '@codemirror/view 6.41.0 through style-mod 4.1.4 (StyleSet.mount)',
  },
  {
    text: /^\.vega-embed \{/,
    attributes: /^id=vega-embed-style$/,
    library: 'vega-embed 7.3.0, embed.js defaultStyle',
  },
  {
    text: /^#vg-tooltip-element \{/,
    attributes: /^id=vega-tooltip-style$/,
    library: 'vega-tooltip 1.1.0 (vega-embed tooltip Handler)',
  },
];

/** CodeMirror draws its visible-tab marker with a data SVG; img-src already allows data:. */
const KNOWN_URLS: KnownUrl[] = [
  {
    url: /^data:image\/svg\+xml,/,
    where: /^stylesheet <style>: .*\.cm-highlightTab /,
    library: '@codemirror/view 6.41.0 baseTheme .cm-highlightTab; img-src data:',
  },
];

/** Console lines in which a browser refuses something under a Content-Security-Policy. */
const REFUSED = /Content Security Policy|Content-Security-Policy/i;

/** One screen of an existing journey: where it is, what answers it, and when it has drawn. */
interface Screen {
  name: string;
  path: string;
  answers: Answers;
  /** The screen has drawn what the journey checks. */
  ready: (page: Page) => Promise<void>;
}

const SCREENS: Screen[] = [
  {
    name: 'the workspace list',
    path: '/',
    answers: {},
    ready: async (page) => {
      await expect(page.getByRole('link', { name: /Workshop notes/ }).first()).toBeVisible();
    },
  },
  {
    name: 'the Explorer',
    path: `/w/${WS}`,
    answers: {},
    ready: async (page) => {
      await expect(page.getByRole('treeitem', { name: /Annual report 2025/ })).toBeVisible();
    },
  },
  {
    name: 'a document',
    path: `/w/${WS}/i/${NOTE}`,
    answers: {},
    ready: async (page) => {
      await expect(page.getByRole('article', { name: 'Meeting notes' })).toContainText(
        'publish the budget',
      );
    },
  },
  {
    name: 'a note being edited',
    path: `/w/${WS}/i/${NOTE}`,
    answers: { '/api/workspaces/open-workspace': () => ok(writableWorkspace) },
    ready: async (page) => {
      const article = page.getByRole('article', { name: 'Meeting notes' });
      await article.getByRole('button', { name: 'Edit', exact: true }).click();
      await expect(article.getByRole('textbox').first()).toBeVisible();
    },
  },
  {
    name: 'the Jobs page',
    path: `/w/${WS}/jobs`,
    answers: { '/api/imports/list-jobs': () => ok({ items: [], next_cursor: null }) },
    ready: async (page) => {
      await expect(page.getByText(/This workspace has no jobs yet\./)).toBeVisible();
    },
  },
  {
    name: 'Settings',
    path: `/w/${WS}/settings`,
    answers: {},
    ready: async (page) => {
      await expect(page.getByRole('heading', { name: /Settings/ }).first()).toBeVisible();
    },
  },
  {
    name: "the workspace's Administration",
    path: `/w/${WS}/admin`,
    answers: {
      '/api/workspaces/open-workspace': () => ok(adminWorkspace),
      '/api/session/get-session': () => ok(localOwner),
      '/api/session/list-connectors': () => ok({ connectors: [] }),
      '/api/events/list-events': () => ok(workspacePage),
    },
    ready: async (page) => {
      await expect(page.getByRole('region', { name: 'Security events' })).toBeVisible();
    },
  },
  {
    name: "the installation's Administration",
    path: '/admin',
    answers: { '/api/session/get-session': () => ok(localOwner), ...tenantLog([]) },
    ready: async (page) => {
      await expect(
        page.getByRole('region', { name: 'Security events of the installation' }),
      ).toBeVisible();
    },
  },
  {
    name: 'a saved View',
    path: `/w/${WS}/i/${VIEW}`,
    answers: viewAnswers(),
    ready: async (page) => {
      const view = page.getByRole('region', { name: 'The View' });
      await expect(view.locator('.vega-embed canvas, .vega-embed svg').first()).toBeVisible();
    },
  },
  ...ORIGINALS.map(
    (original): Screen => ({
      name: `an original: ${original.kind} (${original.mediaType})`,
      path: `/w/${WS}/i/${original.id}`,
      answers: originalsAnswers(),
      ready: async (page) => originalShown(page, original),
    }),
  ),
];

/** The reader shows the original in its viewer, as far as this build's viewer goes. */
async function originalShown(page: Page, original: Original) {
  const region = await openOriginal(page);
  await expect(region).toBeVisible();
  if (original.kind === 'html') {
    await expect(
      page
        .frameLocator('iframe[sandbox]')
        .getByRole('heading', { name: 'Welcome to the workshop' }),
    ).toBeVisible();
  }
}

/** Every console line of the page's own documents in which the browser refuses under a policy. */
function watchConsole(page: Page): string[] {
  const lines: string[] = [];
  page.on('console', (message) => {
    const from = message.location().url;
    // The sandboxed frame's own refusals are the sandbox working, under its own policy.
    if (from.startsWith(SANDBOX_ORIGIN)) return;
    if (REFUSED.test(message.text())) lines.push(message.text());
  });
  return lines;
}

/** What is not on the known lists, in words the failure shows. */
function unlisted(found: PolicyFindings, console: string[]): string[] {
  const left: string[] = [];
  for (const violation of found.violations) {
    const known = KNOWN_VIOLATIONS.some(
      (entry) => entry.directive.test(violation.directive) && entry.blocked.test(violation.blocked),
    );
    if (!known) left.push(`violation: ${JSON.stringify(violation)}`);
  }
  for (const line of console) {
    const known = KNOWN_VIOLATIONS.some(
      (entry) => entry.directive.test(line) && entry.blocked.test(line),
    );
    if (!known) left.push(`console: ${line}`);
  }
  for (const style of found.styles) {
    if (
      !KNOWN_STYLES.some(
        (entry) => entry.text.test(style.text) && entry.attributes.test(style.attributes),
      )
    ) {
      left.push(`<style> inserted: ${JSON.stringify(style)}`);
    }
  }
  for (const use of found.urls) {
    if (!KNOWN_URLS.some((entry) => entry.url.test(use.url) && entry.where.test(use.where))) {
      left.push(`${use.url} used: ${use.where}`);
    }
  }
  for (const worker of found.workers) {
    if (/^(blob:|data:)/i.test(worker)) left.push(`worker from ${worker}`);
  }
  return left;
}

test.describe('the built UI under the page policy', () => {
  const server = useExplorerServer();

  test('the policy is in force: an inline script is refused and reported', async ({ page }) => {
    await watchPolicy(page);
    await underPagePolicy(page, server.origin());
    await routeApi(page);
    const response = await page.goto(`${server.origin()}/`);
    expect(response?.headers()).toMatchObject(pagePolicy(SANDBOX_ORIGIN));
    await expect(page.getByRole('link', { name: /Workshop notes/ }).first()).toBeVisible();
    const ran = await page.evaluate(async () => {
      const script = document.createElement('script');
      script.textContent = 'window.__okfInlineRan = true';
      const reported = new Promise<string>((resolve) => {
        document.addEventListener('securitypolicyviolation', (event) =>
          resolve(event.effectiveDirective),
        );
        setTimeout(() => resolve('no report'), 5_000);
      });
      document.head.append(script);
      return { ran: Reflect.get(window, '__okfInlineRan') === true, reported: await reported };
    });
    expect(ran).toEqual({ ran: false, reported: expect.stringMatching(/^script-src/) });
  });

  test('the inventory records nested styles and transient data images', async ({ page }) => {
    // A5 requires every inserted style and data/blob use. These known probe values are
    // independent of the observer; a library may append a subtree and remove it in one task.
    await watchPolicy(page);
    await underPagePolicy(page, server.origin());
    await routeApi(page);
    await page.goto(`${server.origin()}/`);
    await expect(page.getByRole('link', { name: /Workshop notes/ }).first()).toBeVisible();
    await page.evaluate(() => {
      const subtree = document.createElement('div');
      const style = document.createElement('style');
      style.textContent = ':root { --okf-policy-probe: 1; }';
      const image = document.createElement('img');
      image.src = 'data:image/gif;base64,R0lGODlhAQABAAAAACw=';
      subtree.append(style, image);
      document.body.append(subtree);
      subtree.remove();
    });
    const found = await policyFindings(page);
    expect(found.styles.some((style) => style.text.includes('--okf-policy-probe'))).toBe(true);
    expect(found.urls).toContainEqual({
      url: 'data:image/gif;base64,R0lGODlhAQABAAAAACw=',
      where: '<img> src',
    });
  });

  for (const screen of SCREENS) {
    test(`${screen.name}: nothing outside the policy`, async ({ page, browserName }) => {
      await watchPolicy(page);
      const console = watchConsole(page);
      await underPagePolicy(page, server.origin());
      const { answers } = await playSandbox(page, server.origin());
      await routeApi(page, { ...screen.answers, ...answers });
      await routeObjects(page);
      await page.goto(`${server.origin()}${screen.path}`);
      await screen.ready(page);
      // Let what the screen started settle: its requests answered and two frames drawn.
      await page.waitForLoadState('networkidle', { timeout: 10_000 }).catch(() => undefined);
      await page.evaluate(
        () =>
          new Promise<void>((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
          ),
      );
      const found = await policyFindings(page);
      await mkdir(findingsDir, { recursive: true });
      const file = `${screen.name.replace(/[^a-z0-9]+/gi, '-').toLowerCase()}-${browserName}.json`;
      await writeFile(
        join(findingsDir, file),
        `${JSON.stringify(
          {
            screen: screen.name,
            browserName,
            console,
            ...found,
            libraries: {
              styles: found.styles.map((style) => ({
                ...style,
                library:
                  KNOWN_STYLES.find(
                    (entry) =>
                      entry.text.test(style.text) && entry.attributes.test(style.attributes),
                  )?.library ?? 'unlisted',
              })),
              urls: found.urls.map((use) => ({
                ...use,
                library:
                  KNOWN_URLS.find((entry) => entry.url.test(use.url) && entry.where.test(use.where))
                    ?.library ?? 'unlisted',
              })),
            },
          },
          null,
          2,
        )}\n`,
      );
      expect(unlisted(found, console), `${screen.name} in ${browserName}`).toEqual([]);
    });
  }
});
