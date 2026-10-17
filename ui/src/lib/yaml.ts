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
 *   a valid document, and in the page's own words (`thrownWords`): a call stack that is full
 *   (too deep for this browser), the bound on the uses of an anchor, and anything else as "could
 *   not be read". None of the library's words about itself, and none of the JavaScript engine's,
 *   reach the person. What the parser records about the text itself (a missing bracket, a key
 *   that is repeated) is said in its words, at its line;
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
 * uint32.) A number written with a fraction or an exponent is a float. When its digits denote a
 * whole number all the same (`9007199254740993.0`, `9007199254740993e0`,
 * `9.007199254740993e15`), it is held to the same promise: it is refused when a JavaScript
 * number cannot hold that whole number exactly, and read when it can (`1e20`,
 * `9007199254740992.0`). A real fraction (`0.1`, `1.5e-3`) is read as the nearest double, as
 * every YAML reader reads it. Known limit: a double has about 15 to 17 significant digits and a
 * smallest size, so a fraction with more digits than that is rounded to its nearest double
 * (`9007199254740993.5` becomes `9007199254740994`) and one smaller than the smallest double
 * becomes `0` (`1e-400`), in both cases without a word to the person. The whole-number rule
 * above is not affected: it governs only a text whose digits denote a whole number.
 *
 * The bounds are a first line, not the guarantee. This reader is not run on the page's thread:
 * `lib/yaml-reading.ts` runs it in a worker and ends it after a second, because no bound on length
 * or depth has kept its time small for every shape of text. The bounds refuse an ordinary text
 * that is too long or too deep at once, so that it does not wait for that limit. Measured in the
 * built worker, in Chromium and in WebKit, while other builds ran (one run each):
 * - a naming rules file of 70 rules, 16,384 characters: 30 ms and 51 ms;
 * - 4,000 two-letter keys in one block mapping: 0.2 s and 0.26 s;
 * - 8,190 one-letter keys in one flow mapping (`{一,丁,…}`): 0.53 to 0.65 s in both, the slowest
 *   text found that is read to the end (the parser compares each key with every other);
 * - `[a: a: a: …]` on one line, which the depth scan passes: at 2,042 characters 0.17 s and
 *   0.36 s; at 4,094, WebKit 1.4 s; at 8,192, WebKit 5.9 s (7.3 s on the page's thread before
 *   this, and 21 s at 16,384). Chromium gives up on that text from about 2,850 characters, in
 *   20 ms: its call stack is full, which is caught and shown as a problem;
 * - eight lines of `- [a: a: …]`: at 15,399 characters 1.1 s in Chromium; at 14,439 characters
 *   2 s in WebKit.
 * So a text inside every bound can take longer than the limit, and is then refused as too complex.
 * Rules nest 3 deep and properties a few levels. The depth is an upper estimate for text in block
 * style and in JSON style (a line counts its own `-` and `:`, and JSON-style text is counted one
 * level deeper than it is), so a text of fewer real levels than `MAX_YAML_DEPTH` can be refused:
 * 31 brackets in a top-level list are read, 32 are not. It is not an upper estimate for a colon
 * inside brackets, which the scan does not count. The callers start reading a text a short pause
 * after the last change (`YAML_PAUSE_MS`), not at every keystroke (`usePausedReading`).
 *
 * The schema is YAML 1.2 core, where `yes`, `no`, `on` and `off` are strings. A plain scalar that
 * a YAML 1.1 reader would take for something else is read as the string it is and reported as a
 * warning, so the person can quote it.
 *
 * Places. A reader may be asked where some values stand in the text, each named by a JSON Pointer
 * (`/rules/0/glob`): the service names a problem of a stored file so, and the page shows it at its
 * line and column. Each is found by walking the document the reader built, key by key and index by
 * index; a pointer that goes further than the document does stands at the last value it reached,
 * and one that reaches nothing is not placed. Only a text that was read can be placed.
 *
 * Writing: keys stay in the order the value has them (unknown keys go back in their place), and
 * a string a YAML 1.1 reader would take for a boolean, a number, a date or null is quoted
 * (`compat: 'yaml-1.1'`), so the text means the same to every reader.
 *
 * The service still validates what is sent; this is not a second rule engine.
 */
import {
  CST,
  type Document,
  isAlias,
  isMap,
  isPair,
  isScalar,
  isSeq,
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

/** Where something stands in a YAML text, counted from 1. */
export interface YamlPlace {
  line: number;
  column: number;
}

export type YamlRead =
  | {
      ok: true;
      value: unknown;
      warnings: YamlProblem[];
      /** Where each JSON Pointer asked for stands; present only when some were asked for. */
      places?: Record<string, YamlPlace>;
    }
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
 * brackets a `:` opens none), and the brackets open on it. For text in block style and in JSON
 * style that is an upper estimate of the nesting the parser builds (JSON-style text is counted one
 * level deeper than it is). It does not bound the parser's time: `[a: a: a: …]` nests inside
 * brackets by its colons, which are not counted, and is slow all the same. The time limit of
 * `lib/yaml-reading.ts` is what holds for such a text.
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
  for (const token of new Lexer().lex(text)) {
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
    // A text is one token whatever it holds, and can span lines.
    line += token.split('\n').length - 1;
  }
  return deepest;
}

