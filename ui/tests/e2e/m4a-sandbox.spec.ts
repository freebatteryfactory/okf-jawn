/**
 * An HTML source in the sandbox (UI M4a, ruling A3), in Chromium and WebKit, under the server's
 * page policy: the reader's original of an HTML file is a frame of the sandbox origin, another
 * origin than the page's, through a capability `create_sandbox_capability` minted just before
 * framing. The frame's attributes are read in the browser (`sandbox` present and empty, no token;
 * `referrerpolicy="no-referrer"`; `src` exactly the capability's `url`), and its words say that
 * scripts do not run and that links and forms do nothing.
 *
 * The hostile file (`support/m4a-sandbox.ts`) tries everything: scripts in its head and body and
 * an inline handler would rewrite its heading, mark the page, post to the page that frames it,
 * reach another site and move this page; its picture, its link, its new-window link, its
 * whole-page link and its form point at the other site. The journeys prove that none of it
 * happens: the heading stays as written, nothing is marked or posted, no request reaches the other
 * site, no window opens, and this page stays where it is. A last journey drops the sandbox
 * origin's headers and shows that the frame's own sandbox still stops every script.
 *
 * A reload mints a new capability: after the first has expired (the page says so), and when the
 * person asks. One journey goes by the keyboard alone (Tab and Enter; buttons and fields are in
 * the Tab order of both engines, so no link needs E8 here). Every screen has zero axe violations
 * and visible focus where a control of the page has it, and a screenshot for the owner under
 * `.artifacts/ui/` (untracked).
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself; the API and the
 * sandbox origin are played through `page.route` (`support/explorer.ts`,
 * `support/m4a-sandbox.ts`). The original is reached through `openOriginal`
 * (`support/reader.ts`), whatever the reader's views make of it.
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import { expectVisibleFocus, routeApi, shot, useExplorerServer, WS } from './support/explorer';
import {
  ELSEWHERE,
  expectNoAxeViolationsBesideTheSandbox,
  HTML_FILE,
  HTML_ITEM,
  HTML_OBJECT,
  htmlSourceAnswers,
  playSandbox,
  type SandboxService,
  underPagePolicy,
} from './support/m4a-sandbox';
import { openOriginal } from './support/reader';
import { tabTo } from './support/views';

/** What the page itself saw: messages posted to it and console lines, from the first script on. */
interface Watched {
  console: string[];
  messages: () => Promise<string[]>;
}

/** Collect the messages posted to the page (from before any script of its own) and its console. */
async function watchPage(page: Page): Promise<Watched> {
  await page.addInitScript(() => {
    const seen: string[] = [];
    Object.defineProperty(window, '__okfPosted', { value: seen });
    window.addEventListener('message', (event) => seen.push(String(event.data)));
  });
  const lines: string[] = [];
  page.on('console', (message) => lines.push(message.text()));
  return {
    console: lines,
    messages: () =>
      page.evaluate(() => {
        const seen: unknown = Reflect.get(window, '__okfPosted');
        return Array.isArray(seen) ? seen.map(String) : ['the watch did not start'];
      }),
  };
}

/**
 * Open the HTML source under the page policy, with the sandbox played; `isolation` replaces the
 * sandbox origin's own headers when a journey says so.
 */
async function openHtmlSource(
  page: Page,
  origin: string,
  options: { lifetimeMs?: number; isolation?: Record<string, string> } = {},
): Promise<{ service: SandboxService; watched: Watched; original: Locator }> {
  const watched = await watchPage(page);
  await underPagePolicy(page, origin);
  const { service, answers } = await playSandbox(page, origin, options);
  await routeApi(page, { ...htmlSourceAnswers(), ...answers });
  await page.goto(`${origin}/w/${WS}/i/${HTML_ITEM}`);
  const original = await openOriginal(page);
  await expect(original).toBeVisible();
  return { service, watched, original };
}

const viewer = (original: Locator) =>
  original.getByRole('region', { name: `${HTML_FILE} in the sandbox` });
const frameOf = (original: Locator) =>
  viewer(original).locator(`iframe[title="${HTML_FILE}, as written, in the sandbox"]`);
const inside = (page: Page) =>
  page.frameLocator(`iframe[title="${HTML_FILE}, as written, in the sandbox"]`);
const reload = (original: Locator) =>
  viewer(original).getByRole('button', { name: 'Reload the page' });

/** The frame's attributes as the browser holds them, and the address its document came from. */
async function frameAttributes(page: Page, frame: Locator) {
  const attributes = await frame.evaluate((element) => {
    if (!(element instanceof HTMLIFrameElement)) return null;
    return {
      hasSandbox: element.hasAttribute('sandbox'),
      sandbox: element.getAttribute('sandbox'),
      tokens: element.sandbox.length,
      referrerPolicy: element.referrerPolicy,
      referrerAttribute: element.getAttribute('referrerpolicy'),
      src: element.getAttribute('src'),
      allow: element.getAttribute('allow'),
    };
  });
  const loaded = page.frames().map((frame) => frame.url());
  return { attributes, loaded };
}

