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
 * code, other), the original beside its digest in MergeView, and a correction being edited.
 * Readiness waits for rendered content and the libraries' asynchronous work, not a viewer shell.
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
import { expect, type Locator, type Page, test } from '@playwright/test';
import { adminWorkspace, localOwner, tenantLog, workspacePage } from './support/admin-security';
import {
  type Answers,
  NOTE,
  ok,
  routeApi,
  shot,
  useExplorerServer,
  WS,
  writableWorkspace,
} from './support/explorer';
import {
  ELSEWHERE,
  HTML_ITEM,
  htmlSourceAnswers,
  ORIGINALS,
  type Original,
  originalsAnswers,
  PLAIN_DIGEST,
  PLAIN_ITEM,
  PLAIN_ORIGINAL,
  PLAIN_PATH,
  type PolicyFindings,
  pagePolicy,
  playSandbox,
  policyFindings,
  routeObjects,
  SANDBOX_ORIGIN,
  underPagePolicy,
  watchPolicy,
} from './support/m4a-sandbox';
import { type PolicyConsoleLine, partitionPolicyConsole } from './support/m4a-sandbox-policy';
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
  /** Constrain an authored viewer's use to the screen that exercised it. */
  screen?: RegExp;
}

/** Application violations have no exemptions. Intentional CSP controls are asserted separately. */
const KNOWN_VIOLATIONS: KnownViolation[] = [];

