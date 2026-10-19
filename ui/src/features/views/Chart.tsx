/**
 * Render a validated Vega-Lite spec with an explicit loader boundary and table fallback. The chart
 * is given every dataset it reads, each under its binding's name (Vega-Lite's named `datasets`),
 * so a layered chart over two bindings draws both; each has its own table.
 *
 * What the chart's lookups read that a binding's transforms make (`derived`, from `planChart`, in
 * the order to make them) is made first, by Vega itself with the same expression interpreter and
 * loader boundary as the chart, over the datasets given and those made before it. Only the
 * bindings' own rows are tabled. A failure to make one is the chart's alert.
 */
import { useEffect, useRef, useState } from 'react';
import { type Loader, loader, parse, View } from 'vega';
import embed from 'vega-embed';
import { expressionInterpreter } from 'vega-interpreter';
import { compile, type TopLevelSpec } from 'vega-lite';
import { DataTable } from './DataTable';
import type { DerivedDataset } from './transforms';

/** One dataset a chart reads: the binding's name and its resolved rows. */
export interface ChartDataset {
  name: string;
  rows: ReadonlyArray<Readonly<Record<string, string | number | boolean | null>>>;
}

export interface ChartProps {
  spec: TopLevelSpec;
  /** Every dataset the chart reads. */
  datasets: ReadonlyArray<ChartDataset>;
  /** What the chart's lookups read that its bindings' transforms make, in the order to make it. */
  derived?: ReadonlyArray<DerivedDataset> | undefined;
}

/** The datasets a chart is drawn over, by name. */
type Values = Record<string, object[]>;

const NOTHING_DERIVED: ReadonlyArray<DerivedDataset> = [];

/** Runtime-validate a Vega-Lite grammar value (SPEC §10); never cast untrusted specs. */
export function parseVegaLiteSpec(value: unknown): TopLevelSpec {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('Vega-Lite specification must be a JSON object');
  }
  const candidate = value as TopLevelSpec;
  compile(candidate);
  return candidate;
}

/** A loader that loads nothing: a chart reads only the datasets it is given. */
function restrictedLoader(): Loader {
  const restricted = loader();
  restricted.load = async () => {
    throw new Error('Chart network loading is disabled; use resolved datasets');
  };
  restricted.sanitize = async () => {
    throw new Error('Chart external resources are disabled');
  };
  return restricted;
}

/** The rows of `step.binding` after its transforms, as Vega makes them over `values`. */
async function make(step: DerivedDataset, values: Values, restricted: Loader): Promise<object[]> {
  const compiled = compile(
    parseVegaLiteSpec({
      data: { name: step.binding },
      transform: step.transform,
      mark: 'point',
      datasets: values,
    }),
  ).spec;
  // An unencoded mark draws every row its data's transforms leave: those rows are what is made.
  const from = compiled.marks?.at(0)?.from;
  const source = from !== undefined && 'data' in from ? from.data : undefined;
  if (source === undefined) throw new Error(`Vega made no data from “${step.binding}”`);
  const view = new View(parse(compiled, undefined, { ast: true }), {
    renderer: 'none',
    expr: expressionInterpreter,
    loader: restricted,
  });
  try {
    await view.runAsync();
    return view.data(source).map((tuple: object) => Object.fromEntries(Object.entries(tuple)));
  } finally {
    view.finalize();
  }
}

export function Chart({ spec, datasets, derived = NOTHING_DERIVED }: ChartProps) {
  const container = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const element = container.current;
    if (element === null) return;
    let disposed = false;
    let finalize: (() => void) | undefined;
    const restricted = restrictedLoader();
    const draw = async () => {
      const values: Values = Object.fromEntries(
        datasets.map(({ name, rows }) => [name, rows.map((row) => ({ ...row }))]),
      );
      for (const step of derived) {
        try {
          values[step.name] = await make(step, values, restricted);
        } catch (cause) {
          const why = cause instanceof Error ? cause.message : 'Vega refused them';
          throw new Error(
            `The transforms of “${step.binding}”, which this chart looks up, failed: ${why}`,
          );
        }
      }
      return embed(
        element,
        { ...spec, datasets: values },
        {
          mode: 'vega-lite',
          actions: false,
          ast: true,
          expr: expressionInterpreter,
          loader: restricted,
        },
      );
    };
    draw()
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
  }, [spec, datasets, derived]);
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
