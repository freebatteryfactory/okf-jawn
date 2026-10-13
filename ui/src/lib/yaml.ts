/**
 * YAML for the two advanced editors (the naming rules, an item's raw properties): reading what a
 * person typed into the JSON the wire carries, and writing that JSON as YAML.
 *
 * What is typed is untrusted text. Reading it:
 * - a byte order mark at the very start is taken away before anything else (a text saved as
 *   "UTF-8 with BOM" and pasted carries one); positions are counted without it;
 * - a text longer than `MAX_YAML_CHARS` is refused before it is parsed, and said to be too long:
 *   it is never cut;
 * - a text that nests deeper than `MAX_YAML_DEPTH` is refused before it is parsed, by one pass
 *   over the library's lexer (`deepestNesting`): the parser's time grows much faster than the
 *   text when lists stand in lists, brackets in brackets, or `a: a: a:` on one line;
 * - whatever the parser throws is caught and shown as one more problem, never read as an empty or
 *   a valid document;
 * - every error the parser records is a problem, at its line and column; nothing is read from a
 *   document that has one;
 * - a directive is refused, not followed (a `%YAML 1.1` line would turn on merge keys and the 1.1
 *   booleans). Whether the document carries one is what the parser reports for the document it
 *   read (`Document.directives`, and its warning for a directive it does not know), not a look
 *   at the text: a line that starts with `%` inside a quoted or a block scalar is text;
 * - the library's defaults that refuse early are kept: a repeated key is an error, no `<<` merge
 *   keys, a bound on alias expansion;
 * - a value the wire cannot carry is refused in plain words, not coerced: an explicit tag other
 *   than the core ones (a date, binary, a tag of one's own), a key that is not a string, a
 *   number that is not finite, and a whole number a JavaScript number cannot hold exactly.
 *
 * Whole numbers. The wire is JSON (`properties: z.record(z.string(), z.unknown())` in the generated
 * `zod.gen.ts`; `serde_json::Value` in `crates/contract/src/item.rs`), and the browser writes a
 * number with `JSON.stringify`, which holds a whole number exactly only from -(2^53 - 1) to
 * 2^53 - 1 (`Number.MAX_SAFE_INTEGER`). So integers are read as `bigint` (`intAsBigInt`), which
 * shows the digits that were typed: one inside that range becomes a number, one outside it is
 * refused, never rounded. (The one whole number of the naming rules, `schema_version`, is a
 * uint32.) A number written with a fraction or an exponent is a float, and is read as the
 * nearest double, as every YAML reader reads it.
 *
 * The bounds, and the measurements they were chosen from (this reader, on the build machine while
 * other builds ran, best of three). Before the bounds: 4,096 unclosed brackets took 3.5 s, 2,048
 * nested `- ` 0.7 s, `a: a: a: …` on one line of 16,384 characters 32 s, and a mapping of a
 * megabyte minutes. With them, at 16,384 characters: a naming rules file of 70 rules reads in
 * about 25 ms, and the slowest text found that passes the depth check (3,500 two-letter keys in
 * one mapping, where the parser compares each key with every other) in about 0.2 s; every text
 * that is too deep is refused in about 10 ms. Rules nest 3 deep and properties a few levels. The
 * depth is an upper estimate (a line counts its own `-` and `:`), so a text of fewer real levels
 * than `MAX_YAML_DEPTH` can be refused: 31 brackets in a top-level list are read, 32 are not. The
 * callers read
 * a text a short pause after the last change (`YAML_PAUSE_MS`), not at every keystroke
 * (`usePausedReading`), so the 0.2 s is spent once, after the typing stops.
 *
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
  CST,
  isAlias,
  isMap,
  isPair,
  isScalar,
  Lexer,
  LineCounter,
  parseDocument,
  stringify,
  visit,
} from 'yaml';

/** The longest text that is parsed, in characters (UTF-16 units, as the editor counts them). */
export const MAX_YAML_CHARS = 16_384;

/** The deepest nesting that is parsed. */
export const MAX_YAML_DEPTH = 32;

/** How long after the last change a text is read, in milliseconds. */
export const YAML_PAUSE_MS = 300;

const BYTE_ORDER_MARK = 0xfeff;
const CORE_TAG_PREFIX = 'tag:yaml.org,2002:';
const DIRECTIVE_WORDS = 'A directive line (starting with %) is not accepted here. Remove it.';

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
  ['str', 'int', 'float', 'bool', 'null', 'map', 'seq'].map((name) => `${CORE_TAG_PREFIX}${name}`),
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

/**
 * How deep `text` nests at most, and the first line that is that deep (counted from 1). It is read
 * off the library's lexer, which only cuts the text into tokens (one pass, no tree), so what is
 * counted is what the parser will take for structure: a bracket or a colon inside a quoted text,
 * a block of text or a comment is not counted. A line's depth is the indentation steps it stands
 * in, the `-`, `?` and `:` indicators on it (outside brackets, where each can open a level; inside
 * brackets a `:` opens none), and the brackets open on it. It is meant as an upper estimate of the
 * nesting the parser builds: every way of nesting that was measured to be slow is counted.
 */
