/**
 * Turn WebMCP on and off (rulings B5 and B6), in Chromium and WebKit, from the keyboard alone:
 * Tab to reach a control, Enter to follow a link, Space to turn the setting.
 *
 * Neither engine offers WebMCP yet, so a fake `document.modelContext` shaped as the W3C draft has
 * it is installed before the page's own scripts (`addInitScript`, once per page load): its
 * `registerTool(tool, { signal })` answers a resolved Promise and writes each registration, and
 * each abort of a registration's signal, to a log on `window`. The person opens the workspace,
 * goes to Settings from the bar, turns the setting on (the five tools are registered, the four
 * that read marked `readOnlyHint`), and off (every registration is aborted); after a reload the
 * choice is still what they left. A browser without WebMCP is told so on the page. Each screen has
 * zero axe violations and visible focus, and a screenshot for the owner under `.artifacts/ui/`.
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  NOTE,
  ok,
  routeApi,
  shot,
  useExplorerServer,
  WS,
  workspace,
} from './support/explorer';

const OLD = 'b'.repeat(40);

const SETTING = 'Let an assistant in this browser use the page tools (WebMCP)';
const FIVE = [
  'get_selection',
  'open_comparison',
  'open_source',
  'prepare_proposal',
  'preview_names',
];
const READ_ONLY = ['get_selection', 'open_comparison', 'open_source', 'preview_names'];

/** Install the fake model context before the page's scripts run, on every load of the page. */
async function offerWebMcp(page: Page) {
  await page.addInitScript(() => {
    const log: string[] = [];
    Object.defineProperty(window, 'webmcpLog', { value: log });
    // The live tools by name, so that a spec can run one as an assistant would.
    const live = new Map<string, (input: object) => Promise<unknown>>();
    Object.defineProperty(window, 'webmcpRun', {
      value: (name: string, input: object) =>
        live.get(name)?.(input) ?? Promise.reject(new Error(`no live tool named ${name}`)),
    });
    const registerTool = (
      tool: {
        name: string;
        annotations?: { readOnlyHint?: boolean };
        execute: (input: object, options: { signal: AbortSignal }) => Promise<unknown>;
      },
      options: { signal?: AbortSignal } = {},
    ) => {
      const readOnly = tool.annotations?.readOnlyHint === true ? ' read-only' : '';
      log.push(`register ${tool.name}${readOnly}`);
      live.set(tool.name, (input) => tool.execute(input, { signal: new AbortController().signal }));
      options.signal?.addEventListener('abort', () => {
        log.push(`abort ${tool.name}`);
        live.delete(tool.name);
      });
      return Promise.resolve(undefined);
    };
    Object.defineProperty(document, 'modelContext', {
      configurable: true,
      value: { registerTool },
    });
  });
}

/** What the fake has seen since this page loaded: each registration and each abort. */
async function registrations(page: Page) {
  const log = await page.evaluate(() => {
    const entries: unknown = Reflect.get(window, 'webmcpLog');
    return Array.isArray(entries)
      ? entries.filter((entry): entry is string => typeof entry === 'string')
      : [];
  });
  const registered = log.filter((entry) => entry.startsWith('register '));
  const names = registered.map((entry) => entry.split(' ')[1] ?? '').sort();
  const readOnly = registered
    .filter((entry) => entry.endsWith(' read-only'))
    .map((entry) => entry.split(' ')[1] ?? '')
    .sort();
  const aborted = log.filter((entry) => entry.startsWith('abort ')).map((entry) => entry.slice(6));
  const live = [...names];
  for (const name of aborted) {
    const at = live.indexOf(name);
    if (at >= 0) live.splice(at, 1);
  }
  return { names, readOnly, aborted: aborted.sort(), live };
}

/** Run the live tool `name` as an assistant would; its answer. */
async function runTool(page: Page, name: string, input: object = {}): Promise<unknown> {
  const answer = await page.evaluate(
    async ([tool, given]) => {
      const run: unknown = Reflect.get(window, 'webmcpRun');
      if (typeof run !== 'function') throw new Error('no fake model context');
      return JSON.stringify(await run(tool, given));
    },
    [name, input] as const,
  );
  return JSON.parse(answer);
}

