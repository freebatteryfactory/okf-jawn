/**
 * The Explorer shell in a real browser (Chromium and WebKit, SPEC section 2), keyboard first,
 * with the API answered through `page.route` (no server exists yet in this lane). Every main
 * screen, and the further states a reviewer found (a cited revision, a partial read, a failed
 * folder, more entries, a long title), must have zero axe violations; screenshots go to
 * `.artifacts/ui/` (untracked) for the owner to judge the look.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { expect, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  HEAD,
  NOTE,
  note,
  ok,
  PLAN,
  readAnswer,
  routeApi,
  shot,
  source,
  summary,
  useExplorerServer,
  WS,
} from './support/explorer';

test.describe('Explorer shell', () => {
  const server = useExplorerServer();

  test('lists workspaces and opens one', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/`);
    await expect(page.getByRole('link', { name: 'Workshop notes' })).toBeVisible();
    await expectNoAxeViolations(page, 'workspace list');
    await shot(page, '01-workspaces');
    await page.getByRole('link', { name: 'Workshop notes' }).focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { level: 1, name: 'Workshop notes' })).toBeVisible();
  });

  test('reads an item opened from the tree with the keyboard alone', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}`);
    const tree = page.getByRole('tree', { name: 'Folders' });
    await expect(tree.getByRole('treeitem', { name: 'notes', exact: true })).toBeVisible();
    await expectNoAxeViolations(page, 'workspace');
    await shot(page, '02-workspace-tree');

    await page.getByRole('searchbox', { name: 'Search this workspace' }).focus();
    await page.keyboard.press('Tab'); // the Search button
    await page.keyboard.press('Tab'); // the tree, which focuses its first entry
    await expect(tree.getByRole('treeitem', { name: 'notes', exact: true })).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('ArrowRight');
    await expect(tree.getByRole('treeitem', { name: /Meeting notes/ })).toBeVisible();
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/w/${WS}/i/${NOTE}$`));
    const article = page.getByRole('article', { name: 'Meeting notes' });
    await expect(article.getByText('publish the budget')).toBeVisible();
    await expect(page.getByRole('tab', { name: 'Meeting notes', selected: true })).toBeVisible();
    const sources = page.getByRole('region', { name: 'Source panel' });
    await expect(sources.getByText('Inferred location, not certain:')).toBeVisible();
    await expectNoAxeViolations(page, 'document');
    await shot(page, '03-document-and-sources');
  });

  test('searches, opens a result at its selection, and splits into panes', async ({ page }) => {
    await routeApi(page);
    await page.goto(`${server.origin()}/w/${WS}`);
    const search = page.getByRole('searchbox', { name: 'Search this workspace' });
    await search.fill('budget');
    await search.press('Enter');
    const results = page.getByRole('region', { name: 'Results for “budget”' });
    await expect(results.getByRole('link', { name: /Meeting notes/ })).toBeVisible();
    await expectNoAxeViolations(page, 'search results');
    await shot(page, '04-search-results');

    await results.getByRole('link', { name: /Meeting notes/ }).focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page.getByText('Showing line 3.')).toBeVisible();

    await page.getByRole('button', { name: 'Back to folders' }).click();
    const tree = page.getByRole('tree', { name: 'Folders' });
    // Opening the result revealed it, so its folder is already expanded in the tree.
    await expect(tree.getByRole('treeitem', { name: 'notes', exact: true })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    await tree.getByRole('treeitem', { name: /Plan/ }).click();
    await expect(page.getByRole('tab', { name: 'Plan', selected: true })).toBeVisible();
    await page.getByRole('button', { name: 'Split right' }).click();
    await expect(page.getByRole('region', { name: 'Documents, pane 2 of 2' })).toBeVisible();
    const separator = page.getByRole('separator', { name: 'Resize panes 1 and 2' });
    // After the pointer click above, the separator's own key returns the page to keyboard
    // modality, as a keyboard user resizing would (WebKit ignores a modifier key alone).
    await separator.focus();
    await page.keyboard.press('ArrowLeft');
    await expectVisibleFocus(page);
    await page.getByRole('tab', { name: 'Plan' }).first().focus();
    await page.keyboard.press('ArrowLeft');
    await expect(page.getByRole('tab', { name: 'Meeting notes' })).toBeFocused();
    await expectNoAxeViolations(page, 'split panes');
    await shot(page, '05-split-panes');
  });

  test('shows an unbuilt operation as not available', async ({ page }) => {
    await routeApi(page, {
      '/api/reads/read-item': () => ({
        status: 501,
        body: { code: 'not_implemented', message: 'read_item is not implemented yet' },
      }),
      '/api/reads/get-sources': () => ({
        status: 501,
        body: { code: 'not_implemented', message: 'get_sources is not implemented yet' },
      }),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(
      page
        .getByRole('alert')
        .filter({ hasText: 'Reading this item: Not available in this build.' }),
    ).toBeVisible();
    await expect(
      page
        .getByRole('alert')
        .filter({ hasText: 'Listing this item’s sources: Not available in this build.' }),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'not available');
    await shot(page, '06-not-available');
  });

  test('E6 axe on further states: cited revision, partial read, failed folder, more entries', async ({
    page,
  }) => {
    const cited = 'b'.repeat(40);
    await routeApi(page, {
      '/api/items/list-items': (body) =>
        body.folder === 'notes'
          ? { status: 500, body: { code: 'internal', message: 'storage stumbled' } }
          : ok({ folders: ['notes'], items: [source], revision: HEAD, next_cursor: 'more' }),
      '/api/reads/read-item': (body) =>
        ok(
          readAnswer(note, body, {
            markdown: 'Part one',
            outline: [],
            source: {
              workspace_id: WS,
              item_id: NOTE,
              path: 'notes/meeting.md',
              revision: cited,
              selection: body.selection,
              locations: [],
            },
            truncated: true,
            next_cursor: 'part-2',
            warnings: [{ code: 'w', message: 'A table was simplified', location: 'page 2' }],
          }),
        ),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}?rev=${cited}`);
    await expect(page.getByText(/it is not the latest version/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Continue reading' })).toBeVisible();
    const tree = page.getByRole('tree', { name: 'Folders' });
    // The deep link reveals the item, so its folder is already expanded (and its listing failed).
    await expect(tree.getByRole('treeitem', { name: /Could not list this folder/ })).toBeVisible();
    await expect(tree.getByRole('treeitem', { name: 'Show more entries' })).toBeVisible();
    await expectNoAxeViolations(page, 'further states');
    await shot(page, '07-further-states');
  });

  test('E9 axe: a tree holding an ordinary long document title', async ({ page }) => {
    const title = 'Minutes of the October workshop on source provenance and naming rules';
    const long = summary(PLAN, 'notes/minutes.md', title, 'meeting-note');
    await routeApi(page, {
      '/api/items/list-items': (body) =>
        body.folder === 'notes'
          ? ok({ folders: [], items: [note, long], revision: HEAD })
          : ok({ folders: ['notes'], items: [source], revision: HEAD }),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const tree = page.getByRole('tree', { name: 'Folders' });
    const row = tree.getByRole('treeitem', { name: new RegExp(title) });
    await expect(row).toBeVisible();
    // The whole name is on screen, wrapped inside the column, and nothing scrolls sideways.
    await expect(row).toContainText(title);
    await expect(row.getByText('meeting-note')).toBeInViewport();
    const sideways = await tree.evaluate((element) =>
      Array.from(element.querySelectorAll('*')).some(
        (child) => child.scrollWidth > child.clientWidth + 1 && child.clientWidth > 0,
      ),
    );
    expect(sideways).toBe(false);
    await shot(page, '08-long-title');
    await expectNoAxeViolations(page, 'long title');
  });

  test('axe: a folder long enough that the tree scrolls, reached and moved through by keyboard', async ({
    page,
  }) => {
    const items = Array.from({ length: 80 }, (_, index) =>
      summary(
        `${String(index).padStart(8, '0')}-1111-4111-8111-111111111111`,
        `item-${index}.md`,
        `Item number ${index}`,
        'note',
      ),
    );
    await routeApi(page, {
      '/api/items/list-items': () => ok({ folders: [], items, revision: HEAD }),
    });
    await page.goto(`${server.origin()}/w/${WS}`);
    const tree = page.getByRole('tree', { name: 'Folders' });
    await expect(tree.getByRole('treeitem', { name: /Item number 0,/ })).toBeVisible();
    await expectNoAxeViolations(page, 'scrolling tree');
    // Shift+Tab back into the tree from the separator lands on a row, in one stop.
    await page.getByRole('separator', { name: 'Resize the navigation column' }).focus();
    await page.keyboard.press('Shift+Tab');
    await expect(tree.getByRole('treeitem', { name: /Item number 0,/ })).toBeFocused();
    await page.keyboard.press('End');
    await expect(tree.getByRole('treeitem', { name: /Item number 79,/ })).toBeFocused();
    await expect(tree.getByRole('treeitem', { name: /Item number 79,/ })).toBeInViewport();
    await expectNoAxeViolations(page, 'scrolled tree');
  });
});