export function deepestNesting(text: string): { depth: number; line: number } {
  let deepest = { depth: 0, line: 1 };
  let line = 1;
  /** The indentation of each block the current line stands in. */
  const indents: number[] = [];
  /** The indentation of the current line; null once its first token was seen. */
  let indent: number | null = 0;
  let indicators = 0;
  let open = 0;
  let scalarNext = false;
  for (const token of new Lexer().lex(text)) {
    if (scalarNext) {
      // A scalar is text, whatever it holds; a block of text spans lines.
      scalarNext = false;
      line += token.split('\n').length - 1;
      continue;
    }
    if (token === CST.SCALAR) scalarNext = true;
    if (token === '\n' || token === '\r\n') {
      line += 1;
      indent = 0;
      indicators = 0;
      continue;
    }
    if (indent !== null && token.startsWith(' ')) {
      indent = token.length;
      continue;
    }
    if (token.startsWith('#') || token === CST.DOCUMENT || token === CST.BOM) continue;
    if (indent !== null && open === 0) {
      // The first token of a line outside brackets: where the line stands among the blocks.
      while ((indents.at(-1) ?? -1) >= indent) indents.pop();
      indents.push(indent);
    }
    indent = null;
    if (token === '[' || token === '{') open += 1;
    else if (token === ']' || token === '}') open = Math.max(0, open - 1);
    else if (token === '-' || token === '?' || (token === ':' && open === 0)) indicators += 1;
    const depth = indents.length + indicators + open;
    if (depth > deepest.depth) deepest = { depth, line };
    // A quoted text can span lines.
    line += token.split('\n').length - 1;
  }
  return deepest;
}

const refusedWhole = (message: string): YamlRead => ({ ok: false, problems: [whole(message)] });

/** Read `typed` as one YAML document of values JSON can carry. */
export function readYaml(typed: string): YamlRead {
  // Before anything else: a byte order mark is not part of the text.
  const text = typed.charCodeAt(0) === BYTE_ORDER_MARK ? typed.slice(1) : typed;
  if (text.length > MAX_YAML_CHARS) {
    return refusedWhole(
      `This text is ${text.length} characters long, and at most ${MAX_YAML_CHARS} are read here. Nothing of it was read, and nothing was cut: shorten it.`,
    );
  }
  try {
    const nesting = deepestNesting(text);
    if (nesting.depth > MAX_YAML_DEPTH) {
      return {
        ok: false,
        problems: [
          {
            message: `This text nests more than ${MAX_YAML_DEPTH} levels deep here (lists in lists, brackets in brackets), and deeper is not read. Nothing of it was read.`,
            line: nesting.line,
            column: 1,
          },
        ],
      };
    }
    const lines = new LineCounter();
    const document = parseDocument(text, {
      compat: 'yaml-1.1',
      lineCounter: lines,
      // Whole numbers arrive with the digits that were typed, so one too large is seen.
      intAsBigInt: true,
    });
    const at = (offset: number, message: string): YamlProblem => {
      const { line, col } = lines.linePos(offset);
      return { message, line, column: col };
    };
    const problems = document.errors.map((error) =>
      at(error.pos[0], OWN_WORDS[error.code] ?? firstLine(error.message)),
    );
    // What the parser reports for the document: a `%YAML` line, a `%TAG` line (a tag handle
    // beside the default one, or the default one pointing elsewhere), a directive it does not know.
    const { yaml: version, tags } = document.directives;
    if (
      version.explicit === true ||
      Object.entries(tags).some(([handle, to]) => handle !== '!!' || to !== CORE_TAG_PREFIX) ||
      document.warnings.some((warning) => warning.code === 'BAD_DIRECTIVE')
    ) {
      problems.push(whole(DIRECTIVE_WORDS));
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
        if (
          isScalar(node) &&
          typeof node.value === 'bigint' &&
          (node.value > BigInt(Number.MAX_SAFE_INTEGER) ||
            node.value < -BigInt(Number.MAX_SAFE_INTEGER))
        ) {
          problems.push(
            at(
              offset,
              `This whole number cannot be sent exactly: whole numbers go from -${Number.MAX_SAFE_INTEGER} to ${Number.MAX_SAFE_INTEGER}. Put it in quotes to keep it as text.`,
            ),
          );
        }
        return undefined;
      },
    });
    if (problems.length > 0) return { ok: false, problems };
    // Every whole number left is one a number holds exactly.
    const value: unknown = document.toJS({
      reviver: (_key, entry) => (typeof entry === 'bigint' ? Number(entry) : entry),
    });
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
