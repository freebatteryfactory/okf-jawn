/** Resolve a presentation through the host's `ViewData` and preserve exact source bindings. */
import { useEffect, useState } from 'react';
import { compile, type TopLevelSpec } from 'vega-lite';
import type { z } from 'zod';
import {
  zDataset,
  type zDatasetValue,
  type zPresentResponse,
  type zReadItemResponse,
  type zViewBinding,
} from '../../api/generated/zod.gen';
import type { ResolvedPresentation } from './Bindings';
import { BindingsContext } from './Bindings';
import { Chart } from './Chart';
import { readDataset, readSource, type ViewData } from './data';
import { Layout } from './Layout';
import { Resolution, warnedBinding } from './Resolution';
import { withBindingTransforms } from './transforms';

export interface PresentViewProps {
  response: z.infer<typeof zPresentResponse>;
  /** The host's reads of the sources and datasets the View cites. */
  data: ViewData;
}
type Row = Readonly<Record<string, string | number | boolean | null>>;

/**
 * The records a chart and a table draw, one per row, keyed by the dataset's column names. A row
 * whose width differs from the columns is refused, never padded or cut. Two columns of one name are
 * refused too: the producer guarantees distinct names, so a duplicate is a malformed payload and
 * the later column would silently replace the earlier one.
 */
export function datasetRecords(data: z.infer<typeof zDataset>): ReadonlyArray<Row> {
  const names = new Set<string>();
  for (const column of data.columns) {
    if (names.has(column.name)) throw new Error(`Dataset has two columns named "${column.name}"`);
    names.add(column.name);
  }
  return data.rows.map((cells, index) => {
    if (cells.length !== data.columns.length)
      throw new Error(
        `Dataset row ${index} has ${cells.length} values for ${data.columns.length} columns`,
      );
    return Object.fromEntries(data.columns.map((column, at) => [column.name, cells[at] ?? null]));
  });
}
type Cell = z.infer<typeof zDatasetValue>;
type Kind = z.infer<typeof zDataset>['columns'][number]['kind'];
const timestampShape = /^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-5][0-9]:[0-5][0-9].[0-9]{3}Z$/;

/**
 * Whether a cell is null or of its column's kind, as the contract's `DatasetValue::fits` reads it:
 * an integer is a whole number that fits 64 bits; a date_time is the one canonical spelling
 * `YYYY-MM-DDTHH:MM:SS.sssZ` of an instant that exists. Limitation: a JSON `1.0` parses to the
 * number 1 here and is taken as an integer; the server's parser would call it a float.
 */
export function fits(value: Cell, kind: Kind): boolean {
  if (value === null) return true;
  switch (kind) {
    case 'boolean':
      return typeof value === 'boolean';
    case 'number':
      return typeof value === 'number';
    case 'string':
      return typeof value === 'string';
    case 'integer':
      return (
        typeof value === 'number' &&
        Number.isInteger(value) &&
        value >= -(2 ** 63) &&
        value < 2 ** 64
      );
    case 'date_time': {
      if (typeof value !== 'string' || !timestampShape.test(value)) return false;
      const instant = new Date(value);
      return !Number.isNaN(instant.getTime()) && instant.toISOString() === value;
    }
  }
}

function firstMisfit(data: z.infer<typeof zDataset>) {
  for (const [row, cells] of data.rows.entries())
    for (const [column, definition] of data.columns.entries()) {
      const value = cells[column];
      if (value !== undefined && !fits(value, definition.kind))
        return { row, column, name: definition.name };
    }
  return undefined;
}

/** A citation's identity: everything but the derived `locations`, in a key order of its own. */
function sourceKey(source: z.infer<typeof zViewBinding>['source']): string {
  return JSON.stringify([
    source.workspace_id,
    source.item_id,
    source.path,
    source.revision,
    source.digest ?? null,
    canonical(source.selection),
  ]);
}
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (typeof value === 'object' && value !== null)
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => (a < b ? -1 : 1))
        .map(([key, inner]) => [key, canonical(inner)]),
    );
  return value;
}

/**
 * Read the bytes of a retained dataset. A dataset is read only from the converter's text (SPEC
 * R5, decision O2): any other origin, or any shape the generated `zDataset` refuses, is that
 * chart's own alert with the reason, never a drawn chart.
 */
