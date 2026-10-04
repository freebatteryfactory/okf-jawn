/** Resolve a saved presentation through host tools and preserve exact source bindings. */
import { useEffect, useState } from 'react';
import type { TopLevelSpec } from 'vega-lite';
import { z } from 'zod';
import type { PresentResponse, ViewBinding } from '../../api/generated/types.gen';
import { zGetObjectResponse, zReadItemResponse } from '../../api/generated/zod.gen';
import type { ResolvedPresentation } from './Bindings';
import { BindingsContext } from './Bindings';
import { Chart } from './Chart';
import { catalog } from './catalog';
import { Layout } from './Layout';

export interface PresentViewProps {
  response: PresentResponse;
  callTool: (name: string, input: Record<string, unknown>) => Promise<unknown>;
}
const rowsSchema = z
  .array(z.record(z.string(), z.union([z.string(), z.number(), z.boolean(), z.null()])))
  .max(100000);
const toolResult = z.object({ structuredContent: z.unknown() });

async function dataset(binding: ViewBinding, callTool: PresentViewProps['callTool']) {
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

export function PresentView({ response, callTool }: PresentViewProps) {
  const [bindings, setBindings] = useState<ResolvedPresentation | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    const load = async () => {
      const resolved: ResolvedPresentation = {
        sources: new Map(),
        bindings: new Map(),
        tables: new Map(),
        charts: new Map(),
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
      if (active) setBindings({ ...resolved, sources, bindings: definitions, tables });
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
    const validated = catalog.validate(view.spec);
    if (!validated.success || !validated.data)
      return <p role="alert">The composition does not match the approved catalog.</p>;
    return (
      <BindingsContext.Provider value={bindings}>
        <Layout spec={validated.data} />
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
  // The service must validate the pinned Vega-Lite grammar before returning this tagged response.
  return (
    <section>
      <h2>{view.title}</h2>
      <p>{view.description}</p>
      <Chart spec={view.spec as TopLevelSpec} rows={rows} bindingName={first.name} />
      <p>
        {first.source.path} @ <code>{first.source.revision}</code>
      </p>
    </section>
  );
}
