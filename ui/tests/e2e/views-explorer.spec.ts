/**
 * Views in the Explorer, in Chromium and WebKit (M4b B1 and B2): a saved View opened in its tab,
 * with its table, its chart's data table, its text one step away and a cited source opened at the
 * revision the View pins; a proposed View drawn beside what the proposal changes, then approved;
 * and a proposed file that does not read as a View, said in words with Approve still offered.
 * Each journey has a keyboard-only pass (Tab and Shift+Tab, Enter, Space; Option+Tab to a link in
 * WebKit, as Safari reaches links). Each screen has zero axe violations and visible focus, and a
 * screenshot for the owner under `.artifacts/ui/` (untracked).
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself. The API is answered
 * through `page.route` (`support/explorer.ts`, `support/views.ts`).
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  routeApi,
  shot,
  useExplorerServer,
  WS,
} from './support/explorer';
import {
  ACCEPTED,
  CITED_AT,
  createChange,
  FIGURES,
  FIGURES_PATH,
  PROPOSAL_ID,
  proposalAnswers,
  tabTo,
  VIEW,
  VIEW_PATH,
  viewAnswers,
  viewFileOf,
} from './support/views';

const citedLink = (page: Page) =>
  page.getByRole('navigation', { name: 'Sources this View cites' }).getByRole('link', {
    name: `${FIGURES_PATH}, the whole item, at revision ${CITED_AT.slice(0, 10)}`,
  });

/** The View's own table and, under its chart, the chart's data. */
const tables = (within: Locator) => within.getByRole('table', { name: 'revenue' });

/** A drawn chart: what vega-embed puts in the chart's place. */
const drawnChart = (within: Locator) => within.locator('.vega-embed canvas, .vega-embed svg');

/**
 * Bring keyboard focus to a link from the keyboard alone. WebKit, like Safari by default, leaves
 * links out of the Tab order (a platform setting the page cannot change); Option+Tab is how a
 * Safari user reaches them, and it does so here.
 */
async function focusLink(page: Page, link: Locator, browserName: string) {
  await tabTo(page, link, 150, browserName === 'webkit' ? 'Alt+Tab' : 'Tab');
}

