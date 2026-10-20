/**
 * Corrections of a source (UI M4a, ruling A4), in Chromium and WebKit, each journey once with the
 * pointer and once from the keyboard alone:
 * - Correct text through to the new revision: the reader shows a partly converted PDF and says
 *   the text shown is the converter's; "Correct text" opens a text box beside the text shown,
 *   started from it, with focus in it; the person fixes a misread year and saves; the service
 *   commits a new revision on the head shown with the item's digest there; the workspace then
 *   shows that revision, and the reader reads it: the corrected text, labelled as a correction.
 * - Reconvert's job: "Convert only the pages not converted" asks `redigest_item` with the
 *   settings the conversion recorded; the 202 job is said and followed in the section until it
 *   finishes with the revision it made; "Show the new conversion" shows that revision, and the
 *   reader reads the converter's new text.
 * Every screen has zero axe violations and visible focus, and a screenshot for the owner under
 * `.artifacts/ui/` (untracked).
 *
 * The service is played through `page.route` (`support/m4a-corrections.ts`); the original is
 * reached as every journey reaches it (`openOriginal`). Needs `bun --bun run build` first.
 */
import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  HEAD,
  SOURCE,
  shot,
  useExplorerServer,
  WS,
} from './support/explorer';
import { CORRECTED, DIGEST, FIXED, playCorrections, RECONVERTED } from './support/m4a-corrections';
import { openOriginal } from './support/reader';
import { tabTo } from './support/views';

const short = (revision: string) => revision.slice(0, 10);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const section = (page: Page) => page.getByRole('region', { name: 'Corrections' });
const correctButton = (page: Page) => section(page).getByRole('button', { name: 'Correct text' });
const yourText = (page: Page) => section(page).getByRole('textbox', { name: 'Your correction' });
const saveButton = (page: Page) =>
  section(page).getByRole('button', { name: 'Save the correction' });
const reconvert = (page: Page) => section(page).getByRole('group', { name: 'Reconvert' });
const onlyPages = (page: Page) =>
  reconvert(page).getByRole('button', { name: 'Convert only the pages not converted' });
const showNew = (page: Page) =>
  reconvert(page).getByRole('button', { name: 'Show the new conversion' });
const jobRow = (page: Page) =>
  reconvert(page).getByRole('list', { name: 'The reconversion started here' });

async function openSource(page: Page, origin: string, keyboard = false) {
  await page.goto(`${origin}/w/${WS}/i/${SOURCE}`);
  await expect(await openOriginal(page, { keyboard })).toBeVisible();
  await expect(section(page)).toContainText(
    `As of revision ${short(HEAD)}, the version this workspace shows, the text shown is the converter’s text.`,
  );
}

/** Put the caret after the misread year, at the end of the text's third line, and fix it. */
async function fixTheYear(page: Page, box: Locator) {
  await expect(box).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('End');
  for (let count = 0; count < 5; count += 1) await page.keyboard.press('Backspace');
  await page.keyboard.type('2025.');
  await expect(section(page)).toContainText(
    'Your text differs from the text shown in 1 place: line 3.',
  );
}

/** What the page says once the correction is the head the workspace shows. */
async function expectCorrectionShown(page: Page) {
  await expect(section(page).getByRole('status').filter({ hasText: 'Saved:' })).toHaveText(
    `Saved: your correction is revision ${short(CORRECTED)}. The workspace now shows it, and the text above is your correction.`,
  );
  await expect(page.locator('.head-line')).toContainText(`revision ${short(CORRECTED)}`);
  await expect(page.getByText('Revenue grew by 21% in 2025.', { exact: true })).toBeVisible();
  await expect(
    page.getByText('A person’s correction is shown over the converted text.'),
  ).toBeVisible();
  await expect(section(page)).toContainText(
    `As of revision ${short(CORRECTED)}, the version this workspace shows, the text shown is a person’s correction, shown over the converter’s text.`,
  );
  await expect(yourText(page)).toHaveCount(0);
}

