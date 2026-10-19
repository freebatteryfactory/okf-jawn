/**
 * A binding's declared `transforms` (Vega-Lite `Transform` objects, core ruling E3) run before a
 * chart's own: in every view of a chart that reads the binding by `data: { name }`, they are put
 * before that view's own `transform`. A view that inherits its data from its parent (a layer, a
 * facet's inner spec) gets them through the parent, as Vega-Lite applies a parent's transforms to
 * the data its children inherit.
 *
 * A transform may look up another binding (`lookup` with `from: { data: { name } }`), and what it
 * finds is that binding after its own transforms, as a chart that reads it directly would see it.
 * Vega-Lite reads a lookup's data as it is given, with no transforms of its own, so a binding with
 * transforms that is looked up is made apart (`DerivedDataset`) and the lookup reads that instead.
 * The bindings it looks up in turn are made first, so the list is in the order to make them, as
 * core closes what a chart reads over its bindings' lookups (`views/validate.rs`). Bindings whose
 * transforms look each other up, a binding's own included, have no order to run in: that chart is
 * refused in words (core accepts such a View today).
 *
 * The specification given is never changed: what is changed is copied. The result is validated
 * as any chart is (`parseVegaLiteSpec`), so a transform Vega-Lite refuses is that chart's alert.
 */

/** A binding's data after its own transforms, for the lookups of a chart that read it. */
export interface DerivedDataset {
  /** The dataset name the chart's lookups read it by; no binding has it. */
  name: string;
  /** The binding it is made from. */
  binding: string;
  /** The binding's transforms, each lookup in them reading what it looks up, made before it. */
  transform: readonly unknown[];
}

/** A chart's specification with its bindings' transforms placed, and what it looks up, made. */
export type ChartPlan =
  | { ok: true; spec: unknown; derived: readonly DerivedDataset[] }
  | { ok: false; message: string };

/** The keys under which a Vega-Lite view holds the views it composes. */
const composed = ['layer', 'concat', 'hconcat', 'vconcat'] as const;

type Spec = Record<string, unknown>;

function isSpec(value: unknown): value is Spec {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The binding a view reads by name, if it declares its data that way. */
function readsBinding(view: Spec): string | undefined {
  const data = view.data;
  return isSpec(data) && typeof data.name === 'string' ? data.name : undefined;
}

/** `transform` with its lookup of a named dataset reading `target(name)` instead; else as given. */
function pointLookup(transform: unknown, target: (name: string) => string): unknown {
  if (!isSpec(transform) || !('lookup' in transform) || !isSpec(transform.from)) return transform;
  const data = transform.from.data;
  if (!isSpec(data) || typeof data.name !== 'string') return transform;
  return { ...transform, from: { ...transform.from, data: { ...data, name: target(data.name) } } };
}

/**
 * `spec` with each binding's transforms before the own transforms of every view that reads it,
 * every lookup in the result reading `target(binding)(name)`: `binding` is the binding whose
 * declared transform it is, or undefined for a view's own transform.
 */
function placed(
  spec: unknown,
  transforms: ReadonlyMap<string, readonly unknown[]>,
  target: (binding: string | undefined) => (name: string) => string,
): unknown {
  if (!isSpec(spec)) return spec;
  const next: Spec = { ...spec };
  const binding = readsBinding(spec);
  const declared = (binding === undefined ? undefined : transforms.get(binding)) ?? [];
  const own = Array.isArray(spec.transform) ? spec.transform : [];
  if (declared.length > 0 || own.length > 0)
    next.transform = [
      ...declared.map((step) => pointLookup(step, target(binding))),
      ...own.map((step) => pointLookup(step, target(undefined))),
    ];
  for (const key of composed) {
    const views = spec[key];
    if (Array.isArray(views)) next[key] = views.map((view) => placed(view, transforms, target));
  }
  if (isSpec(spec.spec)) next.spec = placed(spec.spec, transforms, target);
  return next;
}

/**
 * `spec` with its bindings' transforms placed (`placed`), each lookup of a binding
 * that has transforms reading that binding made apart, and the list of what to make, in order.
 */
export function planChart(
  spec: unknown,
  transforms: ReadonlyMap<string, readonly unknown[]>,
): ChartPlan {
  const derived: DerivedDataset[] = [];
  const made = new Map<string, string>();
  // The bindings being made, outermost first: one met again is a cycle.
  const making: string[] = [];
  const taken = new Set(transforms.keys());
  let cycle: readonly string[] | undefined;
  const lookedUp = (binding: string): string => {
    const known = made.get(binding);
    if (known !== undefined) return known;
    const declared = transforms.get(binding) ?? [];
    if (declared.length === 0) return binding;
    const open = making.indexOf(binding);
    if (open >= 0) {
      cycle ??= [...making.slice(open), binding];
      return binding;
    }
    const transform = declared.map((step) => pointLookup(step, within(binding)));
    let name = `${binding} after its transforms`;
    while (taken.has(name)) name = `${name}'`;
    taken.add(name);
    made.set(binding, name);
    derived.push({ name, binding, transform });
    return name;
  };
  // What a lookup in `binding`'s transforms reads, made while `binding` is being made.
  const within =
    (binding: string | undefined) =>
    (name: string): string => {
      if (binding === undefined) return lookedUp(name);
      making.push(binding);
      const found = lookedUp(name);
      making.pop();
      return found;
    };
  const next = placed(spec, transforms, within);
  if (cycle !== undefined)
    return {
      ok: false,
      message: `the transforms of its bindings look each other up (${cycle.join(' → ')}), so none of them can run first`,
    };
  return { ok: true, spec: next, derived };
}
