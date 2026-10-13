/**
 * Stable React keys for server lists that may repeat an entry: the entry's own content, and
 * its occurrence number among equal entries. Nothing is dropped to make keys unique.
 */
export function keyed<T>(values: readonly T[], keyOf: (value: T) => string) {
  const seen = new Map<string, number>();
  return values.map((value) => {
    const base = keyOf(value);
    const occurrence = seen.get(base) ?? 0;
    seen.set(base, occurrence + 1);
    return { key: `${base}#${occurrence}`, value };
  });
}
