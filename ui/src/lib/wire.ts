/**
 * Build a generated SDK/wire request object without copying `undefined` optionals.
 *
 * Parsed Zod values may contain `undefined` under exactOptionalPropertyTypes.
 * Generated `types.gen` optionals are typically `T | null` without `undefined`.
 * Call sites must construct the wire object field-by-field through this helper.
 */
export function omitUndefined(input: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(input)) {
    if (value !== undefined) out[key] = value;
  }
  return out;
}
