/** A2: a bounded sheet range and its continuation, navigated without a pointer in both engines. */
import { expect, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  HEAD,
  SOURCE,
  shot,
  useExplorerServer,
  WS,
} from './support/explorer';
import { activate, sourceFixture, sourceUrl } from './support/m4a-sheets';
import { openOriginal } from './support/reader';

test.describe('A2 spreadsheet ranges', () => {
  const server = useExplorerServer();
  test('keeps the selected range and budget through continuation, and focuses the grown table', async ({
    page,
  }) => {
    const selection = {
      kind: 'cells' as const,
      range: { sheet: 'Q1', row_start: 2, row_end: 3, column_start: 1, column_end: 2 },
    };
    const fixture = await sourceFixture(page, {
      path: 'figures.xlsx',
      mediaType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      original: 'retained workbook bytes',
      markdown: '| Quarter | Revenue |\n| --- | --- |\n| Q1 | 1.2 |\n',
      secondPart: '| Q2 | 1.4 |\n| Q3 | 1.9 |\n',
      sheets: ['Q1', 'Costs'],
    });
    await page.goto(sourceUrl(server.origin(), selection));
    const original = await openOriginal(page);
    await expect(original.getByRole('table')).toHaveAccessibleName(
      'Table 1 of 1 read so far: 2 columns, 1 row',
    );
    await expect(original).toContainText('Sheets the read names: “Q1”, “Costs”.');
    await expect(original).toContainText('formulas and number formats were not read.');
    await expect(original).toContainText('Range asked: sheet “Q1”, rows 2–3, columns 1–2.');
    await expectNoAxeViolations(page, 'spreadsheet first bounded range');
    const header = original.getByRole('columnheader', { name: 'Quarter', exact: true });
    await header.focus();
    await page.keyboard.press('ArrowDown');
    await expect(original.getByRole('cell', { name: 'Q1', exact: true })).toBeFocused();
    await expectVisibleFocus(page);
    await activate(page, original.getByRole('button', { name: 'Read more of this spreadsheet' }));
    await expect(original.getByRole('table')).toHaveAccessibleName(
      'Table 1 of 1: 2 columns, 3 rows',
    );
    // Continuation restores the table's existing keyboard stop, which we moved to Q1.
    await expect(original.getByRole('cell', { name: 'Q1', exact: true })).toBeFocused();
    await page.keyboard.press('Control+End');
    await expect(original.getByRole('cell', { name: '1.9', exact: true })).toBeFocused();
    await expectVisibleFocus(page);
    expect(fixture.reads).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        at: { kind: 'revision', revision: HEAD },
        view: 'text',
        selection,
        cursor: null,
        max_bytes: 65_536,
        max_images: 0,
      },
      {
        workspace_id: WS,
        item_id: SOURCE,
        at: { kind: 'revision', revision: HEAD },
        view: 'text',
        selection,
        cursor: 'second-part',
        max_bytes: 65_536,
        max_images: 0,
      },
    ]);
    expect(fixture.ranges).toEqual([]);
    await expectNoAxeViolations(page, 'spreadsheet continued range');
    await shot(page, 'm4a-sheet-range');
  });
});
