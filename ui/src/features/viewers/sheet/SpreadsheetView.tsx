/**
 * The spreadsheet viewer (UI M4a, ruling A2): a spreadsheet as tables with ranges.
 *
 * A workbook's bytes are not text, and this page has no parser of its own for them, so the
 * viewer shows what the service's read gives of the spreadsheet: its converted text, read as the
 * reader reads it (`text/item-text.ts`: the exact revision, the reader's selection and budget),
 * with each table drawn as a table (`SheetTable`): its caption counts its columns and rows, its
 * column headers and row numbers are `th` with a scope, and it sits in a region keyboard focus
 * reaches. What the read gives and does not give is traced in `sheet-read.ts`; the viewer says
 * both:
 *
 * - the sheets it names, or that it names none;
 * - which sheets' cells are values only (`cell_values_only`), or that it gives no formula, value
 *   or format distinction at all;
 * - for a `cells` selection, the range asked, and that the tables answered are not cut to it.
 *
 * Whose text the tables are (an agent's, a person's correction) is said as the reader says it.
 *
 * Bounded: the read answers at most the reader's budget at a time. A partial read says so and
 * offers the next part, asked with its cursor, one part per press: the viewer never reads on by
 * itself. The parts are joined before they are parsed, so a table cut between two parts is one
 * table again. After the next part is read, keyboard focus goes to the first table that grew or
 * is new (else to the end of the reading), never to the page body; a part that fails leaves focus
 * on the button, under the failure's notice.
 */
import { useId, useLayoutEffect, useRef } from 'react';
import type { Selection } from '../../../api/generated/types.gen';
import { describeCells, describeSelection, textLabels } from '../../documents/describe';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { continuationStatus, joinedText, useItemText } from '../text/item-text';
import type { OriginalProps } from '../types';
import { SheetMarkdown } from './SheetMarkdown';
import { focusTable } from './SheetTable';
import { quoted, sheetNames, valuesOnlySheets } from './sheet-read';

/** The rows of each table on the page, by its number. */
function tableRows(root: HTMLElement | null): Map<string, string> {
  const rows = new Map<string, string>();
  for (const region of root?.querySelectorAll<HTMLElement>('[data-sheet-table]') ?? []) {
    rows.set(region.dataset.sheetTable ?? '', region.dataset.rows ?? '');
  }
  return rows;
}

function RangeAsked({ selection }: { selection: Selection }) {
  if (selection.kind !== 'cells') return null;
  return (
    <p role="note" className="notice">
      Range asked: {describeCells(selection.range)}. The read gives the converted tables that lie in
      this range, whole; it does not say which of their rows and columns are the range, so none is
      marked.
    </p>
  );
}

