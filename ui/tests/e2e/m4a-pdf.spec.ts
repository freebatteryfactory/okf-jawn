/**
 * Open a PDF and page through it (UI M4a, ruling A2), in Chromium and WebKit: once with the
 * pointer, and once from the keyboard alone. The original is a real three-page PDF written at run
 * time (`support/m4a-pdf.ts`), read through `download_object` and drawn by PDF.js, under the page
 * policy the service sends (`PAGE_POLICY`, from `crates/server/src/assets.rs`), with no refusal of
 * it reported.
 *
 * What each journey proves in the browser, beyond the unit tests:
 * - PDF.js's worker is a module worker started from this build's own file on the page's origin,
 *   and the standard font (Helvetica is not embedded) and the CMap (`UniGB-UCS2-H`) are fetched
 *   from the page's origin too; nothing is asked of another origin.
 * - each page is drawn on its canvas (ink on it), with its text in the text layer;
 * - closing the reader's tab ends the document's worker.
 * Also: a `pages` selection opens at its first page and says the range, and a large PDF is read
 * in parts, with a `Range` for each, reading far less than the whole file. Every screen has zero
 * axe violations and visible focus where focus is, and a screenshot under `.artifacts/ui/`.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import { expectNoAxeViolations, expectVisibleFocus, shot, WS } from './support/explorer';
import {
  type ByteRead,
  buildPdf,
  expectDrawn,
  OBJECT_PATH,
  PDFJS_FILES,
  pageImage,
  REPORT,
  routeSource,
  sourceAddress,
  useOriginalServer,
  watchPolicy,
  watchWorkers,
} from './support/m4a-pdf';
import { openOriginal } from './support/reader';
import { tabTo } from './support/views';

const PDF = 'application/pdf';

/** Open the reader on the PDF; returns the original's region and what the page did. */
async function openReport(page: Page, origin: string, selection?: unknown, bytes = REPORT) {
  const policy = await watchPolicy(page);
  const workers = watchWorkers(page);
  const reads = await routeSource(page, {
    mediaType: PDF,
    bytes,
    filename: 'annual-report.pdf',
  });
  await page.goto(sourceAddress(origin, selection));
  const original = await openOriginal(page);
  return { original, policy, workers, reads };
}

/** Everything PDF.js fetched from the page's own origin, by the paths the static server saw. */
function pdfjsFetched(asked: string[]): string[] {
  return asked.filter((path) => path.startsWith(PDFJS_FILES));
}

const pages = (original: Locator) => original.getByRole('group', { name: 'Pages of the PDF' });

