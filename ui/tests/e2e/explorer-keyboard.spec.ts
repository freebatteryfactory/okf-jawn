/**
 * The Explorer from the keyboard alone, in Chromium and WebKit: where focus goes after closing a
 * tab or pane, submitting a search and loading more (it never drops to the page body), that Tab
 * and Shift+Tab traverse the whole page with no trap, that separators resize from the keyboard,
 * and that the focus ring keeps 3:1 against a selected row (WCAG 1.4.11). These are the
 * reviewer's browser probes E1-E5, E7, E8 and E10-E12, adopted.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { expect, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  focused,
  HEAD,
  NOTE,
  note,
  ok,
  PLAN,
  plan,
  readAnswer,
  routeApi,
  shot,
  summary,
  useExplorerServer,
  WS,
  workspace,
} from './support/explorer';

const NEW = 'e'.repeat(40);
const OLD = 'b'.repeat(40);

/** The window regains focus, as TanStack Query's focus manager hears it. */
async function windowRefocus(page: Page) {
  await page.evaluate(() => window.dispatchEvent(new Event('visibilitychange')));
}

async function treeScrollTop(page: Page): Promise<number> {
  return page.evaluate(() => {
    const scroller = document.querySelector('[role="tree"] > div');
    return scroller instanceof HTMLElement ? scroller.scrollTop : -1;
  });
}

/** open_workspace answers HEAD when the workspace opens and NEW from then on. */
function headMovesOn() {
  let opens = 0;
  return () => {
    opens += 1;
    return ok({ ...workspace, head: opens <= 1 ? HEAD : NEW });
  };
}

/** A promise the test resolves, to hold an answer back. */
function held(): { wait: Promise<void>; release: () => void } {
  let release: () => void = () => {};
  const wait = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { wait, release };
}

/** A folder of 80 entries: taller than the column, so the tree scrolls. */
const items = Array.from({ length: 80 }, (_, index) =>
  summary(
    `${String(index).padStart(8, '0')}-1111-4111-8111-111111111111`,
    `item-${index}.md`,
    `Item number ${index}`,
    'note',
  ),
);

/** Roll the mouse wheel over the tree until its first or last entry is on screen. */
async function wheelTree(page: Page, to: 'top' | 'bottom') {
  const tree = page.getByRole('tree', { name: 'Folders' });
  await tree.hover();
  await page.mouse.wheel(0, to === 'top' ? -20_000 : 20_000);
  const end = to === 'top' ? 'Item number 0,' : 'Item number 79,';
  await expect(tree.getByRole('treeitem', { name: new RegExp(end) })).toBeInViewport();
}

const row = (page: Page, name: RegExp) =>
  page.getByRole('tree', { name: 'Folders' }).getByRole('treeitem', { name });

