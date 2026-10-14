/**
 * The field a refusal names, in plain words.
 *
 * The service names the field of an `invalid_input` as a path into the request it refused
 * (`/path`, `/rules/0/glob`). A form that draws that field shows the refusal there. For a field no
 * form draws, the notice says which part of the request it is: each step of the path in words,
 * the innermost first ("“glob” of rule 1"). A step this table does not know is shown as the
 * service wrote it, in quotation marks, so no field is ever left unnamed.
 */

/** The parts of this package's requests, as a person would call them. */
const NAMES: ReadonlyMap<string, string> = new Map([
  ['path', 'the path'],
  ['folder', 'the folder'],
  ['destination', 'the new place'],
  ['title', 'the title'],
  ['type_name', 'the type'],
  ['kind', 'the kind of item'],
  ['body', 'the text'],
  ['properties', 'the properties'],
  ['item_id', 'the item'],
  ['item_ids', 'the items'],
  ['status', 'the status word'],
  ['archived', 'the archived mark'],
  ['rules', 'the rules'],
  ['plan', 'the preview'],
  ['entries', 'the renames'],
  ['from', 'the name now'],
  ['to', 'the name after'],
  ['base_revision', 'the version the change is made on'],
  ['workspace_id', 'the workspace'],
]);

/** What one entry of a list is called, by the list's name. */
const ENTRIES: ReadonlyMap<string, string> = new Map([
  ['rules', 'rule'],
  ['entries', 'rename'],
  ['item_ids', 'item'],
]);

/** `field` (`/rules/0/glob`) in words: "“glob” of rule 1". */
export function fieldInWords(field: string): string {
  const steps = field.split('/').filter((step) => step.length > 0);
  const parts: string[] = [];
  let list: string | null = null;
  for (const step of steps) {
    if (/^\d+$/.test(step) && list !== null) {
      // A place in a list: its entry, counted from one, in place of the list's own name.
      const entry = ENTRIES.get(list);
      const outer = parts.pop() ?? '';
      parts.push(
        entry === undefined
          ? `entry ${Number(step) + 1} of ${outer}`
          : `${entry} ${Number(step) + 1}`,
      );
      list = null;
      continue;
    }
    parts.push(NAMES.get(step) ?? `“${step}”`);
    list = step;
  }
  return parts.length === 0 ? 'the request' : parts.reverse().join(' of ');
}