test.describe('open a PDF and page through it', () => {
  const server = useOriginalServer();

  test('with the pointer: drawn by PDF.js from this origin, paged through, and let go with its tab', async ({
    page,
  }) => {
    const { original, policy, workers, reads } = await openReport(page, server.origin());
    await expectDrawn(original, 1);
    await expect(original.getByText('Page one: the year in figures.')).toBeAttached();
    // One whole read through download_object, never get_object.
    expect(reads).toEqual<ByteRead[]>([{ range: null, length: REPORT.length }]);
    // The worker: this build's own file, on the page's origin.
    expect(workers.started()).toEqual([`${server.origin()}${PDFJS_FILES}pdf.worker.min.js`]);
    // The standard font for Helvetica, from the page's origin.
    expect(pdfjsFetched(server.asked()).some((path) => path.includes('/standard_fonts/'))).toBe(
      true,
    );
    await expectNoAxeViolations(page, 'a PDF open at its first page');
    await shot(page, 'm4a-pdf-01-first-page');

    await pages(original).getByRole('button', { name: 'Next page' }).click();
    await expectDrawn(original, 2);
    await expect(original.getByText('Page two: revenue grew by 12 percent.')).toBeAttached();
    await pages(original).getByRole('button', { name: 'Next page' }).click();
    await expectDrawn(original, 3);
    // The third page writes in a CID font: its CMap came from the page's origin.
    await expect
      .poll(() => pdfjsFetched(server.asked()).filter((path) => path.includes('/cmaps/')))
      .toContain(`${PDFJS_FILES}cmaps/UniGB-UCS2-H.bcmap`);
    const next = pages(original).getByRole('button', { name: 'Next page' });
    await expect(next).toHaveAttribute('aria-disabled', 'true');
    await expectNoAxeViolations(page, 'a PDF at its last page');
    await shot(page, 'm4a-pdf-02-last-page');

    const field = pages(original).getByRole('spinbutton', { name: 'Page number' });
    await field.fill('9');
    await pages(original).getByRole('button', { name: 'Go to page' }).click();
    await expect(original.getByRole('alert')).toHaveText(
      'There is no page 9: this PDF has 3 pages.',
    );
    await expect(field).toHaveAttribute('aria-invalid', 'true');
    await expectNoAxeViolations(page, 'a page the PDF does not have');
    await shot(page, 'm4a-pdf-03-no-such-page');
    await field.fill('1');
    await pages(original).getByRole('button', { name: 'Go to page' }).click();
    await expectDrawn(original, 1);

    // Nothing was asked of another origin, and the page policy refused nothing.
    expect(await policy.refusals()).toEqual([]);
    expect(reads).toHaveLength(1);

    // Closing the reader's tab lets the document go: its worker ends.
    expect(workers.ended()).toEqual([]);
    const canvas = await pageImage(original, 1).elementHandle();
    if (canvas === null) throw new Error('the drawn PDF canvas is missing');
    await page.getByRole('button', { name: 'Close tab' }).click();
    await expect(page).toHaveURL(new RegExp(`/w/${WS}$`));
    await expect.poll(() => workers.ended()).toEqual(workers.started());
    // Keep a handle after detach to prove that closing also frees the browser's pixel buffer.
    expect(
      await canvas.evaluate((element) => {
        if (!(element instanceof HTMLCanvasElement)) throw new Error('the page is not a canvas');
        return { width: element.width, height: element.height };
      }),
    ).toEqual({ width: 0, height: 0 });
    await canvas.dispose();
  });

  test('from the keyboard alone: Tab, Enter and the number field', async ({ page }) => {
    const { original, policy, workers } = await openReport(page, server.origin());
    await expectDrawn(original, 1);

    const next = pages(original).getByRole('button', { name: 'Next page' });
    await tabTo(page, next);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expectDrawn(original, 2);
    await expect(next).toBeFocused();
    await page.keyboard.press('Enter');
    await expectDrawn(original, 3);
    // At its end the button stays, with focus, and does nothing.
    await page.keyboard.press('Enter');
    await expect(next).toBeFocused();
    await expect(original.getByRole('status')).toHaveText('Page 3 of 3');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the last page, from the keyboard');
    await shot(page, 'm4a-pdf-04-keyboard-last-page');

    await page.keyboard.press('Shift+Tab');
    const previous = pages(original).getByRole('button', { name: 'Previous page' });
    await expect(previous).toBeFocused();
    await page.keyboard.press('Space');
    await expectDrawn(original, 2);

    const field = pages(original).getByRole('spinbutton', { name: 'Page number' });
    await tabTo(page, field, 5);
    await expectVisibleFocus(page);
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.type('1');
    await page.keyboard.press('Enter');
    await expectDrawn(original, 1);
    await expect(field).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(pages(original).getByRole('button', { name: 'Go to page' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the number field, from the keyboard');
    await shot(page, 'm4a-pdf-05-keyboard-go-to-page');

    // Closing the tab from the keyboard lets the document go.
    const close = page.getByRole('button', { name: 'Close tab' });
    await tabTo(page, close);
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/w/${WS}$`));
    await expect.poll(() => workers.ended()).toEqual(workers.started());
    expect(await policy.refusals()).toEqual([]);
  });

  test('a pages selection opens at its first page and says the range', async ({ page }) => {
    const { original, policy } = await openReport(page, server.origin(), {
      kind: 'pages',
      range: { start: 2, end: 3 },
    });
    await expectDrawn(original, 2);
    await expect(
      original.getByText('The reader shows pages 2–3: the PDF opened at page 2.'),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'a PDF opened at a selection of pages');
    await shot(page, 'm4a-pdf-06-selection');
    expect(await policy.refusals()).toEqual([]);
  });

  test('a large PDF is read in parts, each a ranged read, far less than the whole', async ({
    page,
  }) => {
    // Its three pages are at its start; six megabytes no page needs come after them.
    const large = buildPdf(
      [
        { lines: ['Annual report 2025', 'Page one: the year in figures.'] },
        { lines: ['Revenue', 'Page two: revenue grew by 12 percent.'] },
        { lines: ['Outlook', 'Page three: the plan for 2026.'] },
      ],
      6 * 1024 * 1024,
    );
    const { original, policy, reads } = await openReport(page, server.origin(), undefined, large);
    await expectDrawn(original, 1);
    await expect(original.getByText(/This PDF is large \(.*\): it is read in parts/)).toBeVisible();
    await pages(original).getByRole('button', { name: 'Next page' }).click();
    await expectDrawn(original, 2);
    expect(reads.length).toBeGreaterThan(0);
    for (const read of reads) expect(read.range).toMatch(/^bytes=\d+-\d+$/);
    const read = reads.reduce((total, part) => total + part.length, 0);
    expect(read).toBeLessThan(large.length / 4);
    await expectNoAxeViolations(page, 'a large PDF read in parts');
    await shot(page, 'm4a-pdf-07-large');
    expect(await policy.refusals()).toEqual([]);
    expect(OBJECT_PATH).toContain('/objects/');
  });
});

test.describe('the page drawn', () => {
  const server = useOriginalServer();

  test('is one image to assistive technology, named by its page and file', async ({ page }) => {
    const { original } = await openReport(page, server.origin());
    await expect(pageImage(original, 1)).toBeVisible();
    await expect(pageImage(original, 1)).toHaveJSProperty('tagName', 'CANVAS');
  });
});
