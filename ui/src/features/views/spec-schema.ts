/**
 * The json-render spec shape, defined once.
 *
 * `rawSpecSchema` is what an author or agent may hand in: slots as a record or a plain array,
 * children optional. `storedSpecSchema` is the canonical shape a saved View carries, closed per
 * approved component. The published JSON Schema is derived from the stored shape; nothing here is
 * restated by hand anywhere else.
 */
import { z } from 'zod';

export const slotsSchema = z.record(z.string(), z.array(z.string()));
export const repeatSchema = z.strictObject({
  statePath: z.union([z.string(), z.object({ $item: z.string() })]),
  key: z.string().optional(),
});
export const rawElementSchema = z.strictObject({
  type: z.string().min(1),
  props: z.record(z.string(), z.unknown()).default({}),
  children: z.array(z.string()).optional(),
  slots: z.unknown().optional(),
  visible: z.unknown().optional(),
  repeat: z.unknown().optional(),
});
export const rawSpecSchema = z.strictObject({
  root: z.string().min(1),
  elements: z.record(z.string(), rawElementSchema),
  state: z.record(z.string(), z.unknown()).optional(),
});
export type RawElement = z.infer<typeof rawElementSchema>;

/** One approved component: its catalog name and the object schema of its props. */
export interface SpecComponent {
  name: string;
  props: z.ZodObject;
}

/** The stored element of one approved component; unknown props and unknown keys are rejected. */
export function storedElementSchema(component: SpecComponent) {
  return z.strictObject({
    type: z.literal(component.name),
    props: z.strictObject(component.props.shape),
    children: z.array(z.string()),
    slots: slotsSchema.optional(),
    visible: z.unknown().optional(),
    repeat: z.unknown().optional(),
  });
}

/** The stored spec over the approved components. */
export function storedSpecSchema(components: readonly SpecComponent[]) {
  const [first, ...rest] = components.map(storedElementSchema);
  if (first === undefined) throw new Error('A stored spec needs at least one approved component');
  return z.strictObject({
    root: z.string(),
    elements: z.record(z.string(), z.union([first, ...rest])),
    state: z.record(z.string(), z.unknown()).optional(),
  });
}

/** Draft-07 JSON Schema of the stored spec, for validators that do not run Zod. */
export function storedSpecJsonSchema(
  components: readonly SpecComponent[],
): Record<string, unknown> {
  return z.toJSONSchema(storedSpecSchema(components), { target: 'draft-7' });
}