/** The frame is exactly what A3 asks of it, framing the `number`th capability minted. */
async function expectFramed(page: Page, original: Locator, service: SandboxService, number = 1) {
  // The page asks for it in its own time: once it has, the service has minted it.
  await expect.poll(() => service.minted.length).toBeGreaterThanOrEqual(number);
  const capability = service.minted[number - 1];
  if (capability === undefined) throw new Error(`capability ${number} was not minted`);
  const frame = frameOf(original);
  await expect(frame).toHaveAttribute('src', capability.url);
  await expect(inside(page).getByRole('heading', { level: 1 })).toHaveText(
    'Welcome to the workshop',
  );
  const { attributes, loaded } = await frameAttributes(page, frame);
  expect(attributes).toEqual({
    hasSandbox: true,
    sandbox: '',
    tokens: 0,
    referrerPolicy: 'no-referrer',
    referrerAttribute: 'no-referrer',
    src: capability.url,
    allow: null,
  });
  // The frame's document is the capability's, on the sandbox origin.
  expect(loaded).toContain(capability.url);
  // Bound to the exact object of the exact revision the reader shows.
  expect(service.asked[number - 1]).toEqual({
    workspace_id: WS,
    item_id: HTML_ITEM,
    revision: 'a'.repeat(40),
    object: HTML_OBJECT,
  });
}

/** None of the file's scripts ran, and nothing of it reached the page or another site. */
async function expectNothingRan(page: Page, service: SandboxService, watched: Watched) {
  await expect(inside(page).getByRole('heading', { level: 1 })).toHaveText(
    'Welcome to the workshop',
  );
  await expect(inside(page).locator('[data-script-ran]')).toHaveCount(0);
  expect(await watched.messages()).toEqual([]);
  expect(service.elsewhere).toEqual([]);
  expect(new URL(page.url()).pathname).toBe(`/w/${WS}/i/${HTML_ITEM}`);
}

/** Wait for a console line the browser writes when it refuses something; returns it. */
async function refusal(watched: Watched, pattern: RegExp): Promise<string> {
  await expect
    .poll(() => watched.console.find((line) => pattern.test(line)) ?? '')
    .toMatch(pattern);
  return watched.console.find((line) => pattern.test(line)) ?? '';
}