export function parseDataset(
  text: string,
  expected: z.infer<typeof zViewBinding>['source'],
): ReadonlyArray<Row> {
  const parsed = zDataset.safeParse(JSON.parse(text));
  if (!parsed.success) {
    const issue = parsed.error.issues.at(0);
    const where = issue && issue.path.length > 0 ? ` at ${issue.path.join('.')}` : '';
    throw new Error(`Dataset is malformed${where}: ${issue?.message ?? 'invalid'}`);
  }
  if (parsed.data.text_origin !== 'converter')
    throw new Error(
      `Dataset text is not the converter's (text_origin is ${parsed.data.text_origin}); no chart is drawn from it`,
    );
  if (sourceKey(parsed.data.source) !== sourceKey(expected))
    throw new Error(
      'Dataset was read from a different source than its binding cites (stale); no chart is drawn from it',
    );
  const misfit = firstMisfit(parsed.data);
  if (misfit)
    throw new Error(
      `Dataset is malformed at rows.${misfit.row}.${misfit.column}: a value of column ${misfit.name} is not of its kind`,
    );
  return datasetRecords(parsed.data);
}
/** Runtime-validate a Vega-Lite grammar value (SPEC §10); never cast untrusted specs. */
export function parseVegaLiteSpec(value: unknown): TopLevelSpec {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('Vega-Lite specification must be a JSON object');
  }
  const candidate = value as TopLevelSpec;
  compile(candidate);
  return candidate;
}

const failureLabel = {
  invalid_spec: 'Invalid specification',
  unknown_binding: 'Unknown binding',
  dataset_unavailable: 'Dataset unavailable',
  invalidated: 'Invalidated',
  too_large: 'Dataset too large',
} as const;

/**
 * Each named chart's specification, or why it is refused. One chart that compile rejects, or that
 * the server reports failed, is that chart's error and nothing else's (SPEC §10). The server's
 * diagnostic is the source of truth for its own failure; a chart it calls ready is still judged
 * here, because renderer specs need runtime validation as well as the wire schema.
 */
/** Each binding's declared transforms, by its name. */
function bindingTransforms(
  bindings: z.infer<typeof zPresentResponse>['resolved_bindings'],
): ReadonlyMap<string, readonly unknown[]> {
  return new Map(bindings.map((binding) => [binding.name, binding.transforms]));
}

function chartsFromView(
  view: z.infer<typeof zPresentResponse>['view'],
  statuses: z.infer<typeof zPresentResponse>['charts'],
  transforms: ReadonlyMap<string, readonly unknown[]>,
): {
  charts: Map<string, TopLevelSpec>;
  errors: Map<string, string>;
  reads: Map<string, readonly string[]>;
} {
  const charts = new Map<string, TopLevelSpec>();
  const errors = new Map<string, string>();
  const reads = new Map<string, readonly string[]>();
  for (const [name, value] of Object.entries(view.charts ?? {})) {
    try {
      charts.set(name, parseVegaLiteSpec(withBindingTransforms(value, transforms)));
    } catch (cause) {
      errors.set(
        name,
        cause instanceof Error
          ? `Chart "${name}" failed validation: ${cause.message}`
          : `Chart "${name}" failed validation`,
      );
    }
  }
  for (const { chart, status } of statuses) {
    const name = chart.kind === 'named' ? chart.name : '';
    if (status.status === 'ready') reads.set(name, status.bindings);
    else errors.set(name, `${failureLabel[status.reason]}: ${status.message}`);
  }
  return { charts, errors, reads };
}

function reason(cause: unknown, fallback: string): string {
  return cause instanceof Error ? cause.message : fallback;
}

