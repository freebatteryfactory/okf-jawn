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
  focused,
  HEAD,
  NOTE,
  note,
  ok,
  PLAN,
  plan,
  readAnswer,
  routeApi,
  useExplorerServer,
  WS,
} from './support/explorer';

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
    // While the page loads, the button says so and keeps focus (a disabled button would not).
    const loading = page.getByRole('button', { name: 'Loading more results…' });
    await expect(loading).toBeFocused();
    answerMore();
    await expect(page.getByRole('alert')).toContainText('Loading more results');
    await expect(page.getByRole('button', { name: 'Show more results' })).toBeFocused();
    expect(await focused(page)).not.toBe('body');
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
});
