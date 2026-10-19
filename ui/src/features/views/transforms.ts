/**
 * A binding's declared `transforms` (Vega-Lite `Transform` objects, core ruling E3) run before a
 * chart's own: in every view of a chart that reads the binding by `data: { name }`, they are put
 * before that view's own `transform`. A view that inherits its data from its parent (a layer, a
 * facet's inner spec) gets them through the parent, as Vega-Lite applies a parent's transforms to
 * the data its children inherit.
 *
 * The specification given is never changed: what is changed is copied. The result is validated
 * as any chart is (`parseVegaLiteSpec`), so a transform Vega-Lite refuses is that chart's alert.
 */

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

/** `spec` with each binding's transforms before the own transforms of every view that reads it. */
export function withBindingTransforms(
  spec: unknown,
  transforms: ReadonlyMap<string, readonly unknown[]>,
): unknown {
  if (!isSpec(spec)) return spec;
  const next: Spec = { ...spec };
  const binding = readsBinding(spec);
  const declared = binding === undefined ? undefined : transforms.get(binding);
  if (declared !== undefined && declared.length > 0) {
    const own = Array.isArray(spec.transform) ? spec.transform : [];
    next.transform = [...declared, ...own];
  }
  for (const key of composed) {
    const views = spec[key];
    if (Array.isArray(views))
      next[key] = views.map((view) => withBindingTransforms(view, transforms));
  }
  if (isSpec(spec.spec)) next.spec = withBindingTransforms(spec.spec, transforms);
  return next;
}
