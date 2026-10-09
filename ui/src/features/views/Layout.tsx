/**
 * Normalize raw presentation data into a canonical Spec, validate it, then render.
 *
 * Order is fixed: materialize slots → construct Spec → catalog.validate → validateSpec →
 * Renderer. Never validate a pre-normalized shape and mutate afterward. Never auto-fix.
 * `visible` is passed through unchanged so validateSpec judges the authored condition.
 * `on` and `watch` are rejected: the catalog has no actions.
 */
import { type Spec, type UIElement, validateSpec } from '@json-render/core';
import { defineRegistry, JSONUIProvider, Renderer } from '@json-render/react';
import { SourceExcerpt } from '../documents/SourceExcerpt';
import { useBindings } from './Bindings';
import { Chart } from './Chart';
import { catalog } from './catalog';
import { DataTable } from './DataTable';
import { type RawElement, rawSpecSchema, repeatSchema, slotsSchema } from './spec-schema';

function refusal(bindings: ReturnType<typeof useBindings>, name: string): string {
  const why = bindings.datasetErrors?.get(name);
  return why ? `: ${why}` : '';
}

/**
 * Why one chart cannot be drawn, or undefined when nothing known stands in its way. The chart's own
 * failure comes first (a specification compile rejects, a status the server reports failed), then
 * the binding it cites: not resolved at all, or a dataset that was refused.
 */
function chartFailure(
  bindings: ReturnType<typeof useBindings>,
  binding: string,
  chart: string,
): string | undefined {
  const own = bindings.chartErrors?.get(chart);
  if (own !== undefined) return own;
  if (!bindings.charts.has(chart)) return `Chart "${chart}" has no specification in this View.`;
  if (!bindings.bindings.has(binding))
    return `Unknown binding "${binding}": the chart cites a binding this View does not resolve.`;
  return bindings.datasetErrors?.get(binding);
}

const { registry } = defineRegistry(catalog, {
  components: {
    Stack: ({ props, children }) => (
      <section className="view-stack">
        {props.title && <h2>{props.title}</h2>}
        {children}
      </section>
    ),
    Columns: ({ children }) => <div className="view-columns">{children}</div>,
    SourceExcerpt: ({ props }) => {
      const bindings = useBindings();
      const source = bindings.sources.get(props.binding);
      const why = bindings.sourceErrors?.get(props.binding);
      return source ? (
        <SourceExcerpt result={source} />
      ) : (
        <p role="alert">
          Source binding unavailable: {props.binding}
          {why ? `: ${why}` : ''}
        </p>
      );
    },
    DataTable: ({ props }) => {
      const bindings = useBindings();
      const rows = bindings.tables.get(props.binding);
      return rows ? (
        <DataTable rows={rows} caption={props.binding} />
      ) : (
        <p role="alert">
          Dataset unavailable: {props.binding}
          {refusal(bindings, props.binding)}
        </p>
      );
    },
    Chart: ({ props }) => {
      const bindings = useBindings();
      const rows = bindings.tables.get(props.binding);
      const spec = bindings.charts.get(props.chart);
      const failure = chartFailure(bindings, props.binding, props.chart);
      return (
        <section>
          <h3>{props.title}</h3>
          {failure === undefined && rows && spec ? (
            <Chart spec={spec} rows={rows} bindingName={props.binding} />
          ) : (
            <p role="alert">{failure ?? 'Resolved chart data or specification unavailable.'}</p>
          )}
        </section>
      );
    },
    SourceList: ({ props }) => {
      const context = useBindings();
      return (
        <ul>
          {props.bindings.map((name) => {
            const entry = context.bindings.get(name);
            return (
              <li key={name}>
                {entry ? `${entry.source.path} @ ${entry.source.revision}` : `Unresolved: ${name}`}
              </li>
            );
          })}
        </ul>
      );
    },
  },
});

/**
 * Convert authored/catalog slot shapes into `Record<string, string[]>`.
 * Accepts already-canonical records or a single default array of child keys.
 */
export function materializeSlots(
  slots: unknown,
  children: string[] | undefined,
): Record<string, string[]> | undefined {
  if (slots === undefined) {
    if (children === undefined) return undefined;
    return { default: [...children] };
  }
  const asRecord = slotsSchema.safeParse(slots);
  if (asRecord.success) return asRecord.data;
  if (Array.isArray(slots) && slots.every((entry) => typeof entry === 'string')) {
    return { default: slots };
  }
  throw new Error('Element slots must be a record of string arrays or a string array');
}

function normalizeElement(key: string, raw: RawElement): UIElement {
  const next: UIElement = {
    type: raw.type,
    props: raw.props,
    // Catalog InferSpec requires children as an array on every element.
    children: raw.children === undefined ? [] : raw.children,
  };
  const slots = materializeSlots(raw.slots, raw.children);
  if (slots !== undefined) next.slots = slots;
  // Pass authored visibility through unchanged so validateSpec judges the original.
  if (raw.visible !== undefined) {
    next.visible = raw.visible as NonNullable<UIElement['visible']>;
  }
  if (raw.repeat !== undefined) {
    const repeat = repeatSchema.safeParse(raw.repeat);
    if (!repeat.success) throw new Error(`Element ${key} has an invalid repeat block`);
    next.repeat =
      repeat.data.key === undefined
        ? { statePath: repeat.data.statePath }
        : { statePath: repeat.data.statePath, key: repeat.data.key };
  }
  return next;
}

/** Build a real Spec from unknown presentation data. Throws on malformed input. */
export function normalizeToSpec(input: unknown): Spec {
  const parsed = rawSpecSchema.safeParse(input);
  if (!parsed.success) throw new Error(`Presentation data is not a Spec: ${parsed.error.message}`);
  const elements: Record<string, UIElement> = {};
  for (const [key, element] of Object.entries(parsed.data.elements)) {
    elements[key] = normalizeElement(key, element);
  }
  const spec: Spec = { root: parsed.data.root, elements };
  if (parsed.data.state !== undefined) spec.state = parsed.data.state;
  return spec;
}

export type SpecPipelineResult = { ok: true; spec: Spec } | { ok: false; error: string };

/**
 * Normalize → catalog.validate → validateSpec. Never mutates after validation.
 */
export function prepareSpec(input: unknown): SpecPipelineResult {
  let spec: Spec;
  try {
    spec = normalizeToSpec(input);
  } catch (cause) {
    return {
      ok: false,
      error: cause instanceof Error ? cause.message : 'Presentation data could not be normalized',
    };
  }
  const catalogResult = catalog.validate(spec);
  if (!catalogResult.success) {
    const detail = catalogResult.error?.message ?? 'catalog rejected the composition';
    return {
      ok: false,
      error: `The composition does not match the approved catalog: ${detail}`,
    };
  }
  const structural = validateSpec(spec);
  if (!structural.valid) {
    const codes = structural.issues.map((issue) => issue.code).join(', ');
    return {
      ok: false,
      error: `The composition failed structural validation: ${codes || 'unknown issue'}`,
    };
  }
  return { ok: true, spec };
}

export interface LayoutProps {
  /** Raw presentation data (for example ViewDocument.spec). */
  spec: unknown;
}
export function Layout({ spec: input }: LayoutProps) {
  const prepared = prepareSpec(input);
  if (!prepared.ok) return <p role="alert">{prepared.error}</p>;
  return (
    <JSONUIProvider registry={registry}>
      <Renderer spec={prepared.spec} registry={registry} />
    </JSONUIProvider>
  );
}
