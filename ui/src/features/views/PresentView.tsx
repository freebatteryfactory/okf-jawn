/** Resolve a saved presentation through host tools and preserve exact source bindings. */
import { useEffect, useState } from 'react';
import { compile, type TopLevelSpec } from 'vega-lite';
import { z } from 'zod';
import {
  zDataset,
  zGetObjectResponse,
  type zPresentResponse,
  zReadItemResponse,
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
/**
 * Read the bytes of a retained dataset. A dataset is read only from the converter's text (SPEC
 * R5, decision O2): any other origin, or any shape the generated `zDataset` refuses, is that
 * chart's own alert with the reason, never a drawn chart.
 */
export function parseDataset(text: string): ReadonlyArray<Row> {
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
  return parseDataset(new TextDecoder('utf-8', { fatal: true }).decode(merged));
}

function chartsFromView(view: z.infer<typeof zPresentResponse>['view']): {
  charts: Map<string, TopLevelSpec>;
  chartError: string | null;
} {
  const charts = new Map<string, TopLevelSpec>();
  for (const [name, value] of Object.entries(view.charts ?? {})) {
    try {
      charts.set(name, parseVegaLiteSpec(value));
    } catch (cause) {
      return {
        charts: new Map(),
        chartError:
          cause instanceof Error
            ? `Chart "${name}" failed validation: ${cause.message}`
            : `Chart "${name}" failed validation`,
      };
    }
  }
  return { charts, chartError: null };
}

export function PresentView({ response, callTool }: PresentViewProps) {
  const [bindings, setBindings] = useState<ResolvedPresentation | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    const load = async () => {
      const { charts, chartError } = chartsFromView(response.view);
      if (chartError) throw new Error(chartError);
      const resolved: ResolvedPresentation = {
        sources: new Map(),
        bindings: new Map(),
        tables: new Map(),
        charts,
      };
      const sources = new Map(resolved.sources);
      const definitions = new Map(resolved.bindings);
      const tables = new Map(resolved.tables);
      for (const binding of response.resolved_bindings) {
        definitions.set(binding.name, binding);
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
        const rows = await dataset(binding, callTool);
        if (rows) tables.set(binding.name, rows);
      }
      if (active) setBindings({ ...resolved, sources, bindings: definitions, tables, charts });
    };
    load().catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : 'Source resolution failed');
    });
    return () => {
      active = false;
    };
  }, [response, callTool]);
  if (error) return <p role="alert">{error}</p>;
  if (!bindings) return <p role="status">Resolving the presentation's exact source references…</p>;
  const view = response.view;
  if (view.grammar === 'json_render') {
    return (
      <BindingsContext.Provider value={bindings}>
        <Layout spec={view.spec} />
      </BindingsContext.Provider>
    );
  }
  const first = response.resolved_bindings.at(0);
  const rows = first ? bindings.tables.get(first.name) : undefined;
  if (!first || !rows) {
    const failed = response.charts.find((result) => result.status.status === 'failed')?.status;
    return (
      <p role="alert">
        {failed && failed.status === 'failed'
          ? failed.message
          : 'A chart requires a retained materialized dataset. No model-provided data was substituted.'}
      </p>
    );
  }
  let chartSpec: TopLevelSpec;
  try {
    chartSpec = parseVegaLiteSpec(view.spec);
  } catch (cause) {
    return (
      <p role="alert">
        {cause instanceof Error
          ? cause.message
          : 'Vega-Lite specification failed runtime validation'}
      </p>
    );
  }
  return (
    <section>
      <h2>{view.title}</h2>
      <p>{view.description}</p>
      <Chart spec={chartSpec} rows={rows} bindingName={first.name} />
      <p>
        {first.source.path} @ <code>{first.source.revision}</code>
      </p>
    </section>
  );
}
