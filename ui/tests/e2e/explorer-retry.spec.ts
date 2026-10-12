/**
 * A failure notice's retry from the keyboard, in Chromium and WebKit: pressing the retry button
 * never unmounts it, so focus never drops to the page body. While the operation is asked again the
 * notice stays, its button reads "Trying again…" and keeps focus, and a polite status says so;
 * then focus is on what the retry loaded, or still on the retry button when it failed again.
 * One case per place a first load or a further page can fail. Focus is read 300 ms into each
 * state, after the frame in which a browser moves focus off a control that was removed or disabled.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  type Answers,
  answers,
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  NOTE,
  note,
  ok,
  PLAN,
  type Reply,
  readAnswer,
  routeApi,
  shot,
  useExplorerServer,
  WS,
  workspace,
} from './support/explorer';

type Body = Record<string, unknown>;
type Answer = Answers[string];
type Outcome = 'answers' | 'fails again';

const failed = (message: string): Reply => ({
  status: 500,
  body: { code: 'internal', message },
});

const cursorOf = (body: Body): unknown =>
  typeof body.page === 'object' && body.page !== null && 'cursor' in body.page
    ? body.page.cursor
    : null;

function hitOf(id: string, title: string) {
  return {
    title,
    snippet: `…${title}…`,
    score: 1,
    source: {
      workspace_id: WS,
      item_id: id,
      path: `notes/${title}.md`,
      revision: HEAD,
      selection: { kind: 'lines', range: { start: 1, end: 1 } },
      locations: [],
    },
  };
}

/**
 * The operation a case studies: its requests fail until the test starts holding them, then each
 * waits for the test to say how it ends. Other requests on the same path get the good answer.
 */
function studied(good: Answer, isStudied: (body: Body) => boolean = () => true) {
  let holding = false;
  const waiting: Array<(outcome: Outcome) => void> = [];
  const answer: Answer = async (body) => {
    if (!isStudied(body)) return good(body);
    if (!holding) return failed('storage stumbled');
    const outcome = await new Promise<Outcome>((resolve) => {
      waiting.push(resolve);
    });
    return outcome === 'answers' ? good(body) : failed('storage stumbled again');
  };
  return {
    answer,
    hold: () => {
      holding = true;
    },
    inFlight: () => waiting.length,
    end: (outcome: Outcome) => {
      const give = waiting.shift();
      if (give === undefined) throw new Error('no request of the studied operation is in flight');
      give(outcome);
    },
  };
}

interface Caller {
  /** The place the notice is shown. */
  name: string;
  url: string;
  /** The path of the operation that fails. */
  path: string;
  /** What it answers when it works; the shared workspace's own answer, unless given. */
  good?: Answer;
  /** Which requests on that path are the failing operation's (all of them, unless given). */
  isStudied?: (body: Body) => boolean;
  /** What the person does to meet the failure, when opening the page is not enough. */
  reach?: (page: Page) => Promise<void>;
  /** The notice's words for what was being done. */
  context: string;
  /** Reach the retry button with Tab alone, from the top of the page. */
  byTab?: boolean;
  /** The element keyboard focus is on after the retry loaded the content. */
  loaded: (page: Page) => Locator;
}

const press = async (page: Page, name: string) => {
  await page.getByRole('button', { name }).focus();
  await page.keyboard.press('Enter');
};

const partial = (body: Body) =>
  ok(
    readAnswer(note, body, {
      markdown: body.cursor === 'part-2' ? 'Part two' : 'Part one',
      outline: [],
      receipt_id:
        body.cursor === 'part-2'
          ? '66666666-6666-4666-8666-666666666666'
          : '55555555-5555-4555-8555-555555555555',
      truncated: body.cursor !== 'part-2',
      ...(body.cursor === 'part-2' ? {} : { next_cursor: 'part-2' }),
    }),
  );

