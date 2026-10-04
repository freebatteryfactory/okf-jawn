/** Accessible data projection; displayed numbers come from resolved data, not model props. */
export interface DataTableProps {
  rows: ReadonlyArray<Readonly<Record<string, string | number | boolean | null>>>;
  caption: string;
}
export function DataTable({ rows, caption }: DataTableProps) {
  const columns = [...new Set(rows.flatMap(row => Object.keys(row)))];
  return <table><caption>{caption}</caption><thead><tr>{columns.map(column => <th key={column} scope="col">{column}</th>)}</tr></thead>
    <tbody>{rows.map((row, index) => <tr key={JSON.stringify([row, index])}>{columns.map(column => <td key={column}>{String(row[column] ?? '')}</td>)}</tr>)}</tbody></table>;
}