export function SpreadsheetView({
  workspaceId,
  itemId,
  revision,
  path,
  appearance,
  selection,
}: OriginalProps) {
  const { request, read } = useItemText(workspaceId, itemId, revision, selection);
  const pages = read.data?.pages ?? [];
  const first = pages[0];
  const last = pages.at(-1);
  const complete = pages.length > 0 && !read.hasNextPage;
  const names = sheetNames(pages, appearance, selection);
  const valuesOnly = valuesOnlySheets(appearance.extraction.warnings);
  const warnings = pages.flatMap((page) => page.warnings);
  const asked = describeSelection(selection);
  const keysId = useId();

  // "Read more" moves keyboard focus to the first table the part grew or added; when the part
  // held no table, to the end of the reading. A part that failed leaves focus on the button.
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const endRef = useRef<HTMLParagraphElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const awaited = useRef<{ part: number; rows: Map<string, string> } | null>(null);
  const partCount = pages.length;
  const continuing = read.isFetchingNextPage;
  const continuationFailed = read.isFetchNextPageError;
  useLayoutEffect(() => {
    const waiting = awaited.current;
    if (waiting === null) return;
    if (partCount > waiting.part) {
      awaited.current = null;
      const grown = [...tableRows(bodyRef.current)].find(
        ([number, rows]) => waiting.rows.get(number) !== rows,
      );
      const table =
        grown === undefined
          ? null
          : bodyRef.current?.querySelector<HTMLElement>(`[data-sheet-table="${grown[0]}"]`);
      if (table === null || table === undefined) endRef.current?.focus();
      else focusTable(table);
    } else if (continuationFailed && !continuing) {
      awaited.current = null;
      moreRef.current?.focus();
    }
  }, [partCount, continuing, continuationFailed]);
  const readMore = () => {
    if (continuing) return;
    awaited.current = { part: pages.length, rows: tableRows(bodyRef.current) };
    void read.fetchNextPage();
  };

  const reading = JSON.stringify(request);
  const readFailure = useRetry({
    request: reading,
    failed: read.isError && !read.isFetchNextPageError,
    error: read.error,
    run: () => read.refetch(),
    loaded: () =>
      bodyRef.current?.querySelector<HTMLElement>('[data-place][tabindex="0"]') ?? endRef.current,
  });
  const moreFailure = useRetry({
    request: reading,
    failed: read.isFetchNextPageError,
    error: read.error,
    run: () => read.fetchNextPage(),
    loaded: () => endRef.current,
  });

  return (
    <div className="sheet-view">
      <p>
        Tables of {path}, as its converted text gives them: {asked}.
      </p>
      <RangeAsked selection={selection} />
      <p>
        {names.length > 0
          ? `Sheets the read names: ${quoted(names)}.`
          : 'The read names no sheet: which sheet a table is on is not known here.'}
      </p>
      <p>
        {valuesOnly.length > 0
          ? `Values only in ${quoted(valuesOnly)}: each cell shows the value the file stores; formulas and number formats were not read.`
          : 'The read gives each cell as text: whether a cell holds a formula or a value, and how it is formatted, is not given.'}
      </p>
      {textLabels(first?.extraction).map((label) => (
        <p key={label} role="note" className="notice">
          {label}
        </p>
      ))}

      {read.isPending && !readFailure.retrying && <p role="status">Reading the spreadsheet…</p>}
      {readFailure.shown && (
        <FailureNotice
          context="Reading the spreadsheet"
          error={readFailure.error}
          retry={readFailure}
          workspaceId={workspaceId}
        />
      )}
      {pages.length > 0 && (
        <div ref={bodyRef}>
          <p id={keysId} className="meta">
            In a table, the arrow keys move from cell to cell; Home and End go to the start and end
            of a row, and with Control to the first and last cell.
          </p>
          <SheetMarkdown markdown={joinedText(pages)} complete={complete} describedBy={keysId} />
        </div>
      )}
      {warnings.map((warning) => (
        <p key={`${warning.code} ${warning.message}`} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}
      <p role="status" className="sr-only">
        {continuationStatus(read, pages.length, moreFailure.retrying)}
      </p>
      {last?.truncated && (
        <div role="note" className="notice">
          <p>
            Partial reading: this is not all of {asked}. A table cut at the end of a part goes on in
            the next.
            {last.next_cursor ? '' : ' The rest cannot be read from here.'}
          </p>
          {read.hasNextPage && (
            <button type="button" ref={moreRef} aria-disabled={continuing} onClick={readMore}>
              {continuing ? 'Reading more…' : 'Read more of this spreadsheet'}
            </button>
          )}
        </div>
      )}
      {moreFailure.shown && (
        <FailureNotice
          context="Reading more of the spreadsheet"
          error={moreFailure.error}
          retry={moreFailure}
          workspaceId={workspaceId}
        />
      )}
      {pages.length > 0 && (
        <p ref={endRef} tabIndex={-1} className="meta">
          {complete
            ? `Read to the end of ${asked}${pages.length > 1 ? `, in ${pages.length} parts` : ''}.`
            : `${pages.length === 1 ? 'One part' : `${pages.length} parts`} of ${asked} read so far.`}
        </p>
      )}
    </div>
  );
}
