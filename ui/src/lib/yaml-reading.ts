/**
 * Reading a typed YAML text off the page's thread, with a time limit of the page's own.
 *
 * Why: what a person types is untrusted text, and no bound on its size or its nesting has kept
 * the parser's time small for every shape of text (`[a: a: a: …]` on one line reads for seconds
 * inside every bound of `lib/yaml.ts`). So the bounds are no longer the guarantee. The guarantee
 * is here: the page's thread never runs the reader on what a person typed. It hands the text to an
 * engine, which reads it elsewhere (a worker: `yaml-worker-engine.ts`), and waits for the answer
 * no longer than `YAML_LIMIT_MS`. When the limit passes the engine is ended, and the answer is a
 * refusal in plain words: the text is too complex to read here. The bounds of `lib/yaml.ts` (its
 * length, its depth) still run first, inside the engine, so an ordinary text that is too long or
 * too deep is refused at once and does not wait for the limit.
 *
 * The limit counts from the moment the engine says the reading itself began (`began`): a worker
 * that is still starting is not a text that is too complex. Until then the reading is bounded by
 * `YAML_START_LIMIT_MS`, and an engine that does not begin in that time, or says that it cannot
 * read at all (`failed`: the worker could not be started, or broke), is ended and answered as a
 * refusal too, in other words: the text was not read, and that is not the text's fault.
 *
 * One answer per reading, and none after it is ended: `readOffThread` returns what ends it, and
 * whoever starts a newer reading of the same text box ends the older one first
 * (`usePausedReading`), so an older answer is never shown.
 *
 * The engine is one interface (`YamlEngine`), so unit tests drive all of this without a worker;
 * the browser specs run the real one.
 */
import type { YamlRead } from './yaml';
import { workerEngine } from './yaml-worker-engine';

/** How long the reading of one text may take, once it began, in milliseconds. */
export const YAML_LIMIT_MS = 1_000;

/** How long an engine may take to begin reading, in milliseconds. */
export const YAML_START_LIMIT_MS = 10_000;

export const TOO_COMPLEX_WORDS =
  'This text is too complex to read here: reading it took more than a second and was stopped. Nothing of it was read, and nothing was cut; it is still in the text box. Make it simpler, for example with each key on a line of its own.';

export const NOT_STARTED_WORDS =
  'This text was not read: the part of this page that reads YAML could not be started. Nothing was sent, and the text is still in the text box. Change the text to try again, or reload the page.';

/** What an engine tells of one reading. */
export interface EngineReport {
  /** The reading itself runs now: the time limit counts from here. */
  began: () => void;
  /** What reading the text found. */
  done: (read: YamlRead) => void;
  /** The engine cannot read at all. */
  failed: () => void;
}

/** Something that reads a text as YAML away from the page's thread. */
export interface YamlEngine {
  /**
   * Start reading `text`, and finding in it the places `locate` names (`readYaml`). `done` and
   * `failed` are reported a turn later at the earliest, not within this call (a worker cannot do
   * otherwise); `began` may be. Answers what ends the reading: once that is called, the engine
   * stops the work if it can; whatever it still reports is dropped by `readOffThread`.
   */
  start: (text: string, report: EngineReport, locate: readonly string[]) => () => void;
}

let chosen: YamlEngine | null = null;

/** Use `engine` for every reading from now on; null goes back to the worker. For tests. */
export function setYamlEngine(engine: YamlEngine | null) {
  chosen = engine;
}

const refusal = (message: string): YamlRead => ({
  ok: false,
  problems: [{ message, line: null, column: null }],
});

/**
 * Read `text` as YAML off the page's thread, finding the places `locate` names in it. `answer` is
 * called once, with what the reading found or with a refusal when it took too long or could not
 * be done; never after the returned function, which ends the reading, was called.
 */
export function readOffThread(
  text: string,
  answer: (read: YamlRead) => void,
  locate: readonly string[] = [],
): () => void {
  let over = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let endEngine: (() => void) | null = null;
  const end = () => {
    over = true;
    if (timer !== null) clearTimeout(timer);
    timer = null;
    endEngine?.();
  };
  const finish = (read: YamlRead) => {
    if (over) return;
    end();
    answer(read);
  };
  const within = (ms: number, words: string) => {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => finish(refusal(words)), ms);
  };
  within(YAML_START_LIMIT_MS, NOT_STARTED_WORDS);
  try {
    chosen ??= workerEngine();
    endEngine = chosen.start(
      text,
      {
        began: () => within(YAML_LIMIT_MS, TOO_COMPLEX_WORDS),
        done: finish,
        failed: () => finish(refusal(NOT_STARTED_WORDS)),
      },
      locate,
    );
  } catch {
    // Whatever starting the engine threw (a browser that refuses the worker): nothing was read.
    // The answer comes a turn later, as every answer does, never inside this call.
    setTimeout(() => finish(refusal(NOT_STARTED_WORDS)), 0);
  }
  return end;
}
