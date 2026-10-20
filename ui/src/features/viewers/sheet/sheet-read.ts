/**
 * What the read of a spreadsheet gives, and nothing it does not (ruling A2; SPEC section 5:
 * "Spreadsheets expose sheet names, columns, counts, samples and bounded row/column selection …
 * Formula/value/format distinctions and extraction limitations must be visible where they
 * matter").
 *
 * Traced in the service, not assumed:
 * - The text is the converted Markdown (`crates/core/src/application/reads.rs`): docling writes
 *   each data region of each sheet as one pipe table, in sheet order, with no heading per sheet
 *   (the pinned docling 2.3.0 `backend/xlsx.rs`; a CSV is one table). So a table's columns are its
 *   header row and its rows are the rows read; which sheet a table is on, and where in the sheet
 *   it starts, are not in the text.
 * - Sheet names reach the wire in four places only, each used here: an outline entry of kind
 *   `sheet`, the `cell_values_only` finding of the extraction (its `sheets`), a `cells` selection
 *   and a `cells` location. The import writes neither sheet entries nor cell locations today
 *   (`crates/ingest/src/outline.rs` makes headings only; line locations are pages), so for a
 *   converted workbook the read usually names none, and the viewer says so.
 * - Formulas and number formats are on no field of the wire. The one distinction it carries is
 *   `cell_values_only`: those sheets' cells are the values the file stores. Anything else is said
 *   to be not given, never guessed.
 * - A `cells` selection selects the lines whose recorded location overlaps the range, whole
 *   (`reads.rs`, `located`): the tables returned are not cut to the range, and the answer does not
 *   say which of their rows the range is.
 */
import type {
  ExtractionWarning,
  ReadItemResponse,
  Selection,
  SourceAppearance,
} from '../../../api/generated/types.gen';

/** The part of a hast node a table is read from (`react-markdown` hands the table's element). */
export interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Readonly<Record<string, unknown>>;
  children?: readonly HastNode[];
}

/** One table of the text: its header row and its body rows, each as wide as the header. */
export interface SheetGrid {
  headers: readonly string[];
  rows: readonly (readonly string[])[];
}

const unique = (names: readonly string[]) => [...new Set(names)];

/** The sheet names the read gives, in the order it gives them. */
export function sheetNames(
  pages: readonly ReadItemResponse[],
  appearance: SourceAppearance,
  selection: Selection,
): string[] {
  const names: string[] = [];
  for (const page of pages) {
    for (const entry of page.outline) if (entry.kind === 'sheet') names.push(entry.label);
  }
  names.push(...valuesOnlySheets(appearance.extraction.warnings));
  if (selection.kind === 'cells') names.push(selection.range.sheet);
  for (const page of pages) {
    for (const location of page.source.locations ?? []) {
      if (location.provenance !== 'unresolved' && location.locator.kind === 'cells') {
        names.push(location.locator.range.sheet);
      }
    }
  }
  return unique(names);
}

/** The sheets whose cells the conversion read as stored values, without formulas or formats. */
export function valuesOnlySheets(warnings: readonly ExtractionWarning[]): string[] {
  return unique(
    warnings.flatMap((warning) => (warning.kind === 'cell_values_only' ? warning.sheets : [])),
  );
}

/** Names in words: “Q1”, “Q2”. */
export const quoted = (names: readonly string[]) => names.map((name) => `“${name}”`).join(', ');

/** The text of a node, as its text nodes give it. */
function textOf(node: HastNode): string {
  if (node.type === 'text') return node.value ?? '';
  return (node.children ?? []).map(textOf).join('');
}

const elements = (node: HastNode | undefined, tagName: string): HastNode[] =>
  (node?.children ?? []).filter((child) => child.type === 'element' && child.tagName === tagName);

const cellsOf = (row: HastNode): string[] =>
  (row.children ?? [])
    .filter(
      (child) => child.type === 'element' && (child.tagName === 'th' || child.tagName === 'td'),
    )
    .map((cell) => textOf(cell).trim());

/**
 * A GFM table's header and rows, as the Markdown parser built it. A body row is as wide as the
 * header: GFM drops cells past it, and a row with fewer cells has empty ones.
 */
export function gridOf(table: HastNode): SheetGrid {
  const header = elements(elements(table, 'thead')[0], 'tr')[0];
  const headers = header === undefined ? [] : cellsOf(header);
  const rows = elements(elements(table, 'tbody')[0], 'tr').map((row) => {
    const cells = cellsOf(row);
    return headers.map((_, index) => cells[index] ?? '');
  });
  return { headers, rows };
}

const counted = (count: number, one: string, many: string) =>
  `${count.toLocaleString('en')} ${count === 1 ? one : many}`;

/** A table's caption: which it is of those read, and how many columns and rows it has. */
export function captionOf(
  number: number,
  total: number,
  grid: SheetGrid,
  complete: boolean,
): string {
  return `Table ${number} of ${total}${complete ? '' : ' read so far'}: ${counted(grid.headers.length, 'column', 'columns')}, ${counted(grid.rows.length, 'row', 'rows')}`;
}