const callers: Caller[] = [
  {
    name: 'the workspace list',
    url: '/',
    path: '/api/workspaces/list-workspaces',
    context: 'Listing your workspaces',
    byTab: true,
    loaded: (page) => page.getByRole('link', { name: 'Workshop notes' }),
  },
  {
    name: 'more workspaces',
    url: '/',
    path: '/api/workspaces/list-workspaces',
    good: (body) =>
      cursorOf(body) === 'more'
        ? ok({
            items: [{ ...workspace, id: '77777777-7777-4777-8777-777777777777', name: 'Second' }],
            next_cursor: null,
          })
        : ok({ items: [workspace], next_cursor: 'more' }),
    isStudied: (body) => cursorOf(body) === 'more',
    reach: (page) => press(page, 'Show more workspaces'),
    context: 'Listing more workspaces',
    loaded: (page) => page.getByRole('link', { name: 'Second' }),
  },
  {
    name: 'opening a workspace',
    url: `/w/${WS}`,
    path: '/api/workspaces/open-workspace',
    context: 'Opening this workspace',
    loaded: (page) => page.getByRole('heading', { level: 1, name: 'Workshop notes' }),
  },
  {
    name: 'the tree’s root listing',
    url: `/w/${WS}`,
    path: '/api/items/list-items',
    context: 'Listing this workspace',
    loaded: (page) =>
      page
        .getByRole('tree', { name: 'Folders' })
        .getByRole('treeitem', { name: 'notes', exact: true }),
  },
  {
    name: 'the source panel',
    url: `/w/${WS}/i/${NOTE}`,
    path: '/api/reads/get-sources',
    context: 'Listing this item’s sources',
    loaded: (page) =>
      page
        .getByRole('region', { name: 'Source panel' })
        .getByRole('heading', { level: 2, name: 'Sources' }),
  },
  {
    name: 'search',
    url: `/w/${WS}?q=budget`,
    path: '/api/search/search-items',
    context: 'Searching',
    loaded: (page) => page.getByRole('link', { name: /Meeting notes/ }),
  },
  {
    name: 'more search results',
    url: `/w/${WS}?q=budget`,
    path: '/api/search/search-items',
    good: (body) =>
      cursorOf(body) === 'more'
        ? ok({ revision: HEAD, hits: [hitOf(PLAN, 'Plan')] })
        : ok({ revision: HEAD, hits: [hitOf(NOTE, 'Meeting notes')], next_cursor: 'more' }),
    isStudied: (body) => cursorOf(body) === 'more',
    reach: (page) => press(page, 'Show more results'),
    context: 'Loading more results',
    loaded: (page) => page.getByRole('link', { name: /Plan/ }),
  },
  {
    name: 'the reader’s properties',
    url: `/w/${WS}/i/${NOTE}`,
    path: '/api/items/get-item',
    context: 'Reading this item’s properties',
    loaded: (page) => page.locator('summary').filter({ hasText: /^Properties \(\d+\)$/ }),
  },
  {
    name: 'the reader',
    url: `/w/${WS}/i/${NOTE}`,
    path: '/api/reads/read-item',
    context: 'Reading this item',
    loaded: (page) => page.locator('[data-part]').filter({ hasText: 'publish the budget' }),
  },
  {
    name: 'the reader’s continuation',
    url: `/w/${WS}/i/${NOTE}`,
    path: '/api/reads/read-item',
    good: partial,
    isStudied: (body) => body.cursor === 'part-2',
    reach: (page) => press(page, 'Continue reading'),
    context: 'Continuing this reading',
    loaded: (page) => page.getByRole('group', { name: 'Part 2 of this reading' }),
  },
];

test.describe('a failure notice’s retry from the keyboard', () => {
  const server = useExplorerServer();

  /** Open the caller's page, meet its failure, and press its retry button from the keyboard. */
  async function retryFromTheKeyboard(page: Page, caller: Caller) {
    const good = caller.good ?? answers()[caller.path];
    if (good === undefined) throw new Error(`no answer for ${caller.path}`);
    const operation = studied(good, caller.isStudied);
    await routeApi(page, { [caller.path]: operation.answer });
    await page.goto(`${server.origin()}${caller.url}`);
    await caller.reach?.(page);
    const alert = page
      .getByRole('alert')
      .filter({ hasText: `${caller.context}: Something went wrong in the service.` });
    await expect(alert).toBeVisible();
    const notice = alert.locator('xpath=..');
    const retry = notice.getByRole('button');
    await expect(retry).toHaveText('Try again');
    operation.hold();
    if (caller.byTab) {
      await page.locator('body').click({ position: { x: 1, y: 1 } });
      let presses = 0;
      while ((await focused(page)) !== 'button:Try again' && presses < 10) {
        await page.keyboard.press('Tab');
        presses += 1;
      }
    } else await retry.focus();
    await expect(retry).toBeFocused();
    await page.keyboard.press('Enter');

    // In flight: the same notice, its button still focused, and the retry said politely.
    await expect(retry).toHaveText('Trying again…');
    await expect.poll(() => operation.inFlight()).toBe(1);
    await page.waitForTimeout(300);
    expect(await focused(page)).toBe('button:Trying again…');
    await expect(retry).toHaveAttribute('aria-disabled', 'true');
    await expect(notice.getByRole('status')).toHaveText(`${caller.context}: trying again…`);
    await expect(alert).toBeVisible();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, `${caller.name}, trying again`);
    return { operation, alert, notice, retry };
  }

  for (const caller of callers) {
    test(`${caller.name}: a retry that succeeds ends with focus on what it loaded`, async ({
      page,
    }) => {
      const { operation, notice } = await retryFromTheKeyboard(page, caller);
      if (caller.name === 'the reader') await shot(page, '10-trying-again');
      operation.end('answers');
      const loaded = caller.loaded(page);
      await expect(loaded).toBeFocused();
      await expect(notice).toHaveCount(0);
      await page.waitForTimeout(300);
      expect(await focused(page)).not.toBe('body');
      await expect(loaded).toBeFocused();
      await expectVisibleFocus(page);
      await expectNoAxeViolations(page, `${caller.name}, loaded by a retry`);
    });

    test(`${caller.name}: a retry that fails again keeps focus on the retry button`, async ({
      page,
    }) => {
      const { operation, alert, notice, retry } = await retryFromTheKeyboard(page, caller);
      operation.end('fails again');
      await expect(alert).toContainText('storage stumbled again');
      await expect(retry).toHaveText('Try again');
      await page.waitForTimeout(300);
      expect(await focused(page)).toBe('button:Try again');
      await expect(retry).toBeFocused();
      await expect(retry).toHaveAttribute('aria-disabled', 'false');
      await expect(notice.getByRole('status')).toHaveText('');
      await expectNoAxeViolations(page, `${caller.name}, failed again`);
      // It can be tried again from where focus is.
      await page.keyboard.press('Enter');
      await expect(retry).toHaveText('Trying again…');
      await expect.poll(() => operation.inFlight()).toBe(1);
      operation.end('answers');
      await expect(caller.loaded(page)).toBeFocused();
    });
  }
});