function luminance(rgb: number[]): number {
  const [r, g, b] = rgb.map((value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
}

function contrast(a: number[], b: number[]): number {
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((high ?? 0) + 0.05) / ((low ?? 0) + 0.05);
}

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

test.describe('Explorer from the keyboard', () => {
  const server = useExplorerServer();

  async function openTwoTabs(page: Page) {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByRole('tab', { name: 'Meeting notes', selected: true })).toBeVisible();
    const tree = page.getByRole('tree', { name: 'Folders' });
    await tree.getByRole('treeitem', { name: /Plan/ }).click();
    await expect(page.getByRole('tab', { name: 'Plan', selected: true })).toBeVisible();
  }

  test('E1 Delete closes a tab and focus moves to the following tab (APG tabs)', async ({
    page,
  }) => {
    await openTwoTabs(page);
    await page.getByRole('tab', { name: 'Plan' }).focus();
    await page.keyboard.press('Delete');
    await expect(page.getByRole('tab', { name: 'Plan' })).toHaveCount(0);
    await expect(page.getByRole('tab', { name: 'Meeting notes' })).toBeFocused();
  });

  test('E2 "Close tab" on the last tab moves focus to the empty documents area', async ({
    page,
  }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByRole('tab', { name: 'Meeting notes', selected: true })).toBeVisible();
    await page.getByRole('button', { name: 'Close tab' }).focus();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/w/${WS}$`));
    const empty = page.getByText('Open an item from the folders or the search results.');
    await expect(empty).toBeVisible();
    await expect(empty).toBeFocused();
  });

  test("E3 closing the only tab of a split pane moves focus to the neighbouring pane's tab", async ({
    page,
  }) => {
    await openTwoTabs(page);
    await page.getByRole('button', { name: 'Split right' }).click();
    const second = page.getByRole('region', { name: 'Documents, pane 2 of 2' });
    await expect(second).toBeVisible();
    await page.keyboard.press('Shift');
    await second.getByRole('button', { name: 'Close tab' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('region', { name: 'Documents, pane 2 of 2' })).toHaveCount(0);
    await expect(page.getByRole('tab', { name: 'Plan', selected: true })).toBeFocused();
  });

  test('E4 Tab and Shift+Tab traverse the whole Explorer with no trap', async ({ page }) => {
    await openTwoTabs(page);
    await page.locator('body').click({ position: { x: 1, y: 1 } });
    const forward: string[] = [];
    for (let index = 0; index < 45; index += 1) {
      await page.keyboard.press('Tab');
      forward.push(await focused(page));
    }
    const backward: string[] = [];
    for (let index = 0; index < 45; index += 1) {
      await page.keyboard.press('Shift+Tab');
      backward.push(await focused(page));
    }
    for (const stop of [
      'input:',
      'treeitem:',
      'tab:',
      'separator:Resize the navigation column',
      'separator:Resize the source panel',
    ]) {
      expect(forward.some((entry) => entry.startsWith(stop) || entry.includes(stop))).toBe(true);
      expect(backward.some((entry) => entry.startsWith(stop) || entry.includes(stop))).toBe(true);
    }
  });

  test('E5 separators resize from the keyboard', async ({ page }) => {
    await openTwoTabs(page);
    const separator = page.getByRole('separator', { name: 'Resize the navigation column' });
    await page.keyboard.press('Shift');
    await separator.focus();
    const before = await separator.getAttribute('aria-valuenow');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
    expect(await separator.getAttribute('aria-valuenow')).not.toBe(before);
    expect(await separator.getAttribute('aria-orientation')).toBe('vertical');
  });

  test('E7 the focus ring on a selected tree row keeps 3:1 against the row (WCAG 1.4.11)', async ({
    page,
  }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const tree = page.getByRole('tree', { name: 'Folders' });
    const row = tree.getByRole('treeitem', { name: /Meeting notes/ });
    await expect(row).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('Shift');
    await row.focus();
    const colours = await row.evaluate((element) => {
      const style = getComputedStyle(element);
      const canvas = document.createElement('canvas');
      canvas.width = 1;
      canvas.height = 1;
      const context = canvas.getContext('2d');
      const resolve = (colour: string) => {
        if (!context) return [0, 0, 0];
        context.fillStyle = '#ffffff';
        context.fillRect(0, 0, 1, 1);
        context.fillStyle = colour;
        context.fillRect(0, 0, 1, 1);
        return Array.from(context.getImageData(0, 0, 1, 1).data.slice(0, 3));
      };
      return {
        outline: `${style.outlineStyle} ${style.outlineWidth}`,
        ring: resolve(style.outlineColor),
        row: resolve(style.backgroundColor),
      };
    });
    expect(colours.outline.startsWith('none')).toBe(false);
    expect(contrast(colours.ring, colours.row)).toBeGreaterThanOrEqual(3);
  });

  test('E8 keyboard alone: search keeps focus in the box, then opens a result', async ({
    page,
    browserName,
  }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(page.getByRole('tree', { name: 'Folders' })).toBeVisible();
    const box = page.getByRole('searchbox', { name: 'Search this workspace' });
    let guard = 0;
    while (!(await focused(page)).startsWith('input') && guard < 10) {
      await page.keyboard.press('Tab');
      guard += 1;
    }
    await expect(box).toBeFocused();
    await page.keyboard.type('budget');
    await page.keyboard.press('Enter');
    const results = page.getByRole('region', { name: 'Results for “budget”' });
    await expect(results.getByRole('link', { name: /Meeting notes/ })).toBeVisible();
    await expect(results.getByRole('status')).toHaveText('1 result shown.');
    await expect(box).toBeFocused();
    const result = results.getByRole('link', { name: /Meeting notes/ });
    if (browserName === 'webkit') {
      // WebKit, like Safari by default, leaves links out of Tab order (a platform setting the
      // page cannot change), so the result is focused directly; Enter still opens it.
      await result.focus();
    } else {
      guard = 0;
      while (!(await focused(page)).includes('Meeting notes') && guard < 10) {
        await page.keyboard.press('Tab');
        guard += 1;
      }
    }
    await expect(result).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByText('Showing line 3.')).toBeVisible();
  });

  test('E10 "Continue reading" moves focus to the part it loaded, even the last one', async ({
    page,
  }) => {
    await routeApi(page, {
      '/api/reads/read-item': (body) =>
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
        ),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await page.getByRole('button', { name: 'Continue reading' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText('Part two')).toBeVisible();
    await expect(page.getByRole('group', { name: 'Part 2 of this reading' })).toBeFocused();
  });

  test('E11 "Show more results" moves focus to the first new result, even on the last page', async ({
    page,
  }) => {
    await routeApi(page, {
      '/api/search/search-items': (body) =>
        (body.page as { cursor: string | null }).cursor === 'more'
          ? ok({ revision: HEAD, hits: [hitOf(PLAN, 'Plan')] })
          : ok({ revision: HEAD, hits: [hitOf(NOTE, 'Meeting notes')], next_cursor: 'more' }),
    });
    await page.goto(`${server.origin()}/w/${WS}?q=budget`);
    await page.getByRole('button', { name: 'Show more results' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('link', { name: /Plan/ })).toBeFocused();
  });

  test('E11b "Show more results" keeps focus while loading and when the next page fails', async ({
    page,
  }) => {
    let answerMore: () => void = () => {};
    const moreAsked = new Promise<void>((resolve) => {
      answerMore = resolve;
    });
    await routeApi(page, {
      '/api/search/search-items': async (body) => {
        if ((body.page as { cursor: string | null }).cursor !== 'more') {
          return ok({ revision: HEAD, hits: [hitOf(NOTE, 'Meeting notes')], next_cursor: 'more' });
        }
        await moreAsked;
        return { status: 500, body: { code: 'internal', message: 'index stumbled' } };
      },
    });
    await page.goto(`${server.origin()}/w/${WS}?q=budget`);
    await page.getByRole('button', { name: 'Show more results' }).focus();
    await page.keyboard.press('Enter');
    // While the page loads, the button says so and keeps focus. A disabled button would not:
    // Chromium and WebKit move focus to the body within a frame of a focused button becoming
    // disabled, so focus is read only after several frames, not at the first poll.
    const loading = page.getByRole('button', { name: 'Loading more results…' });
    await expect(loading).toBeVisible();
    await page.waitForTimeout(300);
    expect(await focused(page)).toContain('Loading more results');
    answerMore();
    await expect(page.getByRole('alert')).toContainText('Loading more results');
    await expect(page.getByRole('button', { name: 'Show more results' })).toBeFocused();
    expect(await focused(page)).not.toBe('body');
  });

  test('Q5b "Show more results" keeps focus through a slow load, checked after the focus fixup', async ({
    page,
  }) => {
    const more = held();
    await routeApi(page, {
      '/api/search/search-items': async (body) => {
        if ((body.page as { cursor: string | null }).cursor !== 'more') {
          return ok({ revision: HEAD, hits: [hitOf(NOTE, 'Meeting notes')], next_cursor: 'more' });
        }
        await more.wait;
        return ok({ revision: HEAD, hits: [] });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}?q=budget`);
    await page.getByRole('button', { name: 'Show more results' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Loading more results…' })).toBeVisible();
    await page.waitForTimeout(300);
    const during = await focused(page);
    more.release();
    expect(during).toContain('Loading more results');
  });

  test('E12 "Show more entries" in the tree moves focus to the first new entry', async ({
    page,
  }) => {
    await routeApi(page, {
      '/api/items/list-items': (body) =>
        (body.page as { cursor: string | null }).cursor === 'more'
          ? ok({ folders: [], items: [plan], revision: HEAD })
          : ok({ folders: [], items: [note], revision: HEAD, next_cursor: 'more' }),
    });
    await page.goto(`${server.origin()}/w/${WS}`);
    const tree = page.getByRole('tree', { name: 'Folders' });
    await expect(tree.getByRole('treeitem', { name: 'Show more entries' })).toBeVisible();
    await page.keyboard.press('Shift');
    await tree.focus();
    await page.keyboard.press('End');
    await expect(tree.getByRole('treeitem', { name: 'Show more entries' })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(tree.getByRole('treeitem', { name: /Plan/ })).toBeFocused();
  });

  test('a press that makes "Split right" unavailable keeps focus on it', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByRole('tab', { name: 'Meeting notes', selected: true })).toBeVisible();
    const split = page.getByRole('button', { name: 'Split right' }).first();
    await split.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('region', { name: /pane 2 of 2/ })).toBeVisible();
    // Focus is still on the first pane's button; the third pane makes it unavailable.
    await page.keyboard.press('Enter');
    await expect(page.getByRole('region', { name: /pane 3 of 3/ })).toBeVisible();
    await page.waitForTimeout(300);
    expect(await focused(page)).toBe('button:Split right');
    await expect(page.locator(':focus')).toHaveAttribute('aria-disabled', 'true');
    await page.keyboard.press('Enter');
    await expect(page.getByRole('region', { name: /pane 4 of/ })).toHaveCount(0);
    await expectNoAxeViolations(page, 'three panes, "Split right" unavailable');
  });

  test('going back to no item makes "Close tab" unavailable and keeps focus on it', async ({
    page,
  }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}`);
    await row(page, /^notes$/).click();
    await row(page, /Meeting notes/).click();
    await expect(page.getByRole('tab', { name: 'Meeting notes', selected: true })).toBeVisible();
    const close = page.getByRole('button', { name: 'Close tab' });
    await close.focus();
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`/w/${WS}$`));
    await expect(close).toHaveAttribute('aria-disabled', 'true');
    await page.waitForTimeout(300);
    expect(await focused(page)).toBe('button:Close tab');
  });

  test('"Show more workspaces" keeps focus while it loads, then moves to the first new one', async ({
    page,
  }) => {
    const more = held();
    const second = { ...workspace, id: '77777777-7777-4777-8777-777777777777', name: 'Second' };
    await routeApi(page, {
      '/api/workspaces/list-workspaces': async (body) => {
        if ((body.page as { cursor?: string | null }).cursor !== 'more') {
          return ok({ items: [workspace], next_cursor: 'more' });
        }
        await more.wait;
        return ok({ items: [second], next_cursor: null });
      },
    });
    await page.goto(`${server.origin()}/`);
    await page.getByRole('button', { name: 'Show more workspaces' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Loading more workspaces…' })).toBeVisible();
    await page.waitForTimeout(300);
    const during = await focused(page);
    more.release();
    expect(during).toBe('button:Loading more workspaces…');
    await expect(page.getByRole('link', { name: 'Second' })).toBeFocused();
    await expectNoAxeViolations(page, 'more workspaces');
  });

  test('Q2a the tree wheeled far from its tab-stop row: axe, and the keyboard lands on a row', async ({
    page,
  }) => {
    await routeApi(page, {
      '/api/items/list-items': () => ok({ folders: [], items, revision: HEAD }),
    });
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(row(page, /Item number 0,/)).toBeVisible();
    await wheelTree(page, 'bottom');
    await expectNoAxeViolations(page, 'tree wheeled away from its tab stop');
    // Shift+Tab from after the tree: the tab stop (the first entry) is scrolled back and focused.
    await page.getByRole('separator', { name: 'Resize the navigation column' }).focus();
    await page.keyboard.press('Shift+Tab');
    await expect(row(page, /Item number 0,/)).toBeFocused();
    await expect(row(page, /Item number 0,/)).toBeInViewport();
    await page.keyboard.press('ArrowDown');
    await expect(row(page, /Item number 1,/)).toBeFocused();
    // Tab from before the tree, wheeled away again: the tab stop again (nothing is selected, so
    // the WAI-ARIA tree pattern says the first entry), scrolled back into view.
    await wheelTree(page, 'bottom');
    await expectNoAxeViolations(page, 'tree wheeled away again');
    await page.getByRole('button', { name: 'Search', exact: true }).focus();
    await page.keyboard.press('Tab');
    await expect(row(page, /Item number 0,/)).toBeFocused();
    await expect(row(page, /Item number 0,/)).toBeInViewport();
    await page.keyboard.press('ArrowDown');
    await expect(row(page, /Item number 1,/)).toBeFocused();
  });

  test('Q2b the selected row deep in the list, wheeled to the top: axe, and Shift+Tab lands on it', async ({
    page,
  }) => {
    const item70 = items[70];
    if (item70 === undefined) throw new Error('no 71st entry');
    await routeApi(page, {
      '/api/items/list-items': () => ok({ folders: [], items, revision: HEAD }),
      '/api/items/get-item': () =>
        ok({ summary: item70, body: 'x', content_digest: 'c'.repeat(64), properties: {} }),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${item70.id}`);
    await expect(row(page, /Item number 70,/)).toHaveAttribute('aria-selected', 'true');
    await wheelTree(page, 'top');
    await expectNoAxeViolations(page, 'selected row wheeled out of the rendered window');
    await page.getByRole('separator', { name: 'Resize the navigation column' }).focus();
    await page.keyboard.press('Shift+Tab');
    await expect(row(page, /Item number 70,/)).toBeFocused();
    await expect(row(page, /Item number 70,/)).toBeInViewport();
    await page.keyboard.press('ArrowUp');
    await expect(row(page, /Item number 69,/)).toBeFocused();
  });

  test('Q2c Shift+Tab onto the selected row: arrows, End, Home, type-ahead and Enter work from it', async ({
    page,
  }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(row(page, /Meeting notes/)).toHaveAttribute('aria-selected', 'true');
    await page.getByRole('separator', { name: 'Resize the navigation column' }).focus();
    await page.keyboard.press('Shift+Tab');
    await expect(row(page, /Meeting notes/)).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(row(page, /^Plan,/)).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(row(page, /^Annual report 2025,/)).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await expect(row(page, /^Plan,/)).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(row(page, /^notes$/)).toBeFocused();
    await page.keyboard.press('End');
    await expect(row(page, /^Annual report 2025,/)).toBeFocused();
    await page.keyboard.press('Home');
    await expect(row(page, /^notes$/)).toBeFocused();
    await page.keyboard.press('p');
    await expect(row(page, /^Plan,/)).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('tab', { name: 'Plan', selected: true })).toBeVisible();
  });
});

