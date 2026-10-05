/**
 * Build a generated SDK/wire request object without copying `undefined` optionals.
 *
 * Apply once at the Apps bridge / HTTP wire boundary (mcp-apps/main.tsx). Feature
 * components must pass fields through unchanged so a second pass cannot hide bugs.
 *
 * Parsed Zod values may contain `undefined` under exactOptionalPropertyTypes.
 * Generated `types.gen` optionals are typically `T | null` without `undefined`.
 */
export function omitUndefined(input: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(input)) {
    if (value !== undefined) out[key] = value;
  }
  return out;
}
