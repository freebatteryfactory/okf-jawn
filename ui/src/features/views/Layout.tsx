/** Render a catalog-validated composition using the same source components as the Explorer. */
import { defineRegistry, Renderer } from '@json-render/react';
import { Chart } from './Chart';
import { catalog } from './catalog';
import { useBindings } from './Bindings';
import { SourceExcerpt } from '../documents/SourceExcerpt';
import { DataTable } from './DataTable';

const { registry } = defineRegistry(catalog, {
  components: {
    Stack: ({ props, children }) => <section className="view-stack">{props.title && <h2>{props.title}</h2>}{children}</section>,
    Columns: ({ children }) => <div className="view-columns">{children}</div>,
    SourceExcerpt: ({ props }) => {
      const source = useBindings().sources.get(props.binding);
      return source ? <SourceExcerpt result={source} /> : <p role="alert">Source binding unavailable: {props.binding}</p>;
    },
    DataTable: ({ props }) => {
      const rows = useBindings().tables.get(props.binding);
      return rows ? <DataTable rows={rows} caption={props.binding} /> : <p role="alert">Dataset unavailable: {props.binding}</p>;
    },
    Chart: ({ props }) => {
      const bindings = useBindings();
      const rows = bindings.tables.get(props.binding);
      const spec = bindings.charts.get(props.binding);
      return <section><h3>{props.title}</h3>{rows && spec ? <Chart spec={spec} rows={rows} bindingName={props.binding} /> : <p role="alert">Resolved chart data or specification unavailable.</p>}</section>;
    },
    SourceList: ({ props }) => {
      const context = useBindings();
      return <ul>{props.bindings.map(name => { const entry = context.bindings.get(name); return <li key={name}>{entry ? `${entry.source.path} @ ${entry.source.revision}` : `Unresolved: ${name}`}</li>; })}</ul>;
    },
  },
});

export interface LayoutProps { spec: Parameters<typeof Renderer>[0]['spec'] }
export function Layout({ spec }: LayoutProps) { return <Renderer spec={spec} registry={registry} />; }