test.describe('a newer head moves nothing until Refresh', () => {
  const server = useExplorerServer();

  test('Q3a window focus with a newer head: nothing reloads, a notice appears, and Refresh keeps the tree’s place and the focus', async ({
    page,
  }) => {
    const listed: string[] = [];
    const newer = held();
    await routeApi(page, {
      '/api/workspaces/open-workspace': headMovesOn(),
      '/api/items/list-items': async (body) => {
        const revision = String((body.at as { revision: string }).revision);
        listed.push(revision);
        if (revision === NEW) await newer.wait;
        return ok({ folders: [], items, revision });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await expect(page.getByRole('tab', { selected: true })).toBeVisible();
    const tree = page.getByRole('tree', { name: 'Folders' });
    await expect(row(page, /Item number 0,/)).toBeVisible();
    await page.keyboard.press('Shift');
    await tree.focus();
    for (let index = 0; index < 40; index += 1) await page.keyboard.press('ArrowDown');
    await expect(row(page, /Item number 40,/)).toBeFocused();
    const scrolled = await treeScrollTop(page);
    expect(scrolled).toBeGreaterThan(0);
    const listedBefore = listed.length;

    await windowRefocus(page);
    const notice = page
      .getByRole('status')
      .filter({ hasText: 'A newer version of this workspace exists' });
    await expect(notice).toBeVisible();
    await expectNoAxeViolations(page, 'newer-head notice');
    await shot(page, '09-newer-head-notice');
    await page.waitForTimeout(300);
    expect(listed.length).toBe(listedBefore);
    await expect(row(page, /Item number 40,/)).toBeFocused();
    expect(await treeScrollTop(page)).toBe(scrolled);
    expect(page.url()).toContain(`rev=${OLD}`);

    // Refresh from the keyboard. While the newer listing loads, the tree keeps the one shown.
    const refresh = page.getByRole('button', { name: /^Refresh/ });
    await refresh.focus();
    await page.keyboard.press('Enter');
    await expect.poll(() => listed.includes(NEW)).toBe(true);
    await expect(row(page, /Item number 40,/)).toBeVisible();
    await expect(tree.getByRole('treeitem', { name: /Loading/ })).toHaveCount(0);
    // No status row is added below the 80 entries either (it would be out of the rendered window).
    await expect(row(page, /Item number 40,/)).toHaveAttribute('aria-setsize', '80');
    newer.release();
    await expect(page.getByText(/Showing the workspace as of/).locator('code')).toHaveAttribute(
      'title',
      NEW,
    );
    await expect(notice).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Refresh' })).toBeFocused();
    expect(await treeScrollTop(page)).toBe(scrolled);
    expect(page.url()).toContain(`rev=${OLD}`);
    // Back into the tree from the keyboard: the tab stop, which the WAI-ARIA tree pattern puts on
    // the first entry here, since the open item is not in this folder.
    await page.getByRole('button', { name: 'Search', exact: true }).focus();
    await page.keyboard.press('Tab');
    await expect(row(page, /Item number 0,/)).toBeFocused();
  });

  test('Q3b window focus while reading part 2 of an unpinned item: nothing reloads, and Refresh keeps the focus', async ({
    page,
  }) => {
    const reads: string[] = [];
    await routeApi(page, {
      '/api/workspaces/open-workspace': headMovesOn(),
      '/api/reads/read-item': (body) => {
        const at = String((body.at as { revision: string }).revision);
        const second = body.cursor === 'part-2';
        reads.push(`${at.slice(0, 1)}:${second ? 2 : 1}`);
        return ok(
          readAnswer(note, body, {
            markdown: second ? 'Part two' : `Part one at ${at.slice(0, 1)}`,
            outline: [],
            receipt_id: second
              ? '66666666-6666-4666-8666-666666666666'
              : '55555555-5555-4555-8555-555555555555',
            source: {
              workspace_id: WS,
              item_id: NOTE,
              path: note.path,
              revision: at,
              selection: body.selection,
              locations: [],
            },
            truncated: !second,
            ...(second ? {} : { next_cursor: 'part-2' }),
          }),
        );
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await page.getByRole('button', { name: 'Continue reading' }).focus();
    await page.keyboard.press('Enter');
    const part2 = page.getByRole('group', { name: 'Part 2 of this reading' });
    await expect(part2).toBeFocused();
    const readsBefore = reads.length;

    await windowRefocus(page);
    await expect(
      page.getByRole('status').filter({ hasText: 'A newer version of this workspace exists' }),
    ).toBeVisible();
    await page.waitForTimeout(300);
    expect(reads.length).toBe(readsBefore);
    await expect(part2).toBeFocused();
    await expect(page.getByText('Part two')).toBeVisible();

    // Refresh reads the item at the newer head from its start (a continuation belongs to its
    // revision), and keyboard focus stays on Refresh.
    await page.getByRole('button', { name: 'Refresh' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText('Part one at e')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Refresh' })).toBeFocused();
  });

  test('Q3c window focus with page 2 of search results focused: nothing reloads, and Refresh keeps the focus', async ({
    page,
  }) => {
    const searched: string[] = [];
    const hit = (id: string, title: string, revision: string) => ({
      title,
      snippet: `…${title}…`,
      score: 1,
      source: {
        workspace_id: WS,
        item_id: id,
        path: `notes/${title}.md`,
        revision,
        selection: { kind: 'lines', range: { start: 1, end: 1 } },
        locations: [],
      },
    });
    await routeApi(page, {
      '/api/workspaces/open-workspace': headMovesOn(),
      '/api/search/search-items': (body) => {
        const at = String((body.at as { revision: string }).revision);
        const more = (body.page as { cursor: string | null }).cursor === 'more';
        searched.push(`${at.slice(0, 1)}:${more ? 2 : 1}`);
        return more
          ? ok({ revision: at, hits: [hit(PLAN, 'Second', at)] })
          : ok({
              revision: at,
              hits: [hit(NOTE, `First at ${at.slice(0, 1)}`, at)],
              next_cursor: 'more',
            });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}?q=budget`);
    await page.getByRole('button', { name: 'Show more results' }).focus();
    await page.keyboard.press('Enter');
    const second = page.getByRole('link', { name: /Second/ });
    await expect(second).toBeFocused();
    const searchedBefore = searched.length;

    await windowRefocus(page);
    await expect(
      page.getByRole('status').filter({ hasText: 'A newer version of this workspace exists' }),
    ).toBeVisible();
    await page.waitForTimeout(300);
    expect(searched.length).toBe(searchedBefore);
    await expect(second).toBeFocused();

    // Refresh searches the newer head from its first page, and keyboard focus stays on Refresh.
    await page.getByRole('button', { name: 'Refresh' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('link', { name: /First at e/ })).toBeVisible();
    expect(searched.at(-1)).toBe('e:1');
    await expect(page.getByRole('button', { name: 'Refresh' })).toBeFocused();
  });

  for (const removed of [true, false]) {
    test(`R4b focus on an entry while the newer listing loads, the entry ${removed ? 'removed' : 'kept'} by the newer head: focus never drops to the body`, async ({
      page,
    }) => {
      const newer = held();
      await routeApi(page, {
        '/api/workspaces/open-workspace': headMovesOn(),
        '/api/items/list-items': async (body) => {
          const revision = String((body.at as { revision: string }).revision);
          if (revision === NEW) await newer.wait;
          const shown =
            revision === NEW && removed ? items.filter((_, index) => index !== 40) : items;
          return ok({ folders: [], items: shown, revision });
        },
      });
      await page.goto(`${server.origin()}/w/${WS}`);
      await expect(row(page, /Item number 0,/)).toBeVisible();
      await page.keyboard.press('Shift');
      await page.getByRole('tree', { name: 'Folders' }).focus();
      for (let index = 0; index < 40; index += 1) await page.keyboard.press('ArrowDown');
      await expect(row(page, /Item number 40,/)).toBeFocused();
      await page.getByRole('button', { name: 'Refresh' }).focus();
      await page.keyboard.press('Enter');
      await expect(page.getByText(/Showing the workspace as of/).locator('code')).toHaveAttribute(
        'title',
        NEW,
      );
      // The newer listing is held: the tree still shows the older one. Back onto row 40.
      await page.keyboard.press('Shift');
      await row(page, /Item number 40,/).focus();
      await expect(row(page, /Item number 40,/)).toBeFocused();
      newer.release();
      // Removed: focus moves to the entry now in its place. Kept: it stays.
      await expect(row(page, removed ? /Item number 41,/ : /Item number 40,/)).toBeFocused();
      await page.waitForTimeout(300);
      expect(await focused(page)).toContain(removed ? 'Item number 41' : 'Item number 40');
      await page.keyboard.press('ArrowDown');
      await expect(row(page, removed ? /Item number 42,/ : /Item number 41,/)).toBeFocused();
    });
  }

  test('N20 a failed newer-head check says so quietly, never as an alert', async ({ page }) => {
    let opens = 0;
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => {
        opens += 1;
        return opens <= 1
          ? ok(workspace)
          : { status: 503, body: { code: 'unavailable', message: 'service restarting' } };
      },
    });
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(row(page, /^notes$/)).toBeVisible();
    await windowRefocus(page);
    const failed = page.getByRole('status').filter({
      hasText: 'Could not check for a newer version: The service is temporarily unavailable.',
    });
    await expect(failed).toBeVisible();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.getByText(/Showing the workspace as of/).locator('code')).toHaveAttribute(
      'title',
      HEAD,
    );
    await expectNoAxeViolations(page, 'failed newer-head check');
  });

  test('"Read the latest version" from the keyboard keeps focus at the revision notice', async ({
    page,
  }) => {
    await routeApi(page, {
      '/api/workspaces/open-workspace': headMovesOn(),
      '/api/reads/read-item': (body) => {
        const at = body.at as { kind: string; revision?: string };
        const resolved = at.kind === 'latest' ? NEW : String(at.revision);
        const answer = readAnswer(note, body);
        return ok({ ...answer, source: { ...answer.source, revision: resolved } });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const latest = page.getByRole('button', { name: 'Read the latest version' });
    await latest.focus();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`rev=${NEW}`));
    await expect(latest).toHaveCount(0);
    await expect.poll(() => focused(page)).toContain(`reading revision ${NEW.slice(0, 10)}`);
  });
});
