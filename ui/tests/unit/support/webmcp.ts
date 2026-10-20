/**
 * A fake WebMCP model context (`document.modelContext`, W3C draft of the Web Machine Learning
 * group): it records each `registerTool` call with the tool and the options it was given, answers
 * it with a Promise that resolves (or with the answer a test chooses), and records when the
 * signal of each registration aborts. Aborting is the only way the draft has to unregister.
 *
 * It also stands for a browser whose storage is blocked: reading `window.localStorage` then throws,
 * as it does where a browser refuses a page its storage.
 */

/** A tool as the draft defines it (`ModelContextTool`), as far as these tests look at it. */
export interface DraftTool {
  name: string;
  title?: string;
  description: string;
  inputSchema?: object;
  execute: (input: object, options: { signal: AbortSignal }) => Promise<unknown>;
  annotations?: { readOnlyHint?: boolean };
}

export interface Registration {
  tool: DraftTool;
  options: { signal?: AbortSignal };
  /** The registration's signal has aborted: the tool is no longer registered. */
  aborted: boolean;
}

export interface FakeModelContext {
  registrations: Registration[];
  /** The registrations whose signal has not aborted. */
  live: () => Registration[];
  /** Run the live tool `name` as an assistant would. */
  run: (name: string, input?: Record<string, unknown>) => Promise<unknown>;
}

/** Install the fake on `document`; `answer` says how each registration's Promise settles. */
export function installModelContext(
  answer: (tool: DraftTool) => Promise<undefined> = () => Promise.resolve(undefined),
): FakeModelContext {
  const registrations: Registration[] = [];
  const context = {
    registerTool: (tool: DraftTool, options: { signal?: AbortSignal } = {}) => {
      const entry: Registration = { tool, options, aborted: options.signal?.aborted === true };
      registrations.push(entry);
      options.signal?.addEventListener('abort', () => {
        entry.aborted = true;
      });
      return answer(tool);
    },
  };
  Object.defineProperty(document, 'modelContext', { configurable: true, value: context });
  const live = () => registrations.filter((entry) => !entry.aborted);
  return {
    registrations,
    live,
    run: (name, input = {}) => {
      const entry = live().find((candidate) => candidate.tool.name === name);
      if (entry === undefined) return Promise.reject(new Error(`no live tool named ${name}`));
      return entry.tool.execute(input, { signal: new AbortController().signal });
    },
  };
}

/** The browser offers no WebMCP. */
export function removeModelContext() {
  Reflect.deleteProperty(document, 'modelContext');
}

/** Reading `window.localStorage` throws, as where the browser blocks the page's storage. */
export function blockStorage(): () => void {
  const own = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    get: () => {
      throw new DOMException('The operation is insecure.', 'SecurityError');
    },
  });
  return () => {
    if (own === undefined) Reflect.deleteProperty(window, 'localStorage');
    else Object.defineProperty(window, 'localStorage', own);
  };
}

/** The tools the page may register, and the four of them that only read. */
export const FIVE = [
  'get_selection',
  'open_source',
  'open_comparison',
  'preview_names',
  'prepare_proposal',
] as const;
export const READ_ONLY = ['get_selection', 'open_source', 'open_comparison', 'preview_names'];
