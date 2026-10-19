/**
 * The MCP App's `ViewData`: a View's two reads as the server tools the App calls through the
 * official bridge, `show` for a source and the app-only `read_object` for a block of a retained
 * object.
 *
 * This is the App's one wire boundary: a call's undefined optionals are omitted here, once, as it
 * leaves for the host (`omitUndefined`); the views components pass every field through. A tool's
 * refusal (`isError`) is checked before its structured content is read, because a refusal carries
 * none, and is the read's error in the tool's own words, bounded. An answer is read with the
 * generated response schema; the views components read it again (`features/views/data.ts`).
 */
import { z } from 'zod';
import { zGetObjectResponse, zReadItemResponse } from '../api/generated/zod.gen';
import type { ViewData } from '../features/views/data';
import { omitUndefined } from '../lib/wire';

/** One server tool call, as the bridge's `callServerTool` takes it. */
export interface ServerToolCall {
  name: string;
  arguments: Record<string, unknown>;
}
export type CallServerTool = (call: ServerToolCall) => Promise<unknown>;

const toolResult = z.object({
  isError: z.boolean().optional(),
  content: z.array(z.looseObject({ type: z.string(), text: z.string().optional() })).optional(),
  structuredContent: z.unknown().optional(),
});
const refusalLimit = 512;

/** The tool's own refusal text, bounded; cut on a code point boundary, ending with an ellipsis. */
function refusalMessage(output: z.infer<typeof toolResult>, fallback: string): string {
  const text = (output.content ?? [])
    .flatMap((part) => (part.type === 'text' && part.text ? [part.text] : []))
    .join('\n')
    .trim();
  if (text.length === 0) return fallback;
  if (text.length <= refusalLimit) return text;
  let end = refusalLimit - 1;
  const last = text.charCodeAt(end - 1);
  if (last >= 0xd800 && last <= 0xdbff) end -= 1; // do not leave half a surrogate pair
  return `${text.slice(0, end)}…`;
}

/** One tool call's structured content, or its refusal as an error. */
async function structured(
  callServerTool: CallServerTool,
  name: string,
  input: Record<string, unknown>,
  fallback: string,
): Promise<unknown> {
  const output = toolResult.parse(await callServerTool({ name, arguments: omitUndefined(input) }));
  if (output.isError === true) throw new Error(refusalMessage(output, fallback));
  return output.structuredContent;
}

/** A View's reads over the App's server tools. */
export function serverToolData(callServerTool: CallServerTool): ViewData {
  return {
    readItem: async (request) =>
      zReadItemResponse.parse(
        await structured(callServerTool, 'show', request, 'The host refused to read the source.'),
      ),
    getObject: async (request) =>
      zGetObjectResponse.parse(
        await structured(
          callServerTool,
          'read_object',
          request,
          'The host refused to read the dataset.',
        ),
      ),
  };
}
