/**
 * The reader's views (UI M4a, ruling A1), in Chromium and WebKit, with the pointer and from the
 * keyboard alone: a converted PDF source offers Text, Outline, Text and pictures, Pages and
 * Original, a Note only Text and Outline (and says why not the others); each view reads the
 * pane's selection; a section chosen in the Outline is read in the Text view, and "Continue
 * reading" goes on from the keyboard. Every screen has zero axe violations and visible focus, and a
 * screenshot for the owner (`m4a-reader-NN-…`, under the untracked `.artifacts/ui/`).
 *
 * The service is played by `page.route` (`support/m4a-reader.ts`); the PNG images are drawn there.
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */
import { expect, type Page, test } from '@playwright/test';
import {
  answers,
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  NOTE,
  note,
  ok,
  readAnswer,
  routeApi,
  shot,
  useExplorerServer,
  WS,
} from './support/explorer';
import {
  type Downloads,
  PHOTO,
  REPORT,
  type Reads,
  readerAnswers,
  routeObjects,
} from './support/m4a-reader';
import { chooseView, openOriginal, readAs } from './support/reader';
import { tabTo } from './support/views';

type Body = Record<string, unknown>;

async function play(
  page: Page,
  overrides: Record<string, (body: Body) => ReturnType<typeof ok>> = {},
) {
  const reads: Reads = { bodies: [] };
  const downloads: Downloads = { paths: [] };
  const others = answers()['/api/items/get-item'];
  if (others === undefined) throw new Error('the explorer answers no get_item');
  await routeApi(page, { ...readerAnswers(reads, others), ...overrides });
  await routeObjects(page, downloads);
  return { reads, downloads };
}

const lastRead = (reads: Reads) => reads.bodies.at(-1) ?? {};

