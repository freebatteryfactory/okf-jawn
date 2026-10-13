/**
 * YAML for the two advanced editors (the naming rules, an item's raw properties): reading what a
 * person typed into the JSON the wire carries, and writing that JSON as YAML.
 *
 * What is typed is untrusted text. Reading it:
 * - a text longer than `MAX_YAML_CHARS` is refused before it is parsed;
 * - whatever the parser throws is caught and shown as one more problem, never read as an empty or
 *   a valid document;
 * - every error the parser records is a problem, at its line and column; nothing is read from a
 *   document that has one;
 * - a `%YAML` or `%TAG` directive is refused, not followed (a `%YAML 1.1` line would turn on merge
 *   keys and the 1.1 booleans);
 * - the library's defaults that refuse early are kept: a repeated key is an error, no `<<` merge
 *   keys, a bound on alias expansion;
 * - a value the wire cannot carry is refused in plain words, not coerced: an explicit tag other
 *   than the core ones (a date, binary, a tag of one's own), a key that is not a string, a
 *   number that is not finite.
 * The schema is YAML 1.2 core, where `yes`, `no`, `on` and `off` are strings. A plain scalar that
 * a YAML 1.1 reader would take for something else is read as the string it is and reported as a
 * warning, so the person can quote it.
 *
 * Writing: keys stay in the order the value has them (unknown keys go back in their place), and
 * a string a YAML 1.1 reader would take for a boolean, a number, a date or null is quoted
 * (`compat: 'yaml-1.1'`), so the text means the same to every reader.
 *
 * The service still validates what is sent; this is not a second rule engine.
 */
import {
  isAlias,
  isMap,
  isPair,
  isScalar,
  LineCounter,
  parseDocument,
  stringify,
  visit,
} from 'yaml';

/** The longest text that is parsed: one mebibyte of characters, far above any rules or properties. */
export const MAX_YAML_CHARS = 1_048_576;

/** One thing wrong with a YAML text, or one warning about it. */
export interface YamlProblem {
  message: string;
  /** Where it is, counted from 1; null when it is about the whole text. */
  line: number | null;
  column: number | null;
}

export type YamlRead =
  | { ok: true; value: unknown; warnings: YamlProblem[] }
  | { ok: false; problems: YamlProblem[] };

const whole = (message: string): YamlProblem => ({ message, line: null, column: null });

/** The tags of the YAML 1.2 core schema: what JSON can carry. */
const CORE_TAGS: ReadonlySet<string> = new Set(
  ['str', 'int', 'float', 'bool', 'null', 'map', 'seq'].map((name) => `tag:yaml.org,2002:${name}`),
);

/** The library's errors whose own words speak of the library, said plainly instead. */
const OWN_WORDS: Partial<Record<string, string>> = {
  MULTIPLE_DOCS: 'Only one document is read here. Remove this “---” line and what follows it.',
  RESOURCE_EXHAUSTION: 'The text could not be read past this point.',
};

/** A problem's words in the editor's: the library's message without its own position and excerpt. */
function firstLine(message: string): string {
  return (message.split('\n').at(0) ?? message).replace(/ at line \d+, column \d+:?$/, '');
}

/** Read `text` as one YAML document of values JSON can carry. */
export function readYaml(text: string): YamlRead {
  if (text.length > MAX_YAML_CHARS) {
    return {
      ok: false,
      problems: [
        whole(
          `This text is ${text.length} characters long; at most ${MAX_YAML_CHARS} are read. Nothing was read.`,
        ),
      ],
    };
  }
  try {
    const lines = new LineCounter();
    const document = parseDocument(text, { compat: 'yaml-1.1', lineCounter: lines });
    const at = (offset: number, message: string): YamlProblem => {
      const { line, col } = lines.linePos(offset);
      return { message, line, column: col };
    };
    const problems = document.errors.map((error) =>
      at(error.pos[0], OWN_WORDS[error.code] ?? firstLine(error.message)),
    );
    if (text.split(/\r?\n/).some((line) => /^%[A-Za-z]/.test(line))) {
      problems.push(whole('A directive line (starting with %) is not accepted here. Remove it.'));
    }
    visit(document, {
      Pair(_key, pair) {
        const key = pair.key;
        if (isScalar(key) && typeof key.value === 'string') return undefined;
        const offset = isScalar(key) || isMap(key) ? (key.range?.[0] ?? 0) : 0;
        problems.push(at(offset, 'A key must be text. Put it in quotes.'));
        return undefined;
      },
      Node(_key, node) {
        if (isAlias(node) || isPair(node)) return undefined;
        const offset = node.range?.[0] ?? 0;
        if (typeof node.tag === 'string' && !CORE_TAGS.has(node.tag)) {
          problems.push(
            at(offset, `The tag ${node.tag} is not accepted here: write the value as plain text.`),
          );
        }
        if (isScalar(node) && typeof node.value === 'number' && !Number.isFinite(node.value)) {
          problems.push(at(offset, 'A number must be finite: .inf and .nan cannot be sent.'));
        }
        if (isScalar(node) && typeof node.value === 'bigint') {
          problems.push(at(offset, 'This number is too large to be sent exactly.'));
        }
        return undefined;
      },
    });
    if (problems.length > 0) return { ok: false, problems };
    const value: unknown = document.toJS();
    return {
      ok: true,
      value,
      warnings: document.warnings.map((warning) => at(warning.pos[0], firstLine(warning.message))),
    };
  } catch (thrown) {
    const said = thrown instanceof Error ? thrown.message : String(thrown);
    return { ok: false, problems: [whole(`This text could not be read as YAML: ${said}`)] };
  }
}

/** Write a JSON value as YAML, its keys in the order it has them. */
export function writeYaml(value: unknown): string {
  return stringify(value, { compat: 'yaml-1.1' });
}

/** A problem as one line of words, with its place when it has one. */
export function describeProblem(problem: YamlProblem): string {
  return problem.line === null
    ? problem.message
    : `Line ${problem.line}, column ${problem.column ?? 1}: ${problem.message}`;
}