test.describe('an HTML source in the sandbox', () => {
  const server = useExplorerServer();

  test('the file in its frame, with its words; its scripts do not run', async ({ page }) => {
    const { service, watched, original } = await openHtmlSource(page, server.origin());
    await expect(viewer(original).getByRole('note')).toHaveText(
      'This is the file as written, shown in a sandbox on a separate origin. Scripts in it do not run, and its links and forms do nothing: none of them reaches another site or changes this page. Pictures, styles and fonts from elsewhere are not loaded.',
    );
    await expectFramed(page, original, service);
    await expect(viewer(original).getByRole('status')).toContainText(
      'Its address on the sandbox origin is good until',
    );
    // The sandbox origin was asked for the capability's address alone, with no referrer.
    expect(service.served).toEqual([
      { path: new URL(service.minted[0]?.url ?? '').pathname, referer: null, cookie: null },
    ]);
    // Every script of the file was refused by the frame's sandbox, and none ran.
    await refusal(watched, /Blocked script execution in 'http:\/\/sandbox\.okf\.test:7712/);
    await expectNothingRan(page, service, watched);
    await expectNoAxeViolationsBesideTheSandbox(page, 'an HTML source in the sandbox');
    await viewer(original).scrollIntoViewIfNeeded();
    await shot(page, 'm4a-sandbox-01-framed');
  });

  test('its links and forms do nothing: no window, no move, no other site', async ({
    page,
    browserName,
  }) => {
    const { service, watched, original } = await openHtmlSource(page, server.origin());
    await expectFramed(page, original, service);

    // The form is not sent: the frame's sandbox has no allow-forms.
    await inside(page).getByRole('button', { name: 'Send' }).click();
    await refusal(watched, /Blocked form submission to 'http:\/\/elsewhere\.okf\.test/);
    await expectNothingRan(page, service, watched);

    // No window opens, and this page does not move: no allow-popups, no allow-top-navigation.
    const popup = page.waitForEvent('popup', { timeout: 2_000 }).then(
      () => 'a window opened',
      () => 'no window',
    );
    await inside(page).getByRole('link', { name: 'Open the programme in a new window' }).click();
    expect(await popup).toBe('no window');
    await inside(page)
      .getByRole('link', { name: 'Open the programme in place of this page' })
      .click();
    await expect(inside(page).getByRole('heading', { level: 1 })).toBeVisible();
    expect(page.context().pages()).toHaveLength(1);
    await expectNothingRan(page, service, watched);

    // A link to another site reaches nothing: the page policy's frame-src refuses it. What the
    // frame shows then is the browser's: Chromium its refusal page, WebKit still the file.
    await inside(page).getByRole('link', { name: 'Read the programme on another site' }).click();
    await refusal(watched, /elsewhere\.okf\.test\/.*frame-src|frame-src.*elsewhere/);
    if (browserName === 'chromium') {
      await expect
        .poll(() => page.frames().map((frame) => frame.url()))
        .toContain('chrome-error://chromewebdata/');
    } else {
      await expect(inside(page).getByRole('heading', { level: 1 })).toHaveText(
        'Welcome to the workshop',
      );
    }
    expect(service.elsewhere).toEqual([]);
    expect(new URL(page.url()).pathname).toBe(`/w/${WS}/i/${HTML_ITEM}`);
    expect(await watched.messages()).toEqual([]);
    await expectNoAxeViolationsBesideTheSandbox(page, 'the sandbox after its links were followed');
    await viewer(original).scrollIntoViewIfNeeded();
    await shot(page, 'm4a-sandbox-02-links-followed');

    // A reload shows the file again, through a new capability.
    await reload(original).click();
    await expectFramed(page, original, service, 2);
    await expect(viewer(original).getByRole('status')).toContainText('good until');
    expect(service.elsewhere).toEqual([]);
  });

  test('a reload after the capability expired mints a new one', async ({ page }) => {
    const { service, watched, original } = await openHtmlSource(page, server.origin(), {
      lifetimeMs: 2_000,
    });
    await expectFramed(page, original, service);
    const status = viewer(original).getByRole('status');
    await expect(status).toContainText('Its address on the sandbox origin expired at');
    await expect(status).toContainText('and reloading asks for a new one.');
    // What the frame loaded stays; nothing is minted until a reload.
    await expect(inside(page).getByRole('heading', { level: 1 })).toBeVisible();
    expect(service.minted).toHaveLength(1);
    await expectNoAxeViolationsBesideTheSandbox(page, 'an expired capability');
    await viewer(original).scrollIntoViewIfNeeded();
    await shot(page, 'm4a-sandbox-03-expired');

    await reload(original).click();
    await expectFramed(page, original, service, 2);
    expect(service.minted).toHaveLength(2);
    expect(service.minted[1]?.url).not.toBe(service.minted[0]?.url);
    expect(service.served.map((served) => served.path)).toEqual(
      service.minted.map((capability) => new URL(capability.url).pathname),
    );
    await expect(status).toContainText('good until');
    await expectNothingRan(page, service, watched);
  });

  test('from the keyboard alone: into the frame and out, and a reload', async ({ page }) => {
    const { service, watched, original } = await openHtmlSource(page, server.origin());
    await expectFramed(page, original, service);

    // Tab reaches the file's form inside the frame; Enter sends nothing.
    const send = inside(page).getByRole('button', { name: 'Send' });
    await tabTo(page, send);
    await page.keyboard.press('Enter');
    await refusal(watched, /Blocked form submission to 'http:\/\/elsewhere\.okf\.test/);
    await expectNothingRan(page, service, watched);

    // Tab leads out of the frame to the page's own control, and Enter reloads.
    await tabTo(page, reload(original));
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expectFramed(page, original, service, 2);
    await expect(reload(original)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNothingRan(page, service, watched);
    await expectNoAxeViolationsBesideTheSandbox(page, 'the sandbox reloaded from the keyboard');
    await shot(page, 'm4a-sandbox-04-keyboard-reloaded');
  });

  test('without the sandbox origin’s headers, the frame’s own sandbox still stops every script', async ({
    page,
  }) => {
    const { service, watched, original } = await openHtmlSource(page, server.origin(), {
      isolation: {},
    });
    await expectFramed(page, original, service);
    await refusal(watched, /Blocked script execution in 'http:\/\/sandbox\.okf\.test:7712/);
    await expect(inside(page).getByRole('heading', { level: 1 })).toHaveText(
      'Welcome to the workshop',
    );
    await expect(inside(page).locator('[data-script-ran]')).toHaveCount(0);
    expect(await watched.messages()).toEqual([]);
    // Without the origin's policy the picture is fetched; no script's request ever is.
    expect(service.elsewhere.filter((url) => !url.startsWith(`${ELSEWHERE}/pixel.png`))).toEqual(
      [],
    );
    expect(new URL(page.url()).pathname).toBe(`/w/${WS}/i/${HTML_ITEM}`);
  });
});
