/**
 * YAML engines for unit tests (`src/lib/yaml-reading.ts`): happy-dom has no worker, and the real
 * one is run by the browser specs.
 *
 * `inlineEngine` reads with the reader itself, on the test's thread, and answers a turn later, as
 * a worker does: never within the call that started the reading. `tests/unit/setup.ts` installs
 * it before every test.
 *
 * `heldEngine` reads nothing by itself: the test says when each reading begins, answers or fails,
 * and sees which were ended. It is how the time limit and a superseded reading are tested.
 */
import { readYaml, type YamlRead } from '../../../src/lib/yaml';
import type { EngineReport, YamlEngine } from '../../../src/lib/yaml-reading';

export const inlineEngine: YamlEngine = {
  start(text, report) {
    const timer = setTimeout(() => {
      report.began();
      report.done(readYaml(text));
    }, 0);
    return () => clearTimeout(timer);
  },
};

/** One reading a held engine was asked for. */
export interface HeldReading {
  text: string;
  /** The page ended it. */
  ended: boolean;
  began: () => void;
  /** Answer with what the reader finds for its text, or with `read`. */
  done: (read?: YamlRead) => void;
  failed: () => void;
}

export function heldEngine(): { engine: YamlEngine; readings: HeldReading[] } {
  const readings: HeldReading[] = [];
  const engine: YamlEngine = {
    start(text: string, report: EngineReport) {
      const reading: HeldReading = {
        text,
        ended: false,
        began: report.began,
        done: (read) => report.done(read ?? readYaml(text)),
        failed: report.failed,
      };
      readings.push(reading);
      return () => {
        reading.ended = true;
      };
    },
  };
  return { engine, readings };
}
