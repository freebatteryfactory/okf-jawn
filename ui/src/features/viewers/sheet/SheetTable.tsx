/**
 * One table of a spreadsheet's text (ruling A2), drawn with TanStack Table (the row and column
 * model) and, from `VIRTUAL_FROM` rows, TanStack Virtual (only the rows in view are drawn).
 *
 * It is a real table: a caption that says which table it is of those read and counts its columns
 * and rows, its header row as `th scope="col"`, and each body row headed by its number in the
 * table as `th scope="row"`. That number counts the rows read under the header row; it is not the
 * sheet's row number, which the read does not give (`sheet-read.ts`). The header row stays in
 * view while the table scrolls.
 *
 * Keyboard. The table holds one stop of the Tab order: one of its cells (at first the first
 * column's header). From there the arrow keys move from cell to cell, Home and End to the start
 * and end of a row, Control with Home or End to the first and last cell, Page Up and Page Down ten
 * rows; the cell reached takes focus and the scrolling region follows it, sideways too. A cell
 * clicked becomes the stop. So a wide or long table is read and scrolled from the keyboard
 * without the region itself being a stop.
 *
 * Drawn in part, a table keeps its table semantics: it says how many rows it has
 * (`aria-rowcount`) and each drawn row says which it is (`aria-rowindex`); the height of the rows
 * not drawn is held by spacer rows whose one cell is hidden from assistive technology. The row
 * that holds the stop is always drawn, wherever the region is scrolled, so keyboard focus is never
 * taken from it by a scroll.
 *
 * The scrolling region carries `data-sheet-table` (its number) and `data-rows` (its rows): the
 * viewer reads them to find, after the next part is read, the table that grew, and moves keyboard
 * focus there (`focusTable`).
 */
