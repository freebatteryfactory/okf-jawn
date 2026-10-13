/**
 * The engine that reads YAML in a Web Worker (`yaml.worker.ts`), for `lib/yaml-reading.ts`.
 *
 * Each reading has a worker to itself, so ending a reading is ending its worker: the browser
 * stops the thread whatever it is doing, which is the one way to stop a parser that does not
 * come back. A worker that answered is kept as the spare for the next reading (one at most), so
 * an ordinary text does not pay for a start each time; a worker whose reading was ended, or that
 * broke, is never used again.
 *
 * The text is sent once the worker has said it is ready, and `began` is reported then: the
 * page's time limit counts the reading, not the worker's start.
 *
 * What the page's Content-Security-Policy must allow for this: the worker's script is a file of
 * the build, served from the page's own origin and loaded by URL as a module worker. That is
 * `worker-src 'self'` (a browser that does not know `worker-src` falls back to `child-src`, then
 * `script-src`, then `default-src`). No `blob:` or `data:` URL is used, and nothing is evaluated
 * from text. The script is served with a JavaScript media type, which a module worker requires.
 */

import type { YamlRead } from './yaml';
import type { EngineReport, YamlEngine } from './yaml-reading';

/** What the worker says: that it has started, or what it read. */
type FromWorker = { ready: true } | { read: YamlRead };

interface Started {
  worker: Worker;
  ready: boolean;
}

export function workerEngine(): YamlEngine {
  /** A worker that has read a text and waits for the next. */
  let spare: Started | null = null;
  return {
    start(text: string, report: EngineReport) {
      const mine: Started = spare ?? {
        worker: new Worker(new URL('./yaml.worker.ts', import.meta.url), { type: 'module' }),
        ready: false,
      };
      spare = null;
      const { worker } = mine;
      /** This reading is over: it answered, broke, or was ended. */
      let over = false;
      const letGo = () => {
        over = true;
        worker.onmessage = null;
        worker.onerror = null;
        worker.onmessageerror = null;
      };
      const send = () => {
        report.began();
        worker.postMessage(text);
      };
      const end = () => {
        // A reading that answered left its worker as the spare: there is nothing to end.
        if (over) return;
        letGo();
        worker.terminate();
      };
      const broke = () => {
        end();
        report.failed();
      };
      worker.onmessage = (event: MessageEvent<FromWorker>) => {
        if ('ready' in event.data) {
          mine.ready = true;
          send();
          return;
        }
        letGo();
        if (spare === null) spare = mine;
        else worker.terminate();
        report.done(event.data.read);
      };
      worker.onerror = broke;
      worker.onmessageerror = broke;
      if (mine.ready) send();
      return end;
    },
  };
}