test.describe('Views in the Explorer', () => {
  const server = useExplorerServer();

  test('open a saved View and its table', async ({ page }) => {
    await routeApi(page, viewAnswers());
    await page.goto(`${server.origin()}/w/${WS}/i/${VIEW}`);
    await expect(
      page.getByRole('tab', { name: 'Revenue by quarter', selected: true }),
    ).toBeVisible();
    const view = page.getByRole('region', { name: 'The View' });
    // The View's own table, row by row, from the retained dataset.
    const table = tables(view);
    await expect(table).toHaveCount(1);
    await expect(table.getByRole('row')).toHaveText([
      'quarterrevenue',
      'Q11.2',
      'Q21.4',
      'Q31.9',
      'Q42.3',
    ]);
    await expect(drawnChart(view).first()).toBeVisible();
    await expect(view.getByRole('alert')).toHaveCount(0);
    // What it was drawn from, at the revision it pins.
    await expect(view.getByRole('list', { name: 'Sources read' })).toContainText(
      `revenue: ${FIGURES_PATH} at revision ${CITED_AT}`,
    );
    // Its text is one step away, not in the View's place.
    const text = page.getByRole('group', { name: 'Text of this View' });
    await expect(text.locator('strong', { hasText: '2025' })).toBeHidden();
    await expectNoAxeViolations(page, 'a saved View');
    await shot(page, 'm4b-views-01-saved-view');
    await page
      .getByRole('navigation', { name: 'Sources this View cites' })
      .scrollIntoViewIfNeeded();
    await shot(page, 'm4b-views-01b-view-sources');

    await view.locator('summary', { hasText: 'Source data' }).click();
    await expect(tables(view)).toHaveCount(2);
    await expect(tables(view).nth(1).getByRole('row')).toHaveCount(5);
    // The pane scrolls inside the page: bring what was opened on screen for the screenshot.
    await tables(view).nth(1).scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'a saved View’s chart data');
    await shot(page, 'm4b-views-02-chart-data');

    await text.locator('summary').click();
    await expect(text.locator('strong', { hasText: '2025' })).toBeVisible();
    await expect(text).toContainText('"grammar": "json_render"');
    await text.locator('summary').scrollIntoViewIfNeeded();
    await expectNoAxeViolations(page, 'a saved View’s text');
    await shot(page, 'm4b-views-03-view-text');

    await citedLink(page).click();
    await expect(page.getByRole('tab', { name: 'Revenue figures', selected: true })).toBeVisible();
    expect(new URL(page.url()).pathname).toBe(`/w/${WS}/i/${FIGURES}`);
    expect(new URL(page.url()).searchParams.get('rev')).toBe(CITED_AT);
    await expect(page.getByText(`You are reading revision ${CITED_AT.slice(0, 10)}`)).toBeVisible();
    await expect(page.getByText('Quarterly revenue as reported, in EUR million.')).toBeVisible();
    await expectNoAxeViolations(page, 'a cited source at the revision the View pins');
    await shot(page, 'm4b-views-04-cited-source');
  });

  test('open a saved View and its table, from the keyboard alone', async ({
    page,
    browserName,
  }) => {
    await routeApi(page, viewAnswers());
    await page.goto(`${server.origin()}/w/${WS}/i/${VIEW}`);
    const view = page.getByRole('region', { name: 'The View' });
    await expect(tables(view)).toHaveCount(1);

    // The chart's data, opened with Space.
    const chartData = view.locator('summary', { hasText: 'Source data' });
    await tabTo(page, chartData);
    await expectVisibleFocus(page);
    await page.keyboard.press('Space');
    await expect(tables(view)).toHaveCount(2);
    await expect(chartData).toBeFocused();

    // The text, opened with Enter.
    const text = page.getByRole('group', { name: 'Text of this View' });
    const textSummary = text.locator('summary');
    await tabTo(page, textSummary);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(text.locator('strong', { hasText: '2025' })).toBeVisible();
    await expectNoAxeViolations(page, 'a saved View, from the keyboard');

    // A cited source, followed with Enter: it opens in a tab at the revision the View pins.
    const link = citedLink(page);
    await focusLink(page, link, browserName);
    await expect(link).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page.getByRole('tab', { name: 'Revenue figures', selected: true })).toBeVisible();
    expect(new URL(page.url()).searchParams.get('rev')).toBe(CITED_AT);
    expect(await focused(page)).not.toBe('body');
    await expectNoAxeViolations(page, 'a cited source, from the keyboard');
  });

  test('review a proposed View, then approve it', async ({ page }) => {
    await routeApi(page, proposalAnswers());
    await page.goto(`${server.origin()}/w/${WS}/proposals/${PROPOSAL_ID}`);
    const detail = page.getByRole('region', { name: 'Add the revenue View' });
    const changes = detail.getByRole('region', { name: 'What it changes' });
    await expect(changes).toBeVisible();
    const proposed = detail.getByRole('region', { name: `Proposed View: ${VIEW_PATH}` });
    await expect(tables(proposed).getByRole('row')).toHaveCount(5);
    await expect(drawnChart(proposed).first()).toBeVisible();
    await expect(proposed.getByRole('alert')).toHaveCount(0);
    const approve = detail.getByRole('button', { name: 'Approve…' });
    await expect(approve).toHaveAttribute('aria-disabled', 'false');
    await expectNoAxeViolations(page, 'a proposed View');
    await shot(page, 'm4b-views-05-proposed-view');

    await approve.click();
    const dialog = page.getByRole('alertdialog', { name: 'Accept “Add the revenue View”?' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Accept this proposal' }).click();
    await expect(dialog).toHaveCount(0);
    await expect(detail.getByText('Accepted', { exact: true })).toBeVisible();
    await expect(page.locator('.head-line code')).toHaveText(ACCEPTED.slice(0, 10));
    await expectNoAxeViolations(page, 'an accepted proposed View');
    await shot(page, 'm4b-views-06-proposed-view-accepted');
  });

  test('review a proposed View, then approve it, from the keyboard alone', async ({ page }) => {
    await routeApi(page, proposalAnswers());
    await page.goto(`${server.origin()}/w/${WS}/proposals/${PROPOSAL_ID}`);
    const detail = page.getByRole('region', { name: 'Add the revenue View' });
    const proposed = detail.getByRole('region', { name: `Proposed View: ${VIEW_PATH}` });
    await expect(tables(proposed)).toHaveCount(1);

    // The proposed chart's data, opened with Space.
    const chartData = proposed.locator('summary', { hasText: 'Source data' });
    await tabTo(page, chartData);
    await expectVisibleFocus(page);
    await page.keyboard.press('Space');
    await expect(tables(proposed)).toHaveCount(2);

    // Approve, then the confirmation: focus starts on Cancel, Shift+Tab reaches Accept.
    const approve = detail.getByRole('button', { name: 'Approve…' });
    await expect(approve).toHaveAttribute('aria-disabled', 'false');
    await tabTo(page, approve);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('alertdialog', { name: 'Accept “Add the revenue View”?' });
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    await expect(dialog.getByRole('button', { name: 'Accept this proposal' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the confirmation of a proposed View');
    await page.keyboard.press('Enter');
    await expect(dialog).toHaveCount(0);
    await expect(detail.getByText(/^Accepted: its changes are in a new version/)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'an accepted proposed View, from the keyboard');
  });

  test('a proposed file that does not read as a View is said in words, and Approve stays offered', async ({
    page,
  }) => {
    const { properties } = viewFileOf();
    await routeApi(
      page,
      proposalAnswers(createChange({ properties, body: 'Revenue grew in 2025.\n' })),
    );
    await page.goto(`${server.origin()}/w/${WS}/proposals/${PROPOSAL_ID}`);
    const detail = page.getByRole('region', { name: 'Add the revenue View' });
    await expect(detail.getByRole('region', { name: 'What it changes' })).toBeVisible();
    const proposed = detail.getByRole('region', { name: `Proposed View: ${VIEW_PATH}` });
    await expect(proposed.getByRole('note')).toContainText(
      'This proposed file cannot be shown as a View: its text has no View block.',
    );
    await expect(proposed.getByRole('table')).toHaveCount(0);
    const approve = detail.getByRole('button', { name: 'Approve…' });
    await expect(approve).toHaveAttribute('aria-disabled', 'false');
    await tabTo(page, approve);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a proposed file that is not a View');
    await shot(page, 'm4b-views-07-proposed-not-a-view');
  });
});