export function PresentView({ response, data }: PresentViewProps) {
  const [bindings, setBindings] = useState<ResolvedPresentation | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    const load = async () => {
      const {
        charts,
        errors: chartErrors,
        reads: chartBindings,
      } = chartsFromView(
        response.view,
        response.charts,
        bindingTransforms(response.resolved_bindings),
      );
      const sources = new Map<string, z.infer<typeof zReadItemResponse>>();
      const definitions = new Map<string, z.infer<typeof zViewBinding>>();
      const tables = new Map<string, ReadonlyArray<Row>>();
      const datasetErrors = new Map<string, string>();
      const sourceErrors = new Map<string, string>();
      // A binding that cannot be read costs only what cites it: its source excerpt, its table and
      // its charts show why. The View's text and every other binding still render (SPEC §10).
      for (const binding of response.resolved_bindings) {
        definitions.set(binding.name, binding);
        try {
          sources.set(binding.name, await readSource(data, binding));
        } catch (cause) {
          sourceErrors.set(binding.name, reason(cause, 'The source could not be read'));
        }
        try {
          const text = await readDataset(data, binding);
          if (text !== undefined) tables.set(binding.name, parseDataset(text, binding.source));
        } catch (cause) {
          datasetErrors.set(binding.name, reason(cause, 'The dataset could not be read'));
        }
      }
      // The server's own reason a dataset is missing, by the binding its warning locates.
      const datasetWarnings = new Map<string, string>();
      for (const warning of response.warnings) {
        const binding = warnedBinding(response.view, warning.location);
        if (warning.code === 'dataset_unavailable' && binding !== undefined)
          datasetWarnings.set(binding, warning.message);
      }
      if (active)
        setBindings({
          sources,
          bindings: definitions,
          tables,
          charts,
          datasetErrors,
          datasetWarnings,
          sourceErrors,
          chartErrors,
          chartBindings,
        });
    };
    // Each binding and chart is caught on its own; this is for what nobody foresaw, so that the
    // View never waits on "Resolving…" forever.
    load().catch((cause) => {
      if (active) setFailure(reason(cause, 'The presentation could not be resolved.'));
    });
    return () => {
      active = false;
    };
  }, [response, data]);
  if (failure !== null) return <p role="alert">{failure}</p>;
  if (!bindings) return <p role="status">Resolving the presentation's exact source references…</p>;
  const view = response.view;
  // Either grammar: the View's own title and description, its drawing, then what it was read from.
  return (
    <BindingsContext.Provider value={bindings}>
      <section>
        <h2>{view.title}</h2>
        <p>{view.description}</p>
        {view.grammar === 'json_render' ? (
          <Layout spec={view.spec} />
        ) : (
          <VegaLiteChart view={view} resolved={bindings} response={response} />
        )}
        <Resolution response={response} />
      </section>
    </BindingsContext.Provider>
  );
}

/** The one chart of a `vega_lite` View: its own alert when it cannot be drawn, never the View's. */
function VegaLiteChart({
  view,
  resolved,
  response,
}: {
  view: z.infer<typeof zPresentResponse>['view'];
  resolved: ResolvedPresentation;
  response: z.infer<typeof zPresentResponse>;
}) {
  const failed = resolved.chartErrors?.get('');
  if (failed !== undefined) return <p role="alert">{failed}</p>;
  let chartSpec: TopLevelSpec;
  try {
    chartSpec = parseVegaLiteSpec(
      withBindingTransforms(view.spec, bindingTransforms(response.resolved_bindings)),
    );
  } catch (cause) {
    return <p role="alert">{reason(cause, 'Vega-Lite specification failed runtime validation')}</p>;
  }
  const wanted =
    'data' in chartSpec && chartSpec.data && 'name' in chartSpec.data
      ? chartSpec.data.name
      : undefined;
  // What the server says the chart reads; else the dataset its spec names, else the first binding.
  const fallback = wanted ?? response.resolved_bindings.at(0)?.name;
  const reads = resolved.chartBindings?.get('') ?? (fallback === undefined ? [] : [fallback]);
  const unmaterialized =
    'A chart requires a retained materialized dataset. No model-provided data was substituted.';
  if (reads.length === 0) return <p role="alert">{unmaterialized}</p>;
  const datasets = [];
  const cited = [];
  for (const name of reads) {
    const binding = resolved.bindings.get(name);
    if (!binding)
      return (
        <p role="alert">
          {`Unknown binding "${name}": the chart names a binding this View does not resolve.`}
        </p>
      );
    const rows = resolved.tables.get(name);
    if (!rows) return <p role="alert">{resolved.datasetErrors?.get(name) ?? unmaterialized}</p>;
    datasets.push({ name, rows });
    cited.push(binding);
  }
  return (
    <>
      <Chart spec={chartSpec} datasets={datasets} />
      {cited.map((binding) => (
        <p key={binding.name}>
          {binding.source.path} @ <code>{binding.source.revision}</code>
        </p>
      ))}
    </>
  );
}
