/** Render a validated Vega-Lite spec with an explicit loader boundary and table fallback. */
import { useEffect, useRef, useState } from 'react';
import embed from 'vega-embed';
import { expressionInterpreter } from 'vega-interpreter';
import { loader } from 'vega';
import type { TopLevelSpec } from 'vega-lite';
import { DataTable } from './DataTable';

export interface ChartProps {
  spec: TopLevelSpec;
  rows: ReadonlyArray<Readonly<Record<string, string | number | boolean | null>>>;
  bindingName: string;
}

export function Chart({ spec, rows, bindingName }: ChartProps) {
  const container = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const element = container.current;
    if (element === null) return;
    let disposed = false;
    let finalize: (() => void) | undefined;
    const restricted = loader();
    restricted.load = async () => { throw new Error('Chart network loading is disabled; use resolved datasets'); };
    restricted.sanitize = async () => { throw new Error('Chart external resources are disabled'); };
    embed(element, { ...spec, datasets: { [bindingName]: rows.map(row => ({ ...row })) } }, {
      mode: 'vega-lite', actions: false, ast: true, expr: expressionInterpreter, loader: restricted,
    }).then(result => { if (disposed) result.finalize(); else finalize = result.finalize; })
      .catch(cause => { if (!disposed) setError(cause instanceof Error ? cause.message : 'Chart rendering failed'); });
    return () => { disposed = true; finalize?.(); };
  }, [spec, rows, bindingName]);
  return <section><div ref={container} />{error && <p role="alert">{error}</p>}<details><summary>Source data</summary><DataTable rows={rows} caption={bindingName} /></details></section>;
}
