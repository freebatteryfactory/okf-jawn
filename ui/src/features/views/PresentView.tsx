/** Resolve a saved presentation through host tools and preserve exact source bindings. */
import { useEffect, useState } from 'react';
import { compile, type TopLevelSpec } from 'vega-lite';
import { z } from 'zod';
import {
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
const rowsSchema = z
  .array(z.record(z.string(), z.union([z.string(), z.number(), z.boolean(), z.null()])))
  .max(100000);
const toolResult = z.object({ structuredContent: z.unknown() });

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
    const output = toolResult.parse(
      await callTool('read_object', {
        source: binding.source,
        object: binding.materialized,
        offset: offset.toString(),
        length: 1048576,
      }),
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
  return rowsSchema.parse(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(merged)));
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
        const output = toolResult.parse(
          await callTool('show', {
            workspace_id: binding.source.workspace_id,
            item_id: binding.source.item_id,
            at: { kind: 'revision', revision: binding.source.revision },
            view: 'text',
            selection: binding.source.selection,
            max_bytes: 65536,
            max_images: 0,
          }),
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
  if (!first || !rows)
    return (
      <p role="alert">
        A chart requires a retained materialized dataset. No model-provided data was substituted.
      </p>
    );
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
