/** Resolve a saved presentation through host tools and preserve exact source bindings. */
import { useEffect, useState } from 'react';
import { compile, type TopLevelSpec } from 'vega-lite';
import { z } from 'zod';
import {
  zDataset,
  type zDatasetValue,
  zGetObjectResponse,
  type zPresentResponse,
  zReadItemResponse,
  type zViewBinding,
} from '../../api/generated/zod.gen';
import type { ResolvedPresentation } from './Bindings';
import { BindingsContext } from './Bindings';
import { Chart } from './Chart';
import { Layout } from './Layout';

export interface PresentViewProps {
  response: z.infer<typeof zPresentResponse>;
  /** Host tool call; the Apps bridge applies omitUndefined once at the wire boundary. */
  callTool: (name: string, input: Record<string, unknown>) => Promise<unknown>;
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
const toolResult = z.object({
  isError: z.boolean().optional(),
  content: z.array(z.looseObject({ type: z.string(), text: z.string().optional() })).optional(),
  structuredContent: z.unknown().optional(),
});
const refusalLimit = 512;

/** The tool's own refusal text, bounded; cut on a code point boundary, ending with an ellipsis. */
function refusalMessage(output: z.infer<typeof toolResult>, fallback: string): string {
  const text = (output.content ?? [])
    .flatMap((part) => (part.type === 'text' && part.text ? [part.text] : []))
    .join('\n')
    .trim();
  if (text.length === 0) return fallback;
  if (text.length <= refusalLimit) return text;
  let end = refusalLimit - 1;
  const last = text.charCodeAt(end - 1);
  if (last >= 0xd800 && last <= 0xdbff) end -= 1; // do not leave half a surrogate pair
  return `${text.slice(0, end)}…`;
}

/**
 * One host tool call whose refusal is surfaced as the tool's own bounded message. `isError` is
 * checked before `structuredContent` is parsed, because a refusal carries no structured content.
 */
async function callChecked(
  callTool: PresentViewProps['callTool'],
  name: string,
  input: Record<string, unknown>,
  fallback: string,
) {
  const output = toolResult.parse(await callTool(name, input));
  if (output.isError === true) throw new Error(refusalMessage(output, fallback));
  return output;
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

async function dataset(
  binding: z.infer<typeof zPresentResponse>['resolved_bindings'][number],
  callTool: PresentViewProps['callTool'],
) {
  if (!binding.materialized) return undefined;
  const chunks: Uint8Array[] = [];
  let offset = 0n;
  let more = true;
  while (more) {
    const output = await callChecked(
      callTool,
      'read_object',
      {
        source: binding.source,
        object: binding.materialized,
        offset: offset.toString(),
        length: 1048576,
      },
      'The host refused to read the dataset.',
    );
    const part = zGetObjectResponse.parse(output.structuredContent);
    if (part.sha256 !== binding.materialized || BigInt(part.offset) !== offset)
      throw new Error('Dataset identity or range changed');
    const bytes = Uint8Array.from(atob(part.data_base64), (character) => character.charCodeAt(0));
    offset += BigInt(bytes.byteLength);
    if (offset > 4194304n) throw new Error('Dataset exceeds the inline display budget');
    if (bytes.length === 0 && part.has_more) throw new Error('Dataset read made no progress');
    chunks.push(bytes);
    more = part.has_more;
  }
  const merged = new Uint8Array(Number(offset));
  let cursor = 0;
  for (const chunk of chunks) {
    merged.set(chunk, cursor);
    cursor += chunk.byteLength;
  }
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', merged));
  const hex = Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  if (hex !== binding.materialized) throw new Error('Dataset digest verification failed');
  return new TextDecoder('utf-8', { fatal: true }).decode(merged);
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
function chartsFromView(
  view: z.infer<typeof zPresentResponse>['view'],
  statuses: z.infer<typeof zPresentResponse>['charts'],
): { charts: Map<string, TopLevelSpec>; errors: Map<string, string> } {
  const charts = new Map<string, TopLevelSpec>();
  const errors = new Map<string, string>();
  for (const [name, value] of Object.entries(view.charts ?? {})) {
    try {
      charts.set(name, parseVegaLiteSpec(value));
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
    if (status.status !== 'failed') continue;
    const name = chart.kind === 'named' ? chart.name : '';
    errors.set(name, `${failureLabel[status.reason]}: ${status.message}`);
  }
  return { charts, errors };
}

function reason(cause: unknown, fallback: string): string {
  return cause instanceof Error ? cause.message : fallback;
}

export function PresentView({ response, callTool }: PresentViewProps) {
  const [bindings, setBindings] = useState<ResolvedPresentation | null>(null);
  useEffect(() => {
    let active = true;
    const load = async () => {
      const { charts, errors: chartErrors } = chartsFromView(response.view, response.charts);
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
          const output = await callChecked(
            callTool,
            'show',
            {
              workspace_id: binding.source.workspace_id,
              item_id: binding.source.item_id,
              at: { kind: 'revision', revision: binding.source.revision },
              view: 'text',
              selection: binding.source.selection,
              max_bytes: 65536,
              max_images: 0,
            },
            'The host refused to read the source.',
          );
          const source = zReadItemResponse.parse(output.structuredContent);
          if (source.source.revision !== binding.source.revision)
            throw new Error('Returned source revision differs from the binding');
          sources.set(binding.name, source);
        } catch (cause) {
          sourceErrors.set(binding.name, reason(cause, 'The source could not be read'));
        }
        try {
          const text = await dataset(binding, callTool);
          if (text !== undefined) tables.set(binding.name, parseDataset(text, binding.source));
        } catch (cause) {
          datasetErrors.set(binding.name, reason(cause, 'The dataset could not be read'));
        }
      }
      if (active)
        setBindings({
          sources,
          bindings: definitions,
          tables,
          charts,
          datasetErrors,
          sourceErrors,
          chartErrors,
        });
    };
    void load();
    return () => {
      active = false;
    };
  }, [response, callTool]);
  if (!bindings) return <p role="status">Resolving the presentation's exact source references…</p>;
  const view = response.view;
  if (view.grammar === 'json_render') {
    return (
      <BindingsContext.Provider value={bindings}>
        <Layout spec={view.spec} />
      </BindingsContext.Provider>
    );
  }
  return (
    <section>
      <h2>{view.title}</h2>
      <p>{view.description}</p>
      <VegaLiteChart view={view} resolved={bindings} response={response} />
    </section>
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
    chartSpec = parseVegaLiteSpec(view.spec);
  } catch (cause) {
    return <p role="alert">{reason(cause, 'Vega-Lite specification failed runtime validation')}</p>;
  }
  const wanted =
    'data' in chartSpec && chartSpec.data && 'name' in chartSpec.data
      ? chartSpec.data.name
      : undefined;
  const first = wanted
    ? response.resolved_bindings.find((binding) => binding.name === wanted)
    : response.resolved_bindings.at(0);
  if (!first)
    return (
      <p role="alert">
        {wanted
          ? `Unknown binding "${wanted}": the chart names a binding this View does not resolve.`
          : 'A chart requires a retained materialized dataset. No model-provided data was substituted.'}
      </p>
    );
  const rows = resolved.tables.get(first.name);
  if (!rows)
    return (
      <p role="alert">
        {resolved.datasetErrors?.get(first.name) ??
          'A chart requires a retained materialized dataset. No model-provided data was substituted.'}
      </p>
    );
  return (
    <>
      <Chart spec={chartSpec} rows={rows} bindingName={first.name} />
      <p>
        {first.source.path} @ <code>{first.source.revision}</code>
      </p>
    </>
  );
}