test.describe('correct a source’s text through to the new revision', () => {
  const server = useExplorerServer();

  test('with the pointer', async ({ page }) => {
    const service = await playCorrections(page);
    await openSource(page, server.origin());
    await expectNoAxeViolations(page, 'corrections offered');
    await section(page).scrollIntoViewIfNeeded();
    await shot(page, 'm4a-corrections-01-offered');

    await correctButton(page).click();
    const box = yourText(page);
    await expect(box).toBeFocused();
    await expect(
      section(page).getByRole('textbox', { name: 'The text shown now' }),
    ).toHaveAttribute('aria-readonly', 'true');
    await fixTheYear(page, box);
    await expectNoAxeViolations(page, 'a correction beside the text shown');
    await shot(page, 'm4a-corrections-02-editing');

    await saveButton(page).click();
    await expectCorrectionShown(page);
    expect(service.corrections).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        base_revision: HEAD,
        digest: DIGEST,
        corrected_markdown: FIXED,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    await expectNoAxeViolations(page, 'the correction saved and shown');
    await shot(page, 'm4a-corrections-03-saved');
  });

  test('from the keyboard alone', async ({ page }) => {
    const service = await playCorrections(page);
    await openSource(page, server.origin(), true);

    await tabTo(page, correctButton(page));
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    const box = yourText(page);
    await expect(box).toBeFocused();
    await expectVisibleFocus(page);
    await fixTheYear(page, box);
    // Tab leaves the text box for the button that saves.
    await page.keyboard.press('Tab');
    await expect(saveButton(page)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a correction, from the keyboard');
    await shot(page, 'm4a-corrections-04-keyboard-editing');

    await page.keyboard.press('Enter');
    await expectCorrectionShown(page);
    // Focus is on the words that say what was done.
    const done = section(page).locator('[tabindex="-1"]').filter({ hasText: 'Saved:' });
    await expect(done).toBeFocused();
    await expectVisibleFocus(page);
    expect(service.corrections.map((body) => body.corrected_markdown)).toEqual([FIXED]);
    await expectNoAxeViolations(page, 'the correction saved, from the keyboard');
    await shot(page, 'm4a-corrections-05-keyboard-saved');
  });
});

test.describe('Reconvert’s job', () => {
  const server = useExplorerServer();

  /** The job is said, followed until it finishes, and the version it made is offered. */
  async function expectFollowedToTheEnd(page: Page) {
    await expect(reconvert(page)).toContainText(
      'The reconversion was accepted as Conversion of a file again, job 77777777: “waiting to start” when the service answered.',
    );
    // Asked again on the jobs' clock until it ends.
    await expect(jobRow(page)).toContainText('Finished.', { timeout: 30_000 });
    await expect(jobRow(page)).toContainText(
      `It made revision ${short(RECONVERTED)} of the workspace.`,
    );
    await expect(showNew(page)).toBeVisible();
  }

  /** The new conversion is the head shown, and the reader reads its text. */
  async function expectNewConversionShown(page: Page) {
    await expect(reconvert(page)).toContainText(
      `The workspace now shows revision ${short(RECONVERTED)}, which the conversion made.`,
    );
    await expect(page.locator('.head-line')).toContainText(`revision ${short(RECONVERTED)}`);
    await expect(
      page.getByText('The pages that were not converted are converted now.'),
    ).toBeVisible();
    await expect(page.getByText('Conversion: Converted.')).toBeVisible();
    await expect(onlyPages(page)).toHaveCount(0);
  }

  test('with the pointer', async ({ page }) => {
    const service = await playCorrections(page);
    await openSource(page, server.origin());
    await expect(reconvert(page)).toContainText('Pages 3–4 were not converted.');
    await expect(reconvert(page)).toContainText(
      'With the settings of its current conversion (docling 2.3.0): text recognition only where a page has no text layer, in en; page images kept, at 144 dpi; table structure reconstructed.',
    );

    await onlyPages(page).click();
    await expect(jobRow(page)).toBeVisible();
    expect(service.reconversions).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        base_revision: HEAD,
        settings: {
          ocr: 'auto',
          ocr_language: 'en',
          page_image_dpi: 144,
          page_images: true,
          table_structure: true,
        },
        unconverted_only: true,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    await expectNoAxeViolations(page, 'a reconversion accepted');
    await reconvert(page).scrollIntoViewIfNeeded();
    await shot(page, 'm4a-corrections-06-reconvert-accepted');

    await expectFollowedToTheEnd(page);
    await expectNoAxeViolations(page, 'a reconversion finished');
    await shot(page, 'm4a-corrections-07-reconvert-finished');

    await showNew(page).click();
    await expectNewConversionShown(page);
    expect(service.reconversions).toHaveLength(1);
    await expectNoAxeViolations(page, 'the new conversion shown');
    await shot(page, 'm4a-corrections-08-reconverted');
  });

  test('from the keyboard alone', async ({ page }) => {
    const service = await playCorrections(page);
    await openSource(page, server.origin(), true);

    await tabTo(page, onlyPages(page));
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expectFollowedToTheEnd(page);
    // The button kept focus through the answer and the job's readings.
    await expect(onlyPages(page)).toBeFocused();
    await tabTo(page, showNew(page));
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a reconversion finished, from the keyboard');
    await shot(page, 'm4a-corrections-09-keyboard-reconvert');

    await page.keyboard.press('Enter');
    await expectNewConversionShown(page);
    // The button stays, with focus, and has nothing more to show.
    await expect(showNew(page)).toBeFocused();
    await expect(showNew(page)).toHaveAttribute('aria-disabled', 'true');
    await expectVisibleFocus(page);
    expect(service.reconversions.map((body) => body.unconverted_only)).toEqual([true]);
    await expectNoAxeViolations(page, 'the new conversion shown, from the keyboard');
  });
});
