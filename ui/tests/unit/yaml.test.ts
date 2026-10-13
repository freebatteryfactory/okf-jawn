/**
 * Reading what a person typed as YAML into the JSON the wire carries, and writing it back: what
 * is refused before anything is sent, and what the text means to every reader.
 */
import { describe, expect, it, vi } from 'vitest';
import {
  deepestNesting,
  describeProblem,
  MAX_YAML_CHARS,
  MAX_YAML_DEPTH,
  readYaml,
  writeYaml,
} from '../../src/lib/yaml';

/** Every text that was given to the parser. */
const parsed = vi.hoisted((): string[] => []);

vi.mock('yaml', async (original) => {
  const actual = await original<typeof import('yaml')>();
  return {
    ...actual,
    parseDocument: (text: string, options?: Parameters<typeof actual.parseDocument>[1]) => {
      parsed.push(text);
      // One text makes the parser throw, as extreme input can; another, something unforeseen.
      if (text === 'the parser throws on this') throw new RangeError('Maximum call stack');
      if (text === 'the parser breaks on this') throw new TypeError('x is not a function');
      return actual.parseDocument(text, options);
    },
  };
});

const value = (text: string) => {
  const read = readYaml(text);
  return read.ok ? read.value : read.problems.map(describeProblem);
};
const problems = (text: string) => {
  const read = readYaml(text);
  return read.ok ? [] : read.problems.map(describeProblem);
};

