/**
 * Reading YAML off the page's thread (`src/lib/yaml-reading.ts`): one answer per reading, the
 * time limit from the moment the reading began, the bound on an engine that does not begin, and
 * nothing answered after a reading was ended. The engine here is held by the test; the real
 * worker is run by the browser specs.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { YamlRead } from '../../src/lib/yaml';
import {
  NOT_STARTED_WORDS,
  readOffThread,
  setYamlEngine,
  TOO_COMPLEX_WORDS,
  YAML_LIMIT_MS,
  YAML_START_LIMIT_MS,
} from '../../src/lib/yaml-reading';
import { heldEngine, inlineEngine } from './support/yaml-engine';

const refused = (words: string): YamlRead => ({
  ok: false,
  problems: [{ message: words, line: null, column: null }],
});

/** Start a reading on an engine the test holds. */
function begin(text = 'a: 1\n') {
  const held = heldEngine();
  setYamlEngine(held.engine);
  const answers: YamlRead[] = [];
  const end = readOffThread(text, (read) => answers.push(read));
  const reading = held.readings.at(0);
  if (reading === undefined) throw new Error('the engine was not started');
  return { answers, end, reading };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('reading YAML off the page’s thread', () => {
  it('hands the text to the engine and answers once with what it found', () => {
    const { answers, reading } = begin('a: 1\nb: [x, z]\n');
    expect(reading.text).toBe('a: 1\nb: [x, z]\n');
    expect(answers).toEqual([]);
    reading.began();
    reading.done();
    expect(answers).toEqual([{ ok: true, value: { a: 1, b: ['x', 'z'] }, warnings: [] }]);
    // Nothing more comes of it: not a second answer, and not the limit a second later.
    reading.done();
    vi.advanceTimersByTime(YAML_START_LIMIT_MS * 2);
    expect(answers).toHaveLength(1);
  });

  it('ends a reading that takes longer than the limit, and says that the text is too complex', () => {
    // The page's own limit: one second.
    expect(YAML_LIMIT_MS).toBe(1_000);
    const { answers, reading } = begin();
    reading.began();
    vi.advanceTimersByTime(YAML_LIMIT_MS - 1);
    expect(answers).toEqual([]);
    expect(reading.ended).toBe(false);
    vi.advanceTimersByTime(1);
    expect(answers).toEqual([refused(TOO_COMPLEX_WORDS)]);
    expect(reading.ended).toBe(true);
    // Its answer, if it comes after all, is dropped.
    reading.done();
    expect(answers).toHaveLength(1);
  });

  it('counts the limit from the moment the reading began, not from the engine’s start', () => {
    const { answers, reading } = begin();
    // An engine that is still starting is not a text that is too complex.
    vi.advanceTimersByTime(YAML_LIMIT_MS * 3);
    expect(answers).toEqual([]);
    reading.began();
    vi.advanceTimersByTime(YAML_LIMIT_MS - 1);
    expect(answers).toEqual([]);
    reading.done();
    expect(answers).toHaveLength(1);
    expect(answers.at(0)?.ok).toBe(true);
  });

  it('bounds an engine that does not begin, and says that the text was not read', () => {
    const { answers, reading } = begin();
    vi.advanceTimersByTime(YAML_START_LIMIT_MS - 1);
    expect(answers).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(answers).toEqual([refused(NOT_STARTED_WORDS)]);
    expect(reading.ended).toBe(true);
  });

  it('says that the text was not read when the engine fails, and ends it', () => {
    const { answers, reading } = begin();
    reading.began();
    reading.failed();
    expect(answers).toEqual([refused(NOT_STARTED_WORDS)]);
    expect(reading.ended).toBe(true);
  });

  it('says that the text was not read when the engine cannot be started, a turn later', () => {
    setYamlEngine({
      start() {
        throw new Error('no worker here');
      },
    });
    const answers: YamlRead[] = [];
    readOffThread('a: 1\n', (read) => answers.push(read));
    expect(answers).toEqual([]);
    vi.advanceTimersByTime(0);
    expect(answers).toEqual([refused(NOT_STARTED_WORDS)]);
    vi.advanceTimersByTime(YAML_START_LIMIT_MS);
    expect(answers).toHaveLength(1);
  });

  it('answers nothing once it was ended: not what the engine found, and not the limit', () => {
    const { answers, end, reading } = begin();
    reading.began();
    end();
    expect(reading.ended).toBe(true);
    reading.done();
    reading.failed();
    vi.advanceTimersByTime(YAML_START_LIMIT_MS * 2);
    expect(answers).toEqual([]);
  });
});

describe('the engine of the unit tests', () => {
  it('answers a turn later, never within the call, and not at all once ended', () => {
    setYamlEngine(inlineEngine);
    const answers: YamlRead[] = [];
    readOffThread('a: [1\n', (read) => answers.push(read));
    expect(answers).toEqual([]);
    vi.advanceTimersByTime(0);
    expect(answers).toHaveLength(1);
    expect(answers.at(0)?.ok).toBe(false);
    const end = readOffThread('a: 1\n', (read) => answers.push(read));
    end();
    vi.advanceTimersByTime(YAML_START_LIMIT_MS);
    expect(answers).toHaveLength(1);
  });
});
