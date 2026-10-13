/**
 * The words a table has for a value off the wire; undefined for a value the table does not know.
 *
 * A table keyed by a generated union has words for every value this build's types name, and the
 * compiler holds it to that. The service may be newer than this build and answer a value those
 * types do not name: a look-up by index would then give nothing, and the place that draws it would
 * show a blank, or the word "undefined". A caller of this function has to say what an unknown
 * value reads as.
 */
export function wordsFor(
  table: Readonly<Record<string, string>>,
  value: string,
): string | undefined {
  return Object.hasOwn(table, value) ? table[value] : undefined;
}