/** Tab until `target` has keyboard focus; never more than `limit` presses. */
async function tabTo(page: Page, target: Locator, limit = 60) {
  for (let presses = 0; presses < limit; presses += 1) {
    if (await target.evaluate((element) => element === document.activeElement)) return;
    await page.keyboard.press('Tab');
  }
  await expect(target).toBeFocused();
}

/** Go to Settings from the workspace's bar, from the keyboard. */
async function settingsFromTheBar(page: Page, browserName: string) {
  const link = page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('link', { name: 'Settings' });
  if (browserName === 'webkit') {
    // WebKit, like Safari by default, leaves links out of Tab order (a platform setting the page
    // cannot change), so the link is focused directly; Enter still follows it.
    await link.focus();
  } else {
    await page.getByRole('heading', { level: 1, name: 'Workshop notes' }).focus();
    await tabTo(page, link, 20);
  }
  await expect(link).toBeFocused();
  await expectVisibleFocus(page);
  await page.keyboard.press('Enter');
  const settings = page.getByRole('region', { name: 'Settings' });
  await expect(settings.getByRole('heading', { level: 2, name: 'Settings' })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/w/${WS}/settings$`));
  return settings;
}

test.describe('WebMCP setting', () => {
  const server = useExplorerServer();

  test('turn WebMCP on and off, from the keyboard, and the choice is kept', async ({
    page,
    browserName,
  }) => {
    await routeApi(page);
    await offerWebMcp(page);
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(page.getByRole('heading', { level: 1, name: 'Workshop notes' })).toBeVisible();
    // Off by default: nothing is registered.
    expect((await registrations(page)).names).toEqual([]);

    const settings = await settingsFromTheBar(page, browserName);
    const box = settings.getByRole('checkbox', { name: SETTING });
    await expect(box).not.toBeChecked();
    await expect(settings).toContainText('kept in this browser, not in the workspace');
    await expect(settings).toContainText('None of them can verify, approve or review anything.');
    await expect(settings).not.toContainText('This browser does not offer WebMCP');
    await expectNoAxeViolations(page, 'settings, WebMCP off');
    await shot(page, 'webmcp-01-settings-off');

    // On: the five tools are registered, the four that read marked so, none aborted.
    await tabTo(page, box);
    await expectVisibleFocus(page);
    await page.keyboard.press('Space');
    await expect(box).toBeChecked();
    await expect(box).toBeFocused();
    await expect(settings.getByRole('status')).toContainText('Turned on');
    await expect(settings.getByText('The browser took 5 of the 5 page tools.')).toBeVisible();
    const on = await registrations(page);
    expect(on.names).toEqual(FIVE);
    expect(on.readOnly).toEqual(READ_ONLY);
    expect(on.aborted).toEqual([]);
    await expectNoAxeViolations(page, 'settings, WebMCP on');
    await shot(page, 'webmcp-02-settings-on');

    // Kept: after a reload the setting is on and the tools are registered again.
    await page.reload();
    await expect(box).toBeChecked();
    await expect.poll(async () => (await registrations(page)).live).toEqual(FIVE);

    // Off: every registration is aborted.
    await tabTo(page, box);
    await page.keyboard.press('Space');
    await expect(box).not.toBeChecked();
    await expect(box).toBeFocused();
    await expectVisibleFocus(page);
    await expect(settings.getByRole('status')).toContainText('Turned off');
    await expect.poll(async () => (await registrations(page)).aborted).toEqual(FIVE);
    expect((await registrations(page)).live).toEqual([]);
    await expectNoAxeViolations(page, 'settings, WebMCP off again');
    await shot(page, 'webmcp-03-settings-off-again');

    // Kept off: after a reload nothing is registered.
    await page.reload();
    await expect(box).not.toBeChecked();
    expect((await registrations(page)).names).toEqual([]);
  });

  test('a browser that does not offer WebMCP is told so, and the choice is still kept', async ({
    page,
    browserName,
  }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(page.getByRole('heading', { level: 1, name: 'Workshop notes' })).toBeVisible();
    const settings = await settingsFromTheBar(page, browserName);
    await expect(settings.getByRole('note')).toContainText('This browser does not offer WebMCP');
    const box = settings.getByRole('checkbox', { name: SETTING });
    await tabTo(page, box);
    await page.keyboard.press('Space');
    await expect(box).toBeChecked();
    await expect(settings.getByRole('status')).toContainText(
      'This browser does not offer WebMCP, so no tool is offered here.',
    );
    await expectNoAxeViolations(page, 'settings, no WebMCP');
    await shot(page, 'webmcp-04-settings-not-offered');
    await page.reload();
    await expect(box).toBeChecked();
  });

  test('what an assistant prepares and opens through the tools, as the person sees it', async ({
    page,
  }) => {
    let proposed = 0;
    await routeApi(page, {
      '/api/workspaces/open-workspace': () =>
        ok({ ...workspace, permissions: ['read', 'propose'] }),
      '/api/history/diff-items': (body) =>
        ok({
          from: body.from,
          to: body.to,
          changes: [
            {
              old_path: 'notes/meeting.md',
              new_path: 'notes/meeting.md',
              kind: 'modified',
              binary: false,
              patch: [
                'diff --git a/notes/meeting.md b/notes/meeting.md',
                '--- a/notes/meeting.md',
                '+++ b/notes/meeting.md',
                '@@ -3,2 +3,2 @@',
                ' # Meeting notes',
                '-The budget is draft.',
                '+The budget is published.',
                '',
              ].join('\n'),
            },
          ],
          warnings: [],
        }),
      '/api/proposals/open-proposal': () => {
        proposed += 1;
        return { status: 500, body: { code: 'internal', message: 'no proposal is expected' } };
      },
    });
    await offerWebMcp(page);
    await page.addInitScript(() => window.localStorage.setItem('okf-jawn.webmcp', 'on'));
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const opener = page.getByRole('button', { name: 'Propose a change…' });
    await expect(opener).toBeVisible();
    await expect.poll(async () => (await registrations(page)).live).toEqual(FIVE);

    // prepare_proposal fills the form for the person; nothing is sent.
    expect(
      await runTool(page, 'prepare_proposal', {
        title: 'Publish the budget',
        description: 'The board approved it on Monday.',
        body: '# Meeting notes\n\nThe budget is published.\n',
      }),
    ).toMatchObject({ sent: false, item_id: NOTE, base_revision: HEAD });
    const title = page.getByRole('textbox', { name: 'Title' });
    await expect(title).toHaveValue('Publish the budget');
    await expect(page.getByRole('textbox', { name: 'Why' })).toHaveValue(
      'The board approved it on Monday.',
    );
    await expect(page.getByText('An assistant in this browser filled in this form.')).toBeVisible();
    await opener.focus();
    await tabTo(page, title, 5);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a proposal an assistant prepared');
    await shot(page, 'webmcp-05-prepared-proposal');
    expect(proposed).toBe(0);

    // open_comparison opens the Changes between two exact revisions.
    expect(await runTool(page, 'open_comparison', { from: OLD })).toMatchObject({
      from: OLD,
      to: HEAD,
      changed_files: 1,
    });
    const comparison = page.getByRole('region', { name: 'Comparison' });
    await expect(comparison.getByRole('region', { name: 'What changed' })).toBeVisible();
    await expect(comparison).toContainText(`as of revision ${OLD.slice(0, 10)}`);
    await expect(page).toHaveURL(new RegExp(`/w/${WS}/compare\\?`));
    await expect.poll(() => focused(page)).not.toBe('body');
    await expectNoAxeViolations(page, 'a comparison an assistant opened');
    await shot(page, 'webmcp-06-comparison');
    expect(proposed).toBe(0);
  });
});