test.describe('the reader’s views', () => {
  const server = useExplorerServer();

  test('a Source’s views, switched with the pointer, each reading the selection', async ({
    page,
  }) => {
    const { reads } = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${REPORT}`);
    const article = page.getByRole('article', { name: 'Annual report 2025' });
    await expect(article.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    const group = readAs(page);
    await expect(group.getByRole('radio')).toHaveCount(5);
    await expect(group.getByRole('radio', { name: 'Text', exact: true })).toBeChecked();
    await expectNoAxeViolations(page, 'the text view of a source');
    await shot(page, 'm4a-reader-01-text');

    await chooseView(page, 'Outline');
    const outline = article.getByRole('navigation', { name: 'Outline of this item' });
    await expect(outline.getByRole('button', { name: 'Budget' })).toBeVisible();
    expect(lastRead(reads).view).toBe('outline');
    await expectNoAxeViolations(page, 'the outline view');
    await shot(page, 'm4a-reader-02-outline');

    await chooseView(page, 'Text and pictures');
    await expect(article.getByRole('img', { name: 'Revenue by quarter, 2025' })).toBeVisible();
    await expect(
      article.getByRole('img', {
        name: 'A picture inferred to be on page 3, not certain; the document gives it no caption.',
      }),
    ).toBeVisible();
    await expect(article.getByText('Inferred location, not certain:')).toBeVisible();
    expect(lastRead(reads)).toMatchObject({ view: 'multimodal', max_images: 8 });
    await expectNoAxeViolations(page, 'the text and pictures view');
    await shot(page, 'm4a-reader-03-pictures');

    const original = await openOriginal(page);
    await expect(original.getByRole('heading', { name: 'Original' })).toBeVisible();
    await expect.poll(() => lastRead(reads).view).toBe('original');
    await shot(page, 'm4a-reader-04-original');

    // A section chosen in the Outline is read in the Text view; every view then reads it.
    await chooseView(page, 'Outline');
    await outline.getByRole('button', { name: 'Budget' }).click();
    await expect(group.getByRole('radio', { name: 'Text', exact: true })).toBeChecked();
    await expect(article.getByText('The budget is published after review.')).toBeVisible();
    await expect(article.getByText('Showing section “Budget”.')).toBeVisible();
    expect(lastRead(reads)).toMatchObject({
      view: 'text',
      selection: { kind: 'section', heading: 'Budget' },
    });
    await openOriginal(page);
    await expect
      .poll(() => lastRead(reads))
      .toMatchObject({
        view: 'original',
        selection: { kind: 'section', heading: 'Budget' },
      });
    const places = page.getByRole('list', { name: 'Where this reading lies in the original' });
    await expect(places.getByRole('listitem')).toHaveText([
      'Direct location: page 4, recorded from the document itself',
      'Inferred location, not certain: page 5, found by matching text or layout',
    ]);
    await expectNoAxeViolations(page, 'the original view of a section');
    await shot(page, 'm4a-reader-05-original-of-a-section');
  });

  test('a Note offers Text and Outline, and says why not the others', async ({ page }) => {
    await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const article = page.getByRole('article', { name: 'Meeting notes' });
    await expect(article.getByText(/publish the budget/)).toBeVisible();
    await expect(readAs(page).getByRole('radio')).toHaveCount(2);
    await expect(
      article.getByText(
        'Not offered here: Text and pictures, because a Note keeps no pictures of its own; pictures are kept from a converted Source; Pages, because a Note has no pages; page renders are kept from a converted Source; Original, because a Note has no original file: its text is the item.',
      ),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'a note’s views');
    await shot(page, 'm4a-reader-06-note');
  });

  test('an original image is read through its pinned object and can be sized from the keyboard', async ({
    page,
  }) => {
    const { downloads } = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${PHOTO}`);
    const original = await openOriginal(page, { keyboard: true });
    const image = original.getByRole('img', {
      name: 'harbour.png, the original image. It carries no caption of its own.',
    });
    await expect(image).toBeVisible();
    await image.evaluate((element) => (element as HTMLImageElement).decode());
    await expect(original.getByText(/640 \u00d7 400 pixels/)).toBeVisible();
    expect(await image.getAttribute('src')).toMatch(/^blob:/);
    expect(downloads.paths).toEqual([
      `/api/workspaces/${WS}/items/${PHOTO}/revisions/${'a'.repeat(40)}/objects/${'e'.repeat(64)}`,
    ]);
    await page.keyboard.press('Tab');
    const sizing = original.getByRole('button', { name: 'Show at actual size' });
    await expect(sizing).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Space');
    await expect(sizing).toHaveAttribute('aria-pressed', 'true');
    await expect(original.getByText(/Shown at its actual size/)).toBeVisible();
    await expectNoAxeViolations(page, 'the original image at actual size');
    await shot(page, 'm4a-reader-09-image-actual-size');
    await page.keyboard.press('PageDown');
    await expect(image).toBeInViewport();
    await shot(page, 'm4a-reader-09b-image-content');
    await page.keyboard.press('Space');
    await expect(sizing).toHaveAttribute('aria-pressed', 'false');
    await expectNoAxeViolations(page, 'the original image fitted to the pane');
  });

  test('page renders honor the eight-image budget and the next inclusive page selection', async ({
    page,
  }) => {
    const { reads } = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${REPORT}`);
    const article = page.getByRole('article', { name: 'Annual report 2025' });
    await chooseView(page, 'Pages', { keyboard: true });
    await expect(article.getByRole('img')).toHaveCount(8);
    await expect(
      article.getByText('4 more images lie in the selection; raise max_images or narrow it'),
    ).toBeVisible();
    const next = article.getByRole('button', { name: 'Show pages 9\u201312' });
    await tabTo(page, next);
    await expect(next).toBeFocused();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(article.getByRole('img')).toHaveCount(4);
    expect(lastRead(reads)).toMatchObject({
      view: 'pages',
      max_images: 8,
      selection: { kind: 'pages', range: { start: 9, end: 12 } },
    });
    await expect(
      article.getByRole('img', { name: /^Page 9 as the converter rendered it/ }),
    ).toBeVisible();
    await expect(
      article.getByRole('img', { name: /^Page 12 as the converter rendered it/ }),
    ).toBeVisible();
    await expect(article.getByRole('button', { name: 'Show pages 1\u20138' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the next bounded page render selection');
    await shot(page, 'm4a-reader-10-page-window');
    await page.keyboard.press('PageDown');
    await expect(
      article.getByRole('img', { name: /^Page 9 as the converter rendered it/ }),
    ).toBeInViewport();
    await shot(page, 'm4a-reader-10b-page-content');
  });

  test('keyboard only: the views of a Source, a section from the Outline, and back', async ({
    page,
  }) => {
    const { reads } = await play(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${REPORT}`);
    const article = page.getByRole('article', { name: 'Annual report 2025' });
    await expect(article.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    await chooseView(page, 'Outline', { keyboard: true });
    await expectVisibleFocus(page);
    const budget = article
      .getByRole('navigation', { name: 'Outline of this item' })
      .getByRole('button', { name: 'Budget' });
    await expect(budget).toBeVisible();
    // Tab from the choice reaches the outline's entries.
    for (
      let presses = 0;
      presses < 10 && !(await budget.evaluate((el) => el === document.activeElement));
      presses += 1
    ) {
      await page.keyboard.press('Tab');
    }
    await expect(budget).toBeFocused();
    await page.keyboard.press('Enter');
    // The entry leaves with its view: focus goes to the first part of the text it chose.
    await expect(article.getByText('The budget is published after review.')).toBeVisible();
    await expect.poll(() => focused(page)).toMatch(/^div:/);
    await expectVisibleFocus(page);
    expect(lastRead(reads)).toMatchObject({ view: 'text', selection: { heading: 'Budget' } });
    await chooseView(page, 'Pages', { keyboard: true });
    await expect(
      article.getByRole('img', { name: /^Page 4 as the converter rendered it/ }),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'the pages view, from the keyboard');
    await shot(page, 'm4a-reader-07-keyboard-pages');
  });
});

