/** Feature-detected page tools call the application's adapters; no approval tool is registered. */
export interface PageTool {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  execute: (input: unknown) => Promise<unknown>;
}
interface ModelContext {
  registerTool: (tool: PageTool, options?: { signal?: AbortSignal }) => void;
  unregisterTool?: (name: string) => void;
}
export function registerPageTools(tools: readonly PageTool[], enabled: boolean): () => void {
  const context = (document as Document & { modelContext?: ModelContext }).modelContext;
  if (!enabled || !context) return () => {};
  const permitted = new Set(['get_selection', 'open_source', 'preview_names', 'prepare_proposal']);
  const controller = new AbortController();
  const registered: string[] = [];
  for (const tool of tools) {
    if (!permitted.has(tool.name)) throw new Error(`Unsupported page tool: ${tool.name}`);
    context.registerTool(tool, { signal: controller.signal }); registered.push(tool.name);
  }
  return () => { controller.abort(); for (const name of registered) context.unregisterTool?.(name); };
}
