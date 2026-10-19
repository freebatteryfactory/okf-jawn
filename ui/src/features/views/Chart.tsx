/**
 * Render a validated Vega-Lite spec with an explicit loader boundary and table fallback. The chart
 * is given every dataset it reads, each under its binding's name (Vega-Lite's named `datasets`),
 * so a layered chart over two bindings draws both; each has its own table.
 */
import { useEffect, useRef, useState } from 'react';
import { loader } from 'vega';
import embed from 'vega-embed';
import { expressionInterpreter } from 'vega-interpreter';
import type { TopLevelSpec } from 'vega-lite';
import { DataTable } from './DataTable';

/** One dataset a chart reads: the binding's name and its resolved rows. */
export interface ChartDataset {
  name: string;
  rows: ReadonlyArray<Readonly<Record<string, string | number | boolean | null>>>;
}

export interface ChartProps {
  spec: TopLevelSpec;
  /** Every dataset the chart reads. */
  datasets: ReadonlyArray<ChartDataset>;
}

export function Chart({ spec, datasets }: ChartProps) {
  const container = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const element = container.current;
    if (element === null) return;
    let disposed = false;
    let finalize: (() => void) | undefined;
    const restricted = loader();
    restricted.load = async () => {
      throw new Error('Chart network loading is disabled; use resolved datasets');
    };
    restricted.sanitize = async () => {
      throw new Error('Chart external resources are disabled');
    };
    embed(
      element,
      {
        ...spec,
        datasets: Object.fromEntries(
          datasets.map(({ name, rows }) => [name, rows.map((row) => ({ ...row }))]),
        ),
      },
      {
        mode: 'vega-lite',
        actions: false,
        ast: true,
        expr: expressionInterpreter,
        loader: restricted,
      },
    )
      .then((result) => {
        if (disposed) result.finalize();
        else finalize = result.finalize;
      })
      .catch((cause) => {
        if (!disposed) setError(cause instanceof Error ? cause.message : 'Chart rendering failed');
      });
    return () => {
      disposed = true;
      finalize?.();
    };
  }, [spec, datasets]);
  return (
    <section>
      <div ref={container} />
      {error && <p role="alert">{error}</p>}
      <details>
        <summary>Source data</summary>
        {datasets.map(({ name, rows }) => (
          <DataTable key={name} rows={rows} caption={name} />
        ))}
      </details>
    </section>
  );
}
