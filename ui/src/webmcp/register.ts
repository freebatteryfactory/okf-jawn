/**
 * Registering the page tools with the browser's model context, as the W3C draft defines it
 * (WebMCP, webmachinelearning.github.io/webmcp, read 2026-10-18):
 * `document.modelContext.registerTool(tool, { signal })` returns a Promise, and there is no
 * `unregisterTool`: aborting the registration's signal unregisters the tool, and the draft then
 * rejects that registration's Promise with the abort's reason.
 *
 * Only five tools may be registered (SPEC section 9: "Page tools expose the selected
 * source/revision/range, open a comparison/source, preview names, and prepare a proposal ... No
 * Verify/Approve page tools."). A set that holds any other name, and above all one named like
 * verify, approve or review, is refused whole, before anything is registered.
 *
 * A registration's Promise is always handled. A rejection that comes after the registration's
 * own abort is that abort, and is expected. Any other one (the browser already has a tool of that
 * name, its permission policy refuses tools, the tool is not valid) is the browser's refusal of
 * that tool, told to `onRefused` with its words; the page never claims a tool it was refused. A
 * resolved one is told to `onRegistered`.
 */

/** The draft's hints about a tool; only `readOnlyHint` is used here. */
export interface ToolAnnotations {
  readOnlyHint?: boolean;
}

/** A tool as the draft's `ModelContextTool` dictionary has it. */
export interface ModelContextTool {
  name: string;
  title?: string;
  description: string;
  inputSchema: object;
  execute: (input: object, options: { signal: AbortSignal }) => Promise<unknown>;
  annotations?: ToolAnnotations;
}

/** The draft's `ModelContext`, as far as this page uses it. */
export interface ModelContext {
  registerTool: (tool: ModelContextTool, options?: { signal?: AbortSignal }) => Promise<undefined>;
}

declare global {
  interface Document {
    /** Present only where the browser offers WebMCP. */
    readonly modelContext?: ModelContext | null;
  }
}

/** The only tools this page registers: four that read, and one that fills a form. */
export const PAGE_TOOL_NAMES = [
  'get_selection',
  'open_source',
  'open_comparison',
  'preview_names',
  'prepare_proposal',
] as const;

export type PageToolName = (typeof PAGE_TOOL_NAMES)[number];

/**
 * A name no page tool may have, whatever else it says: one with a word verify, approve or review
 * in it (`verify_item`, `review`, `approve-proposal`; not `preview_names`).
 */
const FORBIDDEN = /(?:^|[_.-])(?:verify|approve|review)/i;

/** The browser's model context, or null where it offers no WebMCP. */
export function modelContext(): ModelContext | null {
  const context = document.modelContext;
  return context !== undefined && context !== null && typeof context.registerTool === 'function'
    ? context
    : null;
}

export interface RegisterOptions {
  /** Aborting it unregisters every tool registered here. */
  signal: AbortSignal;
  /** The browser took the tool `name`: its registration's Promise resolved. */
  onRegistered?: (name: string) => void;
  /** The browser refused the tool `name`; `reason` says why, in its words. */
  onRefused?: (name: string, reason: string) => void;
}

/**
 * Register `tools` with the browser's model context, each under `signal`. Nothing is registered
 * where the browser offers no WebMCP, or once `signal` has aborted. Throws, registering none of
 * them, when a tool is not one of the five.
 */
export function registerPageTools(
  tools: readonly ModelContextTool[],
  { signal, onRegistered, onRefused }: RegisterOptions,
) {
  const permitted: readonly string[] = PAGE_TOOL_NAMES;
  for (const tool of tools) {
    if (FORBIDDEN.test(tool.name)) {
      throw new Error(
        `Refused page tool ${tool.name}: no page tool verifies, approves or reviews.`,
      );
    }
    if (!permitted.includes(tool.name)) {
      throw new Error(`Refused page tool ${tool.name}: it is not one of this page's tools.`);
    }
  }
  const context = modelContext();
  if (context === null || signal.aborted) return;
  const refused = (name: string, reason: unknown) => {
    // After the abort, the rejection is the abort itself: the tool was unregistered on purpose.
    if (signal.aborted) return;
    onRefused?.(name, reason instanceof Error ? reason.message : String(reason));
  };
  for (const tool of tools) {
    try {
      // An answer that is no Promise (an earlier shape of the draft) is taken as done.
      Promise.resolve(context.registerTool(tool, { signal })).then(
        () => {
          if (!signal.aborted) onRegistered?.(tool.name);
        },
        (reason: unknown) => refused(tool.name, reason),
      );
    } catch (reason) {
      // A browser that throws instead of answering with a rejected Promise.
      refused(tool.name, reason);
    }
  }
}