/** Installed-library styles allowed by the server's existing style-src 'unsafe-inline'. */
const KNOWN_STYLES: KnownStyle[] = [
  {
    text: /^\.\u037c[a-z0-9]+\.cm-focused \{outline:/,
    attributes: /^$/,
    library:
      '@codemirror/view 6.41.0 through style-mod 4.1.4; @codemirror/merge 6.12.1 on comparison/correction screens',
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

/** Source-backed uses under existing directives; the integrated browser run records observations. */
const KNOWN_URLS: KnownUrl[] = [
  {
    url: /^data:image\/svg\+xml,/,
    where: /^stylesheet <style>: .*\.cm-highlightTab /,
    library: '@codemirror/view 6.41.0 baseTheme .cm-highlightTab; img-src data:',
  },
  {
    url: /^blob:/,
    where: /^(URL\.createObjectURL \(image\/png\)|<img> src)$/,
    screen: /^an original: image \(image\/png\)$/,
    library: 'workspace ImageFrame/useObjectUrl (image/png original); img-src blob:',
  },
];

function knownUrl(use: { url: string; where: string }, screen: string): KnownUrl | undefined {
  return KNOWN_URLS.find(
    (entry) =>
      entry.url.test(use.url) &&
      entry.where.test(use.where) &&
      (entry.screen === undefined || entry.screen.test(screen)),
  );
}

/** Console lines in which a browser refuses something under a Content-Security-Policy. */
const REFUSED = /Content Security Policy|Content-Security-Policy/i;

/** One screen of an existing journey: where it is, what answers it, and when it has drawn. */
interface Screen {
  name: string;
  path: string;
  answers: Answers;
  /** The screen has drawn what the journey checks. */
  ready: (page: Page) => Promise<void>;
  /** This readiness check verifies the hostile file actually loaded in its capability frame. */
  hostileFrame?: boolean;
  screenshot?: string;
}

/** CodeMirror renders line divs without newline text nodes; retain every authored line exactly. */
async function expectCodeMirrorLines(textbox: Locator, fixture: string) {
  await expect
    .poll(() => textbox.locator('.cm-line').allTextContents())
    .toEqual(fixture.split('\n'));
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
  {
    name: 'the original beside its digest in MergeView',
    path: `/w/${WS}/i/${PLAIN_ITEM}`,
    answers: originalsAnswers(),
    screenshot: 'm4a-page-policy-comparison',
    ready: async (page) => {
      const original = await openOriginal(page);
      await original
        .getByRole('button', { name: 'Compare the original with its converted text' })
        .click();
      const comparison = original.getByRole('region', {
        name: 'The original beside its converted text',
      });
      const left = comparison.getByRole('textbox', { name: `Original: ${PLAIN_PATH}` });
      const right = comparison.getByRole('textbox', {
        name: 'Converted text: what the reader shows',
      });
      await expect(left).toHaveAttribute('aria-readonly', 'true');
      await expect(right).toHaveAttribute('aria-readonly', 'true');
      await expectCodeMirrorLines(left, PLAIN_ORIGINAL);
      await expectCodeMirrorLines(right, PLAIN_DIGEST);
      await expect(comparison.locator('.cm-mergeView')).toBeVisible();
      await comparison.getByRole('button', { name: 'Next difference' }).click();
      await expect(comparison).toContainText('Difference 1 of 2:');
    },
  },
  {
    name: 'the correction editor with changed text in MergeView',
    path: `/w/${WS}/i/${PLAIN_ITEM}`,
    answers: {
      ...originalsAnswers(),
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
    },
    screenshot: 'm4a-page-policy-correction',
    ready: async (page) => {
      await openOriginal(page);
      const corrections = page.getByRole('region', { name: 'Corrections' });
      await corrections.getByRole('button', { name: 'Correct text' }).click();
      const shown = corrections.getByRole('textbox', { name: 'The text shown now' });
      const yours = corrections.getByRole('textbox', { name: 'Your correction' });
      await expect(shown).toHaveAttribute('aria-readonly', 'true');
      await expectCodeMirrorLines(shown, PLAIN_DIGEST);
      await expectCodeMirrorLines(yours, PLAIN_DIGEST);
      await yours.fill('# Agenda\n\nBudget: 1.4 M\n\nDone.\n');
      await expect(yours).toContainText('Budget: 1.4 M');
      await expect(corrections).toContainText(
        'Your text differs from the text shown in 1 place: line 3.',
      );
      await expect(corrections.locator('.cm-mergeView')).toBeVisible();
      await expect(
        corrections.getByRole('button', { name: 'Save the correction' }),
      ).toHaveAttribute('aria-disabled', 'false');
    },
  },
  ...ORIGINALS.map(
    (original): Screen => ({
      name: `an original: ${original.kind} (${original.mediaType})`,
      path: `/w/${WS}/i/${original.id}`,
      answers: originalsAnswers(),
      ready: async (page) => originalShown(page, original),
      hostileFrame: original.kind === 'html',
    }),
  ),
];

/** Wait for the real library's loaded content, rather than an asynchronous viewer's container. */
async function originalShown(page: Page, original: Original) {
  const region = await openOriginal(page);
  switch (original.kind) {
    case 'pdf': {
      const canvas = region.getByRole('img', {
        name: /^Page 1 of 1 .*Its text is laid over the drawing/,
      });
      await expect(canvas).toBeVisible();
      await expect(region.locator('.textLayer')).toContainText('Annual report 2025');
      await expect
        .poll(() =>
          canvas.evaluate((element) => {
            if (
              !(element instanceof HTMLCanvasElement) ||
              element.width === 0 ||
              element.height === 0
            )
              return false;
            const pixels = element
              .getContext('2d')
              ?.getImageData(0, 0, element.width, element.height).data;
            if (pixels === undefined) return false;
            for (let at = 0; at < pixels.length; at += 4) {
              if ((pixels[at] ?? 255) < 200 && (pixels[at + 3] ?? 0) > 0) return true;
            }
            return false;
          }),
        )
        .toBe(true);
      return;
    }
    case 'image': {
      const image = region.getByRole('img', {
        name: `${original.filename}, the original image. It carries no caption of its own.`,
      });
      await expect(image).toBeVisible();
      expect(
        await image.evaluate(async (element) => {
          if (!(element instanceof HTMLImageElement))
            throw new Error('The original is not an image');
          await element.decode();
          return { width: element.naturalWidth, height: element.naturalHeight };
        }),
      ).toEqual({ width: 320, height: 180 });
      return;
    }
    case 'html':
      await expect(
        page
          .frameLocator('iframe[sandbox]')
          .getByRole('heading', { name: 'Welcome to the workshop' }),
      ).toBeVisible();
      return;
    case 'spreadsheet': {
      const table = region.getByRole('table');
      await expect(table).toHaveAccessibleName('Table 1 of 1: 2 columns, 4 rows');
      await expect(table.getByRole('columnheader', { name: 'quarter', exact: true })).toBeVisible();
      await expect(table.getByRole('cell', { name: '2.3', exact: true })).toBeVisible();
      return;
    }
    case 'text': {
      const text = region.getByRole('textbox', { name: `Original text of ${original.path}` });
      await expect(text).toHaveAttribute('aria-readonly', 'true');
      await expectCodeMirrorLines(text, original.bytes.toString('utf8'));
      if (original.mediaType === 'application/json')
        await expect(text).toHaveAttribute('data-language', 'json');
      return;
    }
    case 'office':
      await expect(region).toContainText('This Rich Text document is shown as its conversion:');
      await expect(
        region.getByRole('link', { name: `Download the original, ${original.filename}` }),
      ).toBeVisible();
      return;
    case 'other':
      await expect(region).toContainText('There is no viewer here for files of this type');
      await expect(
        region.getByRole('link', { name: `Download the original, ${original.filename}` }),
      ).toBeVisible();
  }
}

/** Every console line of the page's own documents in which the browser refuses under a policy. */
function watchConsole(page: Page): PolicyConsoleLine[] {
  const lines: PolicyConsoleLine[] = [];
  page.on('console', (message) => {
    const from = message.location().url;
    if (REFUSED.test(message.text())) lines.push({ text: message.text(), source: from });
  });
  return lines;
}

function attributedConsole(
  lines: readonly PolicyConsoleLine[],
  found: PolicyFindings,
  hostileFrameShown: boolean,
) {
  return partitionPolicyConsole(lines, {
    sandboxOrigin: SANDBOX_ORIGIN,
    pixelUrl: `${ELSEWHERE}/pixel.png`,
    hostileFrameShown,
    parentViolations: found.violations,
  });
}

/** What is not on the known lists, in words the failure shows. */
function unlisted(found: PolicyFindings, console: PolicyConsoleLine[], screen = ''): string[] {
  const left: string[] = [];
  for (const violation of found.violations) {
    const known = KNOWN_VIOLATIONS.some(
      (entry) => entry.directive.test(violation.directive) && entry.blocked.test(violation.blocked),
    );
    if (!known) left.push(`violation: ${JSON.stringify(violation)}`);
  }
  for (const line of console) {
    const known = KNOWN_VIOLATIONS.some(
      (entry) => entry.directive.test(line.text) && entry.blocked.test(line.text),
    );
    if (!known) left.push(`console: ${JSON.stringify(line)}`);
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
    if (knownUrl(use, screen) === undefined) {
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

  test('a parent image violation is retained even when the hostile frame uses the same pixel', async ({
    page,
    browserName,
  }) => {
    // Same target as the known WebKit sandbox refusal. A real parent securitypolicyviolation
    // must keep it in the failing application inventory, even if its console source is absent.
    await watchPolicy(page);
    const lines = watchConsole(page);
    await underPagePolicy(page, server.origin());
    const { answers } = await playSandbox(page, server.origin());
    await routeApi(page, { ...htmlSourceAnswers(), ...answers });
    await page.goto(`${server.origin()}/w/${WS}/i/${HTML_ITEM}`);
    await openOriginal(page);
    await expect(
      page
        .frameLocator('iframe[sandbox]')
        .getByRole('heading', { name: 'Welcome to the workshop' }),
    ).toBeVisible();
    await page.evaluate((url) => {
      const image = document.createElement('img');
      image.alt = 'Intentional parent policy control';
      image.src = url;
      document.body.append(image);
    }, `${ELSEWHERE}/pixel.png`);
    await expect
      .poll(async () =>
        (await policyFindings(page)).violations.some((violation) =>
          violation.directive.startsWith('img-src'),
        ),
      )
      .toBe(true);
    const found = await policyFindings(page);
    const attributed = attributedConsole(lines, found, true);
    expect(
      unlisted(found, attributed.application).some(
        (entry) => entry.startsWith('violation:') && entry.includes('img-src'),
      ),
    ).toBe(true);
    const unlocated = {
      text: `Refused to load ${ELSEWHERE}/pixel.png because it does not appear in the img-src directive of the Content Security Policy.`,
      source: '',
    };
    expect(attributedConsole([unlocated], found, true).application).toEqual([unlocated]);
    await mkdir(findingsDir, { recursive: true });
    await writeFile(
      join(findingsDir, `intentional-parent-image-control-${browserName}.json`),
      `${JSON.stringify({ intentionalControl: true, allConsole: lines, console: attributed.application, sandboxConsole: attributed.sandbox, ...found }, null, 2)}\n`,
    );
  });

  for (const screen of SCREENS) {
    test(`${screen.name}: nothing outside the policy`, async ({ page, browserName }) => {
      await watchPolicy(page);
      const lines = watchConsole(page);
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
      const console = attributedConsole(lines, found, screen.hostileFrame === true);
      if (screen.screenshot !== undefined) await shot(page, screen.screenshot);
      await mkdir(findingsDir, { recursive: true });
      const file = `${screen.name.replace(/[^a-z0-9]+/gi, '-').toLowerCase()}-${browserName}.json`;
      await writeFile(
        join(findingsDir, file),
        `${JSON.stringify(
          {
            screen: screen.name,
            browserName,
            allConsole: lines,
            console: console.application,
            sandboxConsole: console.sandbox,
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
                library: knownUrl(use, screen.name)?.library ?? 'unlisted',
              })),
            },
          },
          null,
          2,
        )}\n`,
      );
      expect(
        unlisted(found, console.application, screen.name),
        `${screen.name} in ${browserName}`,
      ).toEqual([]);
    });
  }
});