import { createColumnHelper, tableFeatures, useTable } from '@tanstack/react-table';
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import {
  type FocusEvent,
  type KeyboardEvent,
  type ReactNode,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { captionOf, type SheetGrid } from './sheet-read';

/** From this many rows, only the rows in view are drawn. */
export const VIRTUAL_FROM = 100;

/** A row's height before it is measured: one line of text with the cell's padding and rule. */
const ROW_ESTIMATE_PX = 34;

/** How many rows Page Up and Page Down move. */
const PAGE_ROWS = 10;

interface SheetRow {
  /** Its place among the rows read, from zero. */
  index: number;
  cells: readonly string[];
}

/** A cell: `row` 0 is the header row, `column` 0 the row numbers. */
interface Place {
  row: number;
  column: number;
}

const features = tableFeatures({});
const helper = createColumnHelper<typeof features, SheetRow>();

const clamp = (value: number, last: number) => Math.max(0, Math.min(last, value));

/** Where a key moves the stop, in a table of `rows` body rows and `columns` data columns. */
function moved(place: Place, event: KeyboardEvent, rows: number, columns: number): Place | null {
  const across = (column: number) => ({ row: place.row, column: clamp(column, columns) });
  const down = (row: number) => ({ row: clamp(row, rows), column: place.column });
  switch (event.key) {
    case 'ArrowRight':
      return across(place.column + 1);
    case 'ArrowLeft':
      return across(place.column - 1);
    case 'ArrowDown':
      return down(place.row + 1);
    case 'ArrowUp':
      return down(place.row - 1);
    case 'PageDown':
      return down(place.row + PAGE_ROWS);
    case 'PageUp':
      return down(place.row - PAGE_ROWS);
    case 'Home':
      return event.ctrlKey ? { row: 0, column: 0 } : across(0);
    case 'End':
      return event.ctrlKey ? { row: rows, column: columns } : across(columns);
    default:
      return null;
  }
}

const placeOf = (place: Place) => `${place.row}:${place.column}`;

/** The cell an event happened in, as a place. */
function placeIn(target: EventTarget): Place | null {
  if (!(target instanceof HTMLElement)) return null;
  const found = /^(\d+):(\d+)$/.exec(
    target.closest<HTMLElement>('[data-place]')?.dataset.place ?? '',
  );
  return found === null ? null : { row: Number(found[1]), column: Number(found[2]) };
}

/** Move keyboard focus to a table of the page, by its number: to its stop. */
export function focusTable(region: HTMLElement): void {
  region.querySelector<HTMLElement>('[data-place][tabindex="0"]')?.focus();
}

/** A row that holds the height of rows not drawn; its one cell is hidden from assistive technology. */
function Spacer({ height, width }: { height: number; width: number }) {
  return (
    <tr>
      <td aria-hidden="true" colSpan={width} className="border-0 p-0" style={{ height }} />
    </tr>
  );
}

const CELL = 'focus-visible:outline-offset-[-3px]';
const HEADER = `sticky top-0 bg-[Canvas] ${CELL}`;

export interface SheetTableProps {
  grid: SheetGrid;
  /** Which table it is, from one, among `total` read. */
  number: number;
  total: number;
  /** The whole selection has been read: no row of this table is still to come. */
  complete: boolean;
  /** The element that says how the keyboard moves in a table. */
  describedBy?: string | undefined;
}

export function SheetTable({ grid, number, total, complete, describedBy }: SheetTableProps) {
  const columns = useMemo(
    () =>
      helper.columns(
        grid.headers.map((name, column) =>
          helper.accessor((row) => row.cells[column] ?? '', {
            id: `column-${column}`,
            header: () =>
              name === '' ? <span className="sr-only">Column {column + 1}, no heading</span> : name,
          }),
        ),
      ),
    [grid.headers],
  );
  const data = useMemo(() => grid.rows.map((cells, index) => ({ index, cells })), [grid.rows]);
  const table = useTable({ features, columns, data, getRowId: (row) => String(row.index) });
  const rows = table.getRowModel().rows;
  const lastColumn = grid.headers.length;

  // The one stop of the Tab order, and whether focus is to follow it once it is drawn.
  const [chosen, setStop] = useState<Place>({ row: 0, column: Math.min(1, lastColumn) });
  const stop = { row: clamp(chosen.row, rows.length), column: clamp(chosen.column, lastColumn) };
  const follow = useRef(false);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const drawnInPart = rows.length >= VIRTUAL_FROM;
  const stopIndex = stop.row - 1;
  const virtualizer = useVirtualizer({
    count: drawnInPart ? rows.length : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_ESTIMATE_PX,
    getItemKey: (index) => rows[index]?.id ?? index,
    overscan: 10,
    // The stop's row is drawn wherever the region is scrolled.
    rangeExtractor: (range) => {
      const indexes = defaultRangeExtractor(range);
      if (stopIndex < 0 || stopIndex >= range.count || indexes.includes(stopIndex)) return indexes;
      return [...indexes, stopIndex].sort((a, b) => a - b);
    },
  });

  useLayoutEffect(() => {
    if (!follow.current) return;
    const cell = scrollRef.current?.querySelector<HTMLElement>(`[data-place="${placeOf(stop)}"]`);
    if (cell === null || cell === undefined) return;
    follow.current = false;
    cell.focus();
  });
  const onKeyDown = (event: KeyboardEvent) => {
    const next = moved(stop, event, rows.length, lastColumn);
    if (next === null) return;
    event.preventDefault();
    follow.current = true;
    setStop(next);
  };
  const onFocus = (event: FocusEvent) => {
    const place = placeIn(event.target);
    if (place !== null && placeOf(place) !== placeOf(stop)) setStop(place);
  };
  const cellAt = (row: number, column: number) => ({
    'data-place': `${row}:${column}`,
    tabIndex: row === stop.row && column === stop.column ? 0 : -1,
  });

  const width = lastColumn + 1;
  const rowAt = (at: number): ReactNode => {
    const row = rows[at];
    if (row === undefined) return null;
    return (
      <tr
        key={row.id}
        aria-rowindex={at + 2}
        {...(drawnInPart ? { 'data-index': at, ref: virtualizer.measureElement } : {})}
      >
        <th scope="row" className={CELL} {...cellAt(at + 1, 0)}>
          {at + 1}
        </th>
        {row.getAllCells().map((cell, column) => (
          <td key={cell.id} className={CELL} {...cellAt(at + 1, column + 1)}>
            <table.FlexRender cell={cell} />
          </td>
        ))}
      </tr>
    );
  };
  const body: ReactNode[] = [];
  if (drawnInPart) {
    let drawnTo = 0;
    for (const item of virtualizer.getVirtualItems()) {
      if (item.start > drawnTo) {
        body.push(
          <Spacer key={`before-${item.index}`} height={item.start - drawnTo} width={width} />,
        );
      }
      body.push(rowAt(item.index));
      drawnTo = item.end;
    }
    const rest = virtualizer.getTotalSize() - drawnTo;
    if (rest > 0) body.push(<Spacer key="after" height={rest} width={width} />);
  } else {
    body.push(...rows.map((_, at) => rowAt(at)));
  }

  return (
    <div
      ref={scrollRef}
      className="my-3 max-h-[28rem] overflow-auto"
      data-sheet-table={number}
      data-rows={rows.length}
    >
      <table
        aria-rowcount={rows.length + 1}
        aria-describedby={describedBy}
        className="m-0"
        onKeyDown={onKeyDown}
        onFocus={onFocus}
      >
        <caption className="py-1 text-left font-semibold">
          {captionOf(number, total, grid, complete)}
        </caption>
        <thead>
          {table.getHeaderGroups().map((group) => (
            <tr key={group.id} aria-rowindex={1}>
              <th scope="col" className={HEADER} {...cellAt(0, 0)}>
                Row
              </th>
              {group.headers.map((header, column) => (
                <th key={header.id} scope="col" className={HEADER} {...cellAt(0, column + 1)}>
                  {header.isPlaceholder ? null : <table.FlexRender header={header} />}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>{body}</tbody>
      </table>
    </div>
  );
}
