/**
 * The worker that reads YAML for the page (`yaml-worker-engine.ts`): the reader of `lib/yaml.ts`
 * runs here, on a thread of its own, so however long a text takes, the page goes on answering the
 * person, and the page can end the work by ending the worker.
 *
 * It says `ready` once, when it has started. After that it reads one text at a time: each message
 * is a text, and each answer is what `readYaml` found for it (a plain value, as JSON carries it,
 * or the problems). `readYaml` catches whatever the parser throws, so a text never breaks the
 * worker; if something else does, the page sees the worker's error event.
 */
import { readYaml } from './yaml';

addEventListener('message', (event: MessageEvent<string>) => {
  postMessage({ read: readYaml(event.data) });
});
postMessage({ ready: true });