test.describe('"Continue reading" from the keyboard', () => {
  const server = useExplorerServer();

  test('each part takes focus, the last one too, and focus moved elsewhere stays there', async ({
    page,
  }) => {
    let held: (() => void) | null = null;
    await play(page, {
      '/api/reads/read-item': async (body) => {
        const cursor = typeof body.cursor === 'string' ? body.cursor : null;
        const number = cursor === null ? 1 : Number(cursor.replace('part-', ''));
        if (number === 3) await new Promise<void>((resolve) => (held = resolve));
        return ok(
          readAnswer(note, body, {
            markdown: `Part ${number} of the minutes`,
            outline: [],
            receipt_id: `55555555-5555-4555-8555-00000000000${number}`,
            truncated: number < 3,
            ...(number < 3 ? { next_cursor: `part-${number + 1}` } : {}),
          }),
        );
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const article = page.getByRole('article', { name: 'Meeting notes' });
    await expect(article.getByText('Part 1 of the minutes')).toBeVisible();
    const more = article.getByRole('button', { name: 'Continue reading' });
    await tabTo(page, more);
    await page.keyboard.press('Enter');
    const second = article.getByRole('group', { name: 'Part 2 of this reading' });
    await expect(second).toBeFocused();
    await expectVisibleFocus(page);
    await shot(page, 'm4a-reader-08-continued');
    // The last part is held back; the person moves on to the search box meanwhile.
    await tabTo(page, more);
    await page.keyboard.press('Enter');
    await expect(article.getByRole('button', { name: 'Reading more…' })).toBeFocused();
    const search = page.getByRole('searchbox').first();
    await tabTo(page, search);
    await expect.poll(() => held !== null).toBe(true);
    held?.();
    await expect(article.getByText('Part 3 of the minutes')).toBeVisible();
    await page.waitForTimeout(200);
    await expect(search).toBeFocused();
  });
});
