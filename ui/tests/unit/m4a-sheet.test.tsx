/**
 * Spreadsheets as tables with ranges (UI M4a, ruling A2). A spreadsheet's original is binary, so
 * the viewer shows what the read gives of it: `read_item` (`view: 'text'`) with the reader's
 * selection and budget, whose Markdown holds the converted tables. Each table is a table with a
 * caption that counts its columns and rows, its column headers and row numbers as `th` with a
 * scope; it holds one stop of the Tab order, from which the arrow keys move from cell to cell.
 * Sheet names and the formula, value and format distinctions are shown where the read gives them
 * (the outline's sheet entries, the `cell_values_only` finding) and said in words where it does
 * not. A `cells` selection is asked as it is and said in words; the rest of a partial read is
 * asked with its cursor, one part per press, never walked.
 */
import { configure, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Selection } from '../../src/api/generated/types.gen';
import { SpreadsheetView } from '../../src/features/viewers/sheet/SpreadsheetView';
import { fail, mockApi, ok } from './support/api';
import {
  appearanceOf,
  extractionOf,
  READ_ITEM,
  renderViewer,
  sourceRead,
  viewerProps,
} from './support/m4a-sheets';
import { HEAD, SOURCE, WS } from './support/workspace';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const XLSX = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';
const PATH = 'figures.xlsx';

const revenue = [
  '| Quarter | Revenue |',
  '| --- | --- |',
  '| Q1 | 1.2 |',
  '| Q2 | 1.4 |',
  '| Q3 | 1.9 |',
].join('\n');
const costs = ['| Item | Cost |  |', '| --- | --- | --- |', '| Rent |  | Ada |'].join('\n');

function renderSheet(
  selection: Selection = { kind: 'all' },
  appearance = appearanceOf(XLSX, 9000),
) {
  return renderViewer(<SpreadsheetView {...viewerProps(PATH, appearance, selection)} />);
}

const asked = (selection: Selection, cursor: string | null = null) => ({
  workspace_id: WS,
  item_id: SOURCE,
  at: { kind: 'revision', revision: HEAD },
  view: 'text',
  selection,
  cursor,
  max_bytes: 65_536,
  max_images: 0,
});

