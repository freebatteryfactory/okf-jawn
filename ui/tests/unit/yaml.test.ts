/**
 * Reading what a person typed as YAML into the JSON the wire carries, and writing it back: what
 * is refused before anything is sent, and what the text means to every reader.
 */
import { describe, expect, it, vi } from 'vitest';
import { describeProblem, MAX_YAML_CHARS, readYaml, writeYaml } from '../../src/lib/yaml';

vi.mock('yaml', async (original) => {
  const actual = await original<typeof import('yaml')>();
  return {
    ...actual,
    parseDocument: (text: string, options?: Parameters<typeof actual.parseDocument>[1]) => {
      // One text makes the parser throw, as extreme input can.
      if (text === 'the parser throws on this') throw new RangeError('Maximum call stack');
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
      `This text is ${MAX_YAML_CHARS + 1} characters long; at most ${MAX_YAML_CHARS} are read. Nothing was read.`,
    ]);
  });

  it('shows what the parser throws as a problem, never as an empty or a valid document', () => {
    expect(readYaml('the parser throws on this')).toEqual({
      ok: false,
      problems: [
        {
          message: 'This text could not be read as YAML: Maximum call stack',
          line: null,
          column: null,
        },
      ],
    });
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
    expect(problems(laughs).join(' ')).toMatch(/could not be read as YAML/);
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