describe('reading YAML', () => {
  it('reads a mapping as JSON, its keys in the order they were written', () => {
    const read = readYaml('zeta: 1\nalpha: [x, 2, true, null]\nnested:\n  b: text\n  a: 1.5\n');
    expect(read).toEqual({
      ok: true,
      value: { zeta: 1, alpha: ['x', 2, true, null], nested: { b: 'text', a: 1.5 } },
      warnings: [],
    });
    expect(Object.keys(read.ok ? (read.value as object) : {})).toEqual(['zeta', 'alpha', 'nested']);
    expect(value('# a comment\na: 1 # one\n')).toEqual({ a: 1 });
    expect(value('a: &x [1]\nb: *x\n')).toEqual({ a: [1], b: [1] });
    expect(value('')).toBeNull();
  });

  it('reads yes, on and a date as the text they are, and warns that another reader may not', () => {
    const read = readYaml('a: yes\nb: on\nc: 2026-10-12\nd: "yes"\n');
    expect(read.ok && read.value).toEqual({ a: 'yes', b: 'on', c: '2026-10-12', d: 'yes' });
    expect(read.ok ? read.warnings.map(describeProblem) : []).toEqual([
      'Line 1, column 4: Value may be parsed as either !!str or !!bool',
      'Line 2, column 4: Value may be parsed as either !!str or !!bool',
      'Line 3, column 4: Value may be parsed as either !!str or !!timestamp',
    ]);
  });

  it('refuses what cannot be read, at its line', () => {
    expect(problems('a: 1\na: 2\n')).toEqual(['Line 2, column 1: Map keys must be unique']);
    expect(problems('a:\n\tb: 1\n')).toEqual([
      'Line 2, column 1: Tabs are not allowed as indentation',
    ]);
    expect(problems('a: "x\n').at(0)).toMatch(/^Line 2, column 1: Missing closing "quote/);
    expect(problems('a: 1\n---\nb: 2\n')).toEqual([
      'Line 2, column 1: Only one document is read here. Remove this “---” line and what follows it.',
    ]);
    // Nothing is read from a text that has a problem: not its good half either.
    expect(readYaml('a: 1\nb: [\n')).toMatchObject({ ok: false });
  });

  it('refuses a directive instead of following it', () => {
    for (const text of [
      '%YAML 1.1\n---\na: yes\n',
      '%TAG !e! tag:example.com,2000:\n---\na: 1\n',
    ]) {
      expect(problems(text)).toContain(
        'A directive line (starting with %) is not accepted here. Remove it.',
      );
    }
    // A percent sign that starts no directive is text.
    expect(value('a: "%YAML"\nb: 50%\n')).toEqual({ a: '%YAML', b: '50%' });
  });

  it('refuses a value the wire cannot carry, in plain words', () => {
    expect(problems('a: !!timestamp 2026-10-12\n')).toEqual([
      'Line 1, column 16: The tag tag:yaml.org,2002:timestamp is not accepted here: write the value as plain text.',
    ]);
    expect(problems('a: !!binary aGVsbG8=\n').at(0)).toMatch(
      /tag:yaml.org,2002:binary is not accepted/,
    );
    expect(problems('a: !mine x\n').at(0)).toMatch(/The tag !mine is not accepted/);
    expect(problems('a: !!set {x, y}\n').at(0)).toMatch(/2002:set is not accepted/);
    expect(problems('1: a\n')).toEqual(['Line 1, column 1: A key must be text. Put it in quotes.']);
    expect(problems('? [a, b]\n: c\n')).toEqual([
      'Line 1, column 1: A key must be text. Put it in quotes.',
    ]);
    expect(problems('a: .inf\nb: .nan\n')).toEqual([
      'Line 1, column 4: A number must be finite: .inf and .nan cannot be sent.',
      'Line 2, column 4: A number must be finite: .inf and .nan cannot be sent.',
    ]);
    // A core tag says nothing JSON cannot.
    expect(value('a: !!str 12\n"1": b\n')).toEqual({ a: '12', '1': 'b' });
  });

  it('bounds the text before it is parsed: exactly the bound is read, one character more is not', () => {
    const head = 'a: 1\n#';
    const atBound = head + 'x'.repeat(MAX_YAML_CHARS - head.length);
    expect(atBound).toHaveLength(MAX_YAML_CHARS);
    expect(value(atBound)).toEqual({ a: 1 });
    expect(problems(`${atBound}x`)).toEqual([
      `This text is ${MAX_YAML_CHARS + 1} characters long, and at most ${MAX_YAML_CHARS} are read here. Nothing of it was read, and nothing was cut: shorten it.`,
    ]);
  });

  it('shows what the parser throws as a problem, never as an empty or a valid document, and in the page’s own words', () => {
    // A full call stack: said as what it means, not as the engine says it.
    expect(readYaml('the parser throws on this')).toEqual({
      ok: false,
      problems: [
        {
          message:
            'This text nests deeper than this browser can read. Nothing of it was read: put fewer things inside one another on one line.',
          line: null,
          column: null,
        },
      ],
    });
    // Anything else that is thrown: no word of it is shown.
    expect(problems('the parser breaks on this')).toEqual([
      'This text could not be read as YAML. Nothing of it was read.',
    ]);
  });

  it('refuses a hundred uses of one anchor in the page’s own words, and reads ninety-nine', () => {
    const uses = (count: number) =>
      `a: &a x\nl: [${Array.from({ length: count }, () => '*a').join(', ')}]\n`;
    const read = readYaml(uses(99));
    expect(read.ok && (read.value as { l: string[] }).l).toHaveLength(99);
    expect(problems(uses(100))).toEqual([
      'This text uses an anchor (&name, and *name where its value is used again) more often than is read here. Nothing of it was read: write the value out where it is used.',
    ]);
    // None of the library's words about itself.
    expect(problems(uses(100)).join(' ')).not.toMatch(/attack|exhaustion|Excessive/);
  });

  it('bounds alias expansion: a text that grows without end when read is refused', () => {
    const laughs = [
      'a: &a [x, x, x, x, x, x, x, x, x, x]',
      'b: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a, *a]',
      'c: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b, *b]',
      'd: [*c, *c, *c, *c, *c, *c, *c, *c, *c, *c]',
    ].join('\n');
    const read = readYaml(laughs);
    expect(read.ok).toBe(false);
    expect(problems(laughs).join(' ')).toMatch(/uses an anchor .* more often than is read here/);
  });
});

describe('writing YAML', () => {
  it('keeps the order of the keys, and quotes text another reader would take for something else', () => {
    const written = writeYaml({
      zeta: 'yes',
      alpha: '2026-10-12',
      n: '1e3',
      t: '~',
      on: 'on',
      p: 'plain',
      i: 1,
    });
    expect(written).toBe(
      'zeta: "yes"\nalpha: "2026-10-12"\n"n": "1e3"\nt: "~"\n"on": "on"\np: plain\ni: 1\n',
    );
  });

  it('writes what it reads back to the same value, with no warning', () => {
    const rules = {
      schema_version: 1,
      rules: [{ glob: '**/*.md', prefix: '', case: 'kebab', flag: true, destination: null }],
      'x-unknown': { deep: ['yes', 'no', 3] },
    };
    expect(readYaml(writeYaml(rules))).toEqual({ ok: true, value: rules, warnings: [] });
  });
});

describe('what is refused before or instead of being followed', () => {
  const DIRECTIVE = 'A directive line (starting with %) is not accepted here. Remove it.';

  it('takes a byte order mark away first: the text reads as without it, and positions too', () => {
    expect(value('\uFEFFa: 1\n')).toEqual({ a: 1 });
    expect(problems('\uFEFFa: 1\na: 2\n')).toEqual(['Line 2, column 1: Map keys must be unique']);
    expect(problems('\uFEFF1: a\n')).toEqual([
      'Line 1, column 1: A key must be text. Put it in quotes.',
    ]);
    // The mark is not counted in the length: a text of exactly the bound behind it is read.
    const atBound = `a: 1\n#${'x'.repeat(MAX_YAML_CHARS - 6)}`;
    expect(value(`\uFEFF${atBound}`)).toEqual({ a: 1 });
  });

  it('refuses a directive wherever the parser finds one: after a byte order mark, after a comment', () => {
    for (const text of [
      '\uFEFF%YAML 1.1\n---\na: yes\nb: 010\nc: 1:30\nd: 2026-10-12\n',
      '# a comment first\n%YAML 1.1\n---\na: yes\n',
      '\uFEFF# both\n%YAML 1.1\n---\na: yes\n',
      // The version this reader reads anyway is a directive all the same.
      '%YAML 1.2\n---\na: 1\n',
      '%TAG !e! tag:example.com,2000:\n---\na: 1\n',
      // The default handle pointed elsewhere.
      '%TAG !! tag:example.com,2000:\n---\na: 1\n',
      // One the parser does not know: it only warns, and it is refused here.
      '%FOO bar\n---\na: 1\n',
    ]) {
      expect(readYaml(text).ok, text).toBe(false);
      expect(problems(text), text).toContain(DIRECTIVE);
    }
  });

  it('reads a line that starts with a percent sign inside a text as text', () => {
    expect(value("'a\n%b'\n")).toBe('a %b');
    expect(value('note: |\n  %YAML 1.1\n  is a directive\n')).toEqual({
      note: '%YAML 1.1\nis a directive\n',
    });
    expect(value('---\na: "%YAML"\nb: 50%\n')).toEqual({ a: '%YAML', b: '50%' });
  });

  it('reads a whole number a number holds exactly, and refuses one it cannot hold, never rounding', () => {
    const most = Number.MAX_SAFE_INTEGER;
    expect(most).toBe(9007199254740991);
    expect(value(`a: ${most}\nb: -${most}\nc: 0\nd: 7\ne: 0x1F\nf: 1.5\ng: 1e3\n`)).toEqual({
      a: most,
      b: -most,
      c: 0,
      d: 7,
      e: 31,
      f: 1.5,
      g: 1000,
    });
    const words = `This whole number cannot be sent exactly: whole numbers go from -${most} to ${most}. Put it in quotes to keep it as text.`;
    expect(problems('a: 9007199254740992\n')).toEqual([`Line 1, column 4: ${words}`]);
    expect(problems('a: -9007199254740992\n')).toEqual([`Line 1, column 4: ${words}`]);
    expect(problems('a: 9007199254740993\n')).toEqual([`Line 1, column 4: ${words}`]);
    expect(problems('a: 1\nticket: 12345678901234567890\n')).toEqual([
      `Line 2, column 9: ${words}`,
    ]);
    expect(problems('a: [1, 0x20000000000001]\n')).toEqual([`Line 1, column 8: ${words}`]);
    // In quotes it is text, digit for digit.
    expect(value('ticket: "12345678901234567890"\n')).toEqual({ ticket: '12345678901234567890' });
    // What is read is a number, not a bigint: JSON can carry it.
    expect(JSON.stringify(value('a: 7\n'))).toBe('{"a":7}');
  });

  it('refuses a whole number written with a fraction or an exponent when a number cannot hold it, and reads a real fraction', () => {
    const cannot = (became: string) =>
      `Line 1, column 4: This whole number cannot be sent exactly: it would become ${became}. Put it in quotes to keep it as text.`;
    // The digits denote a whole number, and the nearest double is another one.
    expect(problems('a: 9007199254740993.0\n')).toEqual([cannot('9007199254740992')]);
    expect(problems('a: 9007199254740993e0\n')).toEqual([cannot('9007199254740992')]);
    expect(problems('a: 9.007199254740993e15\n')).toEqual([cannot('9007199254740992')]);
    expect(problems('a: -9007199254740993.0\n')).toEqual([cannot('-9007199254740992')]);
    expect(problems('a: 900719925474099.3e1\n')).toEqual([cannot('9007199254740992')]);
    expect(problems('a: 90071992547409930e-1\n')).toEqual([cannot('9007199254740992')]);
    expect(problems('a: 6.02e23\n')).toEqual([cannot('601999999999999995805696')]);
    // A whole number a double holds exactly is read, however it is written.
    expect(
      value(
        'a: 9007199254740992.0\nb: 1e20\nc: 12.0\nd: 1.2e1\ne: 0.0\nf: 120e-1\ng: +4.e2\nh: -12.0\n',
      ),
    ).toEqual({ a: 9007199254740992, b: 1e20, c: 12, d: 12, e: 0, f: 12, g: 400, h: -12 });
    // A real fraction is read as the nearest double, as every YAML reader reads it.
    expect(
      value('a: 0.1\nb: 1.5e-3\nc: 9007199254740993.5\nd: 123456789.123456789\ne: .5\nf: 1e-400\n'),
    ).toEqual({
      a: 0.1,
      b: 0.0015,
      c: 9007199254740994,
      d: 123456789.12345679,
      e: 0.5,
      f: 0,
    });
    // In quotes it is text, digit for digit.
    expect(value('a: "9007199254740993.0"\n')).toEqual({ a: '9007199254740993.0' });
  });

  it('judges a number with an enormous exponent without raising a power of ten that size', () => {
    const started = performance.now();
    expect(
      value('a: 0e999999999\nb: 0.0e-999999999\nc: 1e-999999999\nd: 12345e-999999999\n'),
    ).toEqual({ a: 0, b: 0, c: 0, d: 0 });
    // Not finite: refused as that, before any counting.
    expect(problems('a: 1e999999999\n')).toEqual([
      'Line 1, column 4: A number must be finite: .inf and .nan cannot be sent.',
    ]);
    expect(performance.now() - started).toBeLessThan(1_000);
  });

  it('says in its own words that the rest of an unclosed text could not be read', () => {
    // The parser's second error for this text has an engine's words; they are never shown.
    expect(problems('a: [\n')).toEqual([
      'Line 2, column 1: Flow sequence in block collection must be sufficiently indented and end with a ]',
      'Line 1, column 1: The text could not be read past this point.',
    ]);
  });
});

describe('the bounds that keep the page from stalling', () => {
  const tooDeep = (line: number) =>
    `Line ${line}, column 1: This text nests more than ${MAX_YAML_DEPTH} levels deep here (lists in lists, brackets in brackets), and deeper is not read. Nothing of it was read.`;
  const brackets = (count: number) => `${'['.repeat(count)}${']'.repeat(count)}\n`;

  it('are small: rules and properties are a few kilobytes and a few levels', () => {
    expect(MAX_YAML_CHARS).toBe(16_384);
    expect(MAX_YAML_DEPTH).toBe(32);
  });

  it('measures the depth from what the parser takes for structure', () => {
    expect(deepestNesting('')).toEqual({ depth: 0, line: 1 });
    expect(deepestNesting('a: 1\n')).toEqual({ depth: 2, line: 1 });
    expect(deepestNesting('rules:\n  - glob: x\n    case: y\n')).toEqual({ depth: 4, line: 2 });
    expect(deepestNesting('a: 1\nb: [[1], {c: [2]}]\n')).toEqual({ depth: 5, line: 2 });
    expect(deepestNesting(`${'- '.repeat(5)}a\n`).depth).toBe(6);
    expect(deepestNesting(`${'? '.repeat(5)}a\n`).depth).toBe(6);
    expect(deepestNesting(`${'a: '.repeat(5)}1\n`).depth).toBe(6);
    // Brackets and colons in a quoted text, a block of text or a comment are text.
    expect(deepestNesting('a: "[[[[ {{{{ x: y: z:"\n').depth).toBe(2);
    expect(deepestNesting("a: '[[[['\n").depth).toBe(2);
    expect(deepestNesting('a: |\n  [[[[ b: c: d:\n  - - - -\n').depth).toBe(2);
    expect(deepestNesting('# [[[[ a: b: c:\na: 1\n').depth).toBe(2);
    // Comment lines stand in no block, however they are indented.
    const comments = Array.from({ length: 40 }, (_, step) => `${' '.repeat(step + 1)}# c\n`);
    expect(deepestNesting(`a: 1\n${comments.join('')}b: 2\n`).depth).toBe(2);
    expect(value(`a: 1\n${comments.join('')}b: 2\n`)).toEqual({ a: 1, b: 2 });
    // Inside brackets a colon opens no level: the pairs of a mapping stand side by side.
    expect(deepestNesting('{a: 1, b: 2, c: 3, d: 4}\n').depth).toBe(2);
    // The first line that is the deepest, counted through a text that spans lines.
    expect(deepestNesting('a: "x\n  y"\nb:\n  c: [[\n').line).toBe(4);
  });

  it('reads brackets 31 deep and refuses 32, before the parser runs', () => {
    expect(deepestNesting(brackets(MAX_YAML_DEPTH - 1)).depth).toBe(MAX_YAML_DEPTH);
    const parsedBefore = parsed.length;
    expect(readYaml(brackets(MAX_YAML_DEPTH - 1)).ok).toBe(true);
    expect(parsed.length).toBe(parsedBefore + 1);
    expect(problems(`a: 1\n${brackets(MAX_YAML_DEPTH)}`)).toEqual([tooDeep(2)]);
    expect(problems('['.repeat(4000))).toEqual([tooDeep(1)]);
    // Neither was given to the parser.
    expect(parsed.length).toBe(parsedBefore + 1);
  });

  it('refuses lists in lists, keys in keys and mappings on one line beyond the depth', () => {
    for (const step of ['- ', '? ', 'a: ']) {
      const within = `${step.repeat(MAX_YAML_DEPTH - 1)}a\n`;
      const beyond = `${step.repeat(MAX_YAML_DEPTH)}a\n`;
      expect(deepestNesting(within).depth, step).toBe(MAX_YAML_DEPTH);
      expect(problems(within).join(' '), step).not.toContain('levels deep');
      expect(problems(beyond), step).toEqual([tooDeep(1)]);
    }
    // Indentation, step by step.
    const stairs = (steps: number) =>
      Array.from({ length: steps }, (_, step) => `${' '.repeat(step)}a:\n`).join('');
    expect(readYaml(stairs(MAX_YAML_DEPTH - 1)).ok).toBe(true);
    expect(problems(stairs(MAX_YAML_DEPTH))).toEqual([tooDeep(MAX_YAML_DEPTH)]);
  });

  it('does not refuse a text for brackets that are text', () => {
    const many = '['.repeat(100);
    expect(value(`pattern: "${many}"\nnote: |\n  ${many}\n# ${many}\n`)).toEqual({
      pattern: many,
      note: `${many}\n`,
    });
  });

  it('gives a text over the length bound to neither the scan nor the parser, and cuts nothing', () => {
    const parsedBefore = parsed.length;
    const long = `a: 1\n#${'x'.repeat(MAX_YAML_CHARS)}`;
    const read = readYaml(long);
    expect(read).toEqual({
      ok: false,
      problems: [
        {
          message: `This text is ${long.length} characters long, and at most ${MAX_YAML_CHARS} are read here. Nothing of it was read, and nothing was cut: shorten it.`,
          line: null,
          column: null,
        },
      ],
    });
    expect(parsed.length).toBe(parsedBefore);
  });
});