const refusedWhole = (message: string): YamlRead => ({ ok: false, problems: [whole(message)] });

/**
 * What the reader threw, in the page's words. The library throws for the bound on alias
 * expansion; the engine throws when its call stack is full; whatever else is thrown is not
 * described further.
 */
function thrownWords(thrown: unknown): string {
  const said = thrown instanceof Error ? thrown.message : '';
  if (thrown instanceof RangeError && /call stack/i.test(said)) {
    return 'This text nests deeper than this browser can read. Nothing of it was read: put fewer things inside one another on one line.';
  }
  if (/alias count/i.test(said)) {
    return 'This text uses an anchor (&name, and *name where its value is used again) more often than is read here. Nothing of it was read: write the value out where it is used.';
  }
  return 'This text could not be read as YAML. Nothing of it was read.';
}

/**
 * The whole number that the digits of a float denote (`12.0`, `1.2e1`), counted exactly; null
 * when they denote a real fraction. `source` is a float as the YAML core schema writes one, of a
 * finite value: so the power of ten that is raised here is at most that of the largest double,
 * and a fraction smaller than one is told without raising any.
 */
function wholeNumberOf(source: string): bigint | null {
  const parts = /^([-+]?)(\d*)\.?(\d*)(?:[eE]([-+]?\d+))?$/.exec(source);
  if (parts === null) return null;
  const [, sign = '', before = '', after = '', power = '0'] = parts;
  const digits = `${before}${after}`.replace(/^0+/, '');
  if (digits === '') return 0n;
  const signed = (amount: bigint) => (sign === '-' ? -amount : amount);
  const shift = Number(power) - after.length;
  if (shift >= 0) return signed(BigInt(digits) * 10n ** BigInt(shift));
  // Fewer digits than places after the point: less than one, and not zero.
  if (-shift >= digits.length) return null;
  const unit = 10n ** BigInt(-shift);
  const amount = BigInt(digits);
  return amount % unit === 0n ? signed(amount / unit) : null;
}

/** The keys of a JSON Pointer (RFC 6901); null when it is not one. '' is the whole document. */
export function pointerKeys(pointer: string): string[] | null {
  if (pointer === '') return [];
  if (!pointer.startsWith('/')) return null;
  return pointer
    .slice(1)
    .split('/')
    .map((key) => key.replaceAll('~1', '/').replaceAll('~0', '~'));
}

/**
 * Where the value `pointer` names begins in the text, as an offset: the last value reached when
 * the pointer goes further than the document does; null when it is not a pointer or the document
 * is empty.
 */
function offsetOf(document: Document.Parsed, pointer: string): number | null {
  const keys = pointerKeys(pointer);
  if (keys === null) return null;
  const startOf = (node: unknown): number | null =>
    (isScalar(node) || isMap(node) || isSeq(node)) && node.range ? node.range[0] : null;
  let node: unknown = document.contents;
  let reached = startOf(node);
  for (const key of keys) {
    let next: unknown = null;
    if (isMap(node)) {
      const pair = node.items.find((each) => isScalar(each.key) && String(each.key.value) === key);
      if (pair !== undefined) next = pair.value ?? pair.key;
    } else if (isSeq(node) && /^(0|[1-9]\d*)$/.test(key)) {
      next = node.items.at(Number(key)) ?? null;
    }
    if (next === null) break;
    node = next;
    reached = startOf(node) ?? reached;
  }
  return reached;
}

/** Read `typed` as one YAML document of values JSON can carry; `locate` names places to find. */
export function readYaml(typed: string, locate: readonly string[] = []): YamlRead {
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
        // A float whose digits denote a whole number: refused when the number it became is
        // another one.
        if (
          isScalar(node) &&
          typeof node.value === 'number' &&
          Number.isFinite(node.value) &&
          typeof node.source === 'string'
        ) {
          const written = wholeNumberOf(node.source);
          const became = BigInt(Math.trunc(node.value));
          if (written !== null && written !== became) {
            problems.push(
              at(
                offset,
                `This whole number cannot be sent exactly: it would become ${became}. Put it in quotes to keep it as text.`,
              ),
            );
          }
        }
        return undefined;
      },
    });
    if (problems.length > 0) return { ok: false, problems };
    // Every whole number left is one a number holds exactly.
    const value: unknown = document.toJS({
      reviver: (_key, entry) => (typeof entry === 'bigint' ? Number(entry) : entry),
    });
    const warnings = document.warnings.map((warning) =>
      at(warning.pos[0], firstLine(warning.message)),
    );
    if (locate.length === 0) return { ok: true, value, warnings };
    const places: Record<string, YamlPlace> = {};
    for (const pointer of locate) {
      const offset = offsetOf(document, pointer);
      if (offset === null) continue;
      const { line, col } = lines.linePos(offset);
      places[pointer] = { line, column: col };
    }
    return { ok: true, value, warnings, places };
  } catch (thrown) {
    return refusedWhole(thrownWords(thrown));
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
