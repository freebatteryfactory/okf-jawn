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
import { z } from 'zod';
import { SourceExcerpt } from '../documents/SourceExcerpt';
import { useBindings } from './Bindings';
import { Chart } from './Chart';
import { catalog } from './catalog';
import { DataTable } from './DataTable';

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
      const source = useBindings().sources.get(props.binding);
      return source ? (
        <SourceExcerpt result={source} />
      ) : (
        <p role="alert">Source binding unavailable: {props.binding}</p>
      );
    },
    DataTable: ({ props }) => {
      const rows = useBindings().tables.get(props.binding);
      return rows ? (
        <DataTable rows={rows} caption={props.binding} />
      ) : (
        <p role="alert">Dataset unavailable: {props.binding}</p>
      );
    },
    Chart: ({ props }) => {
      const bindings = useBindings();
      const rows = bindings.tables.get(props.binding);
      const spec = bindings.charts.get(props.chart);
      return (
        <section>
          <h3>{props.title}</h3>
          {rows && spec ? (
            <Chart spec={spec} rows={rows} bindingName={props.binding} />
          ) : (
            <p role="alert">Resolved chart data or specification unavailable.</p>
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

const slotsSchema = z.record(z.string(), z.array(z.string()));
const repeatSchema = z.strictObject({
  statePath: z.union([z.string(), z.object({ $item: z.string() })]),
  key: z.string().optional(),
});
const rawElementSchema = z.strictObject({
  type: z.string().min(1),
  props: z.record(z.string(), z.unknown()).default({}),
  children: z.array(z.string()).optional(),
  slots: z.unknown().optional(),
  visible: z.unknown().optional(),
  repeat: z.unknown().optional(),
});
const rawSpecSchema = z.strictObject({
  root: z.string().min(1),
  elements: z.record(z.string(), rawElementSchema),
  state: z.record(z.string(), z.unknown()).optional(),
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

function normalizeElement(key: string, raw: z.infer<typeof rawElementSchema>): UIElement {
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