describe('a spreadsheet as tables (A2)', () => {
  it('reads its text at the exact revision with the reader’s selection and budget, and shows each table with a caption, scoped headers and counts', async () => {
    const api = mockApi({
      [READ_ITEM]: () => ok(sourceRead(PATH, `${revenue}\n\n${costs}\n`)),
    });
    renderSheet();
    const tables = await screen.findAllByRole('table');
    expect(tables).toHaveLength(2);
    expect(api.callsTo(READ_ITEM)).toEqual([asked({ kind: 'all' })]);

    const [first, second] = tables;
    if (first === undefined || second === undefined) throw new Error('two tables expected');
    expect(within(first).getByText('Table 1 of 2: 2 columns, 3 rows')).toBeTruthy();
    expect(first.querySelector('caption')?.textContent).toBe('Table 1 of 2: 2 columns, 3 rows');
    const headers = within(first).getAllByRole('columnheader');
    expect(headers.map((header) => header.textContent)).toEqual(['Row', 'Quarter', 'Revenue']);
    for (const header of headers) expect(header.getAttribute('scope')).toBe('col');
    const rowHeaders = within(first).getAllByRole('rowheader');
    expect(rowHeaders.map((header) => header.textContent)).toEqual(['1', '2', '3']);
    for (const header of rowHeaders) expect(header.getAttribute('scope')).toBe('row');
    expect(within(first).getByRole('cell', { name: '1.9' })).toBeTruthy();
    expect(first.getAttribute('aria-rowcount')).toBe('4');

    // An empty heading is named for what it is, and an empty cell stays a cell.
    expect(within(second).getByText('Table 2 of 2: 3 columns, 1 row')).toBeTruthy();
    expect(
      within(second)
        .getAllByRole('columnheader')
        .map((header) => header.textContent),
    ).toEqual(['Row', 'Item', 'Cost', 'Column 3, no heading']);
    expect(
      within(second)
        .getAllByRole('cell')
        .map((cell) => cell.textContent),
    ).toEqual(['Rent', '', 'Ada']);

    expect(api.violations).toEqual([]);
  });

  it('holds one stop of the Tab order per table, from which the arrow keys move from cell to cell', async () => {
    const api = mockApi({ [READ_ITEM]: () => ok(sourceRead(PATH, revenue)) });
    renderSheet();
    const table = await screen.findByRole('table');
    const stops = [...table.querySelectorAll('[tabindex="0"]')];
    expect(stops.map((stop) => stop.textContent)).toEqual(['Quarter']);
    expect(table.querySelectorAll('[tabindex="-1"]')).toHaveLength(11);
    expect(table.getAttribute('aria-describedby')).toBe(
      screen.getByText(/the arrow keys move from cell to cell/).id,
    );

    await userEvent.tab();
    expect(document.activeElement?.textContent).toBe('Quarter');
    await userEvent.keyboard('{ArrowDown}{ArrowDown}{ArrowRight}');
    expect(document.activeElement?.textContent).toBe('1.4');
    // The cell reached is the stop now; the one left is not.
    expect(document.activeElement?.getAttribute('tabindex')).toBe('0');
    expect(
      within(table).getByRole('columnheader', { name: 'Quarter' }).getAttribute('tabindex'),
    ).toBe('-1');
    await userEvent.keyboard('{Home}');
    expect(document.activeElement?.textContent).toBe('2');
    await userEvent.keyboard('{Control>}{End}{/Control}');
    expect(document.activeElement?.textContent).toBe('1.9');
    await userEvent.keyboard('{ArrowDown}{ArrowRight}');
    expect(document.activeElement?.textContent).toBe('1.9');
    await userEvent.keyboard('{Control>}{Home}{/Control}');
    expect(document.activeElement?.textContent).toBe('Row');
    // A cell clicked is the stop.
    await userEvent.click(within(table).getByRole('cell', { name: 'Q3' }));
    expect(within(table).getByRole('cell', { name: 'Q3' }).getAttribute('tabindex')).toBe('0');
    expect(api.violations).toEqual([]);
  });

  it('says in words that the read names no sheet and gives no formula or format', async () => {
    const api = mockApi({ [READ_ITEM]: () => ok(sourceRead(PATH, revenue)) });
    renderSheet();
    await screen.findByRole('table');
    expect(
      screen.getByText('The read names no sheet: which sheet a table is on is not known here.'),
    ).toBeTruthy();
    expect(
      screen.getByText(
        'The read gives each cell as text: whether a cell holds a formula or a value, and how it is formatted, is not given.',
      ),
    ).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('names the sheets the read gives, and says where cells are values only', async () => {
    const api = mockApi({
      [READ_ITEM]: () =>
        ok(
          sourceRead(PATH, revenue, {
            outline: [
              {
                kind: 'sheet',
                label: 'Q1',
                level: 0,
                selection: { kind: 'lines', range: { start: 1, end: 5 } },
              },
              {
                kind: 'sheet',
                label: 'Costs',
                level: 0,
                selection: { kind: 'lines', range: { start: 6, end: 9 } },
              },
            ],
          }),
        ),
    });
    renderSheet(
      { kind: 'all' },
      appearanceOf(
        XLSX,
        9000,
        extractionOf({ warnings: [{ kind: 'cell_values_only', sheets: ['Q1', 'Costs'] }] }),
      ),
    );
    await screen.findByRole('table');
    expect(screen.getByText('Sheets the read names: “Q1”, “Costs”.')).toBeTruthy();
    expect(
      screen.getByText(
        'Values only in “Q1”, “Costs”: each cell shows the value the file stores; formulas and number formats were not read.',
      ),
    ).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('asks a cells selection as it is, says which range, and that no row of it is marked', async () => {
    const cells: Selection = {
      kind: 'cells',
      range: { sheet: 'Q1', row_start: 2, row_end: 3, column_start: 1, column_end: 2 },
    };
    const api = mockApi({
      [READ_ITEM]: () =>
        ok(
          sourceRead(
            PATH,
            revenue,
            {
              source: {
                workspace_id: WS,
                item_id: SOURCE,
                path: PATH,
                revision: HEAD,
                selection: cells,
                locations: [
                  { provenance: 'direct', locator: { kind: 'cells', range: cells.range } },
                ],
              },
            },
            cells,
          ),
        ),
    });
    renderSheet(cells);
    await screen.findByRole('table');
    expect(api.callsTo(READ_ITEM)).toEqual([asked(cells)]);
    expect(
      screen.getByText(
        'Range asked: sheet “Q1”, rows 2–3, columns 1–2. The read gives the converted tables that lie in this range, whole; it does not say which of their rows and columns are the range, so none is marked.',
      ),
    ).toBeTruthy();
    // The range names its sheet, so the sheet is named.
    expect(screen.getByText('Sheets the read names: “Q1”.')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('shows the read’s own warnings, and says when what was read holds no table', async () => {
    const api = mockApi({
      [READ_ITEM]: () =>
        ok(
          sourceRead(PATH, '', {
            warnings: [
              { code: 'nothing_located', message: 'no converted text lies in the selection' },
            ],
          }),
        ),
    });
    renderSheet({
      kind: 'cells',
      range: { sheet: 'Q9', row_start: 1, row_end: 2, column_start: 1, column_end: 1 },
    });
    expect(await screen.findByText('no converted text lies in the selection')).toBeTruthy();
    expect(screen.getByText('The text read holds no table.')).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('asks the next part with the cursor, one part per press, and joins a table cut between parts', async () => {
    const firstPart = ['| Quarter | Revenue |', '| --- | --- |', '| Q1 | 1.2 |', ''].join('\n');
    const secondPart = ['| Q2 | 1.4 |', '| Q3 | 1.9 |', ''].join('\n');
    const api = mockApi({
      [READ_ITEM]: (body) =>
        body.cursor === 'part-2'
          ? ok(
              sourceRead(PATH, secondPart, {
                source: {
                  workspace_id: WS,
                  item_id: SOURCE,
                  path: PATH,
                  revision: HEAD,
                  selection: { kind: 'lines', range: { start: 4, end: 5 } },
                  locations: [],
                },
              }),
            )
          : ok(
              sourceRead(PATH, firstPart, {
                truncated: true,
                next_cursor: 'part-2',
                source: {
                  workspace_id: WS,
                  item_id: SOURCE,
                  path: PATH,
                  revision: HEAD,
                  selection: { kind: 'lines', range: { start: 1, end: 3 } },
                  locations: [],
                },
              }),
            ),
    });
    renderSheet();
    expect(await screen.findByText('Table 1 of 1 read so far: 2 columns, 1 row')).toBeTruthy();
    expect(
      screen.getByText(
        'Partial reading: this is not all of the whole item. A table cut at the end of a part goes on in the next.',
      ),
    ).toBeTruthy();
    // Nothing more is asked until the person asks it.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(api.callsTo(READ_ITEM)).toHaveLength(1);

    const more = screen.getByRole('button', { name: 'Read more of this spreadsheet' });
    await userEvent.click(more);
    expect(await screen.findByText('Table 1 of 1: 2 columns, 3 rows')).toBeTruthy();
    expect(api.callsTo(READ_ITEM)).toEqual([
      asked({ kind: 'all' }),
      asked({ kind: 'all' }, 'part-2'),
    ]);
    expect(screen.getByRole('table').getAttribute('aria-rowcount')).toBe('4');
    expect(screen.queryByRole('button', { name: 'Read more of this spreadsheet' })).toBeNull();
    expect(screen.getByText('Part 2 read.')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('draws only the rows in view of a long table, and says how many there are', async () => {
    // happy-dom lays nothing out: the region is given the height a browser gives it (28rem).
    vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(448);
    const rows = Array.from({ length: 400 }, (_, index) => `| r${index + 1} | ${index} |`);
    const api = mockApi({
      [READ_ITEM]: () =>
        ok(sourceRead(PATH, ['| Name | Value |', '| --- | --- |', ...rows].join('\n'))),
    });
    renderSheet();
    const table = await screen.findByRole('table');
    expect(within(table).getByText('Table 1 of 1: 2 columns, 400 rows')).toBeTruthy();
    expect(table.getAttribute('aria-rowcount')).toBe('401');
    const drawn = within(table).getAllByRole('rowheader');
    expect(drawn.length).toBeGreaterThan(0);
    expect(drawn.length).toBeLessThan(400);
    // Each row drawn says where it is among them all.
    expect(drawn.at(0)?.closest('tr')?.getAttribute('aria-rowindex')).toBe('2');
    expect(api.violations).toEqual([]);
  });

  it('shows a refused read in words, with its retry', async () => {
    let refuse = true;
    const api = mockApi({
      [READ_ITEM]: () =>
        refuse ? fail(500, 'internal', 'the store failed') : ok(sourceRead(PATH, revenue)),
    });
    renderSheet();
    expect(await screen.findByText(/Reading the spreadsheet:/)).toBeTruthy();
    refuse = false;
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(screen.getByRole('table')).toBeTruthy());
    expect(api.violations).toEqual([]);
  });
});
