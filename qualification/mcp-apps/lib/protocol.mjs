/**
 * The protocol half of the MCP Apps qualification: what an MCP client is told by the server.
 *
 * observeProtocol asks and writes down; judgeProtocol decides from that record alone and
 * returns one criterion per rule. A wrong answer, or a call the client rejects, is an
 * observation and fails its criterion: nothing here throws for it, so one failure cannot hide
 * the others. Only a client that cannot connect at all is the caller's (environment) problem.
 *
 * The server asked is this harness, not the product: these rules show that rmcp and the MCP
 * client carry the Apps declarations and the read_object contract, not that the product
 * server's handlers work.
 */

import { createHash } from 'node:crypto';

/** One sentence per criterion: what must hold for it to pass. The keys are the criterion ids. */
export const PROTOCOL_RULES = Object.freeze({
  'protocol/render_tools': 'tools/list lists exactly the four render tools, each with _meta.ui.resourceUri naming the shared App resource.',
  'protocol/app_only_tools': 'The tools whose _meta.ui.visibility is exactly ["app"] are read_object and show, and every listed tool is a render tool or one of these.',
  'protocol/read_object_declaration': "The listed read_object carries the product's inputSchema and outputSchema from api/mcp-tools.json unchanged.",
  'protocol/resource_listed': 'resources/list lists exactly one resource, the shared App resource.',
  'protocol/resource_mime_type': "resources/read returns text whose mimeType is the ext-apps SDK's RESOURCE_MIME_TYPE.",
  'protocol/resource_meta_ui': 'resources/read attaches _meta.ui.csp as an object with connectDomains and resourceDomains arrays.',
  'protocol/resource_is_built_bundle': 'The text resources/read returns hashes to the sha256 the bundle manifest of this run records.',
  'protocol/render_tool_results': 'Every render tool returns structuredContent for the App and a non-empty text block for hosts without Apps.',
  'protocol/show_bound_sources': "show, called as PresentView calls it, returns the bound item and revision for every resolved binding of the present result.",
  'protocol/read_object_ranged_loop': "read_object, called in PresentView's loop, serves each retained dataset in more than one block, each block naming the object and the offset asked for, until has_more is false.",
  'protocol/read_object_digest': 'The bytes assembled from those blocks hash to the binding\'s materialized digest, and total_size is their count.',
  'protocol/read_object_unknown_digest_refused': 'read_object answers a digest nobody retains with isError and no structuredContent.',
  'protocol/read_object_unknown_argument_refused': "read_object answers a request with an argument the product request type does not have with isError and no structuredContent, as the product's deny_unknown_fields does.",
});

/** The criterion ids judgeProtocol always returns, in order. */
export const PROTOCOL_CRITERIA = Object.freeze(Object.keys(PROTOCOL_RULES));

const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const messageOf = (error) => String(error?.message ?? error);
const names = (items) => items.map((item) => item.name).sort();
const sameList = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const textOf = (result) => (result?.content ?? []).filter((block) => block?.type === 'text' && typeof block.text === 'string').map((block) => block.text).join('\n');

/** JSON text with every object's keys sorted, so two encodings of one value compare equal. */
export function canonicalJson(value) {
  const sorted = (item) => {
    if (Array.isArray(item)) return item.map(sorted);
    if (item !== null && typeof item === 'object') {
      return Object.fromEntries(Object.keys(item).sort().map((key) => [key, sorted(item[key])]));
    }
    return item;
  };
  return JSON.stringify(sorted(value));
}

/** The value of `call`, or the message of what it threw: `{ value }` or `{ error }`. */
async function attempt(call) {
  try {
    return { value: await call() };
  } catch (error) {
    return { error: messageOf(error) };
  }
}

/** How a refusal looked: an error flag, no structured content, and the tool's text. */
function refusalOf(outcome) {
  if (outcome.error !== undefined) return { error: outcome.error };
  const result = outcome.value;
  return {
    is_error: result?.isError === true,
    structured: result?.structuredContent !== undefined && result?.structuredContent !== null,
    text: textOf(result),
  };
}

/**
 * Everything the protocol rules need, asked of `client` (anything with the MCP client's
 * listTools, listResources, readResource, callTool and getServerVersion). `views` are the
 * render tools to call. The read_object loop is PresentView's: a megabyte at the running
 * offset until has_more is false; it also stops on an error, a block without progress or
 * `maxBlocks` calls, and says which.
 */
export async function observeProtocol(client, { views, maxBlocks = 1000 }) {
  const tools = await attempt(() => client.listTools());
  const resources = await attempt(() => client.listResources());
  const observed = {
    server: client.getServerVersion?.() ?? null,
    tools: tools.error !== undefined
      ? { error: tools.error }
      : (tools.value?.tools ?? []).map((tool) => ({
          name: tool.name,
          ui: tool._meta?.ui ?? null,
          input_schema: tool.inputSchema ?? null,
          output_schema: tool.outputSchema ?? null,
        })),
    resources: resources.error !== undefined
      ? { error: resources.error }
      : (resources.value?.resources ?? []).map((resource) => ({ uri: resource.uri, name: resource.name ?? null, mime_type: resource.mimeType ?? null })),
    resource_reads: [],
    render_calls: [],
    present_bindings: [],
    show_calls: [],
    object_reads: [],
    unknown_digest: null,
    unknown_argument: null,
  };

  for (const resource of Array.isArray(observed.resources) ? observed.resources : []) {
    const read = await attempt(() => client.readResource({ uri: resource.uri }));
    const content = read.value?.contents?.[0];
    observed.resource_reads.push({
      uri: resource.uri,
      ...(read.error !== undefined ? { error: read.error } : {}),
      mime_type: content?.mimeType ?? null,
      text_sha256: typeof content?.text === 'string' ? sha256(Buffer.from(content.text, 'utf8')) : null,
      csp: content?._meta?.ui?.csp ?? null,
    });
  }

  for (const view of views) {
    const call = await attempt(() => client.callTool({ name: view.tool, arguments: {} }));
    const structured = call.value?.structuredContent;
    observed.render_calls.push({
      tool: view.tool,
      ...(call.error !== undefined ? { error: call.error } : {}),
      structured: structured !== null && typeof structured === 'object',
      text_fallback: call.value ? textOf(call.value) : null,
    });
    if (Array.isArray(structured?.resolved_bindings)) observed.present_bindings = structured.resolved_bindings;
  }

  // The present view resolves each binding through the app-only show tool
  // (arguments as ui/src/features/views/PresentView.tsx sends them).
  for (const binding of observed.present_bindings) {
    const shown = await attempt(() =>
      client.callTool({
        name: 'show',
        arguments: {
          workspace_id: binding.source.workspace_id,
          item_id: binding.source.item_id,
          at: { kind: 'revision', revision: binding.source.revision },
          view: 'text',
          selection: binding.source.selection,
          max_bytes: 65536,
          max_images: 0,
        },
      }),
    );
    const source = shown.value?.structuredContent?.source;
    observed.show_calls.push({
      binding: binding.name,
      ...(shown.error !== undefined ? { error: shown.error } : {}),
      requested: { item_id: binding.source.item_id, revision: binding.source.revision },
      returned: source ? { item_id: source.item_id ?? null, revision: source.revision ?? null } : null,
      is_error: shown.value?.isError === true,
    });
  }

  // The present view fetches each retained dataset through the app-only read_object tool.
  const retained = observed.present_bindings.filter((binding) => typeof binding.materialized === 'string');
  for (const binding of retained) {
    const request = (offset) => ({ source: binding.source, object: binding.materialized, offset: String(offset), length: 1048576 });
    const chunks = [];
    const blocks = [];
    let offset = 0;
    let stopped = null;
    let error = null;
    while (stopped === null) {
      if (blocks.length >= maxBlocks) {
        stopped = 'block_limit';
        break;
      }
      const read = await attempt(() => client.callTool({ name: 'read_object', arguments: request(offset) }));
      if (read.error !== undefined) {
        stopped = 'error';
        error = read.error;
        break;
      }
      const part = read.value?.structuredContent;
      const bytes = typeof part?.data_base64 === 'string' ? Buffer.from(part.data_base64, 'base64') : null;
      blocks.push({
        requested_offset: String(offset),
        is_error: read.value?.isError === true,
        text_fallback: textOf(read.value).length > 0,
        sha256: part?.sha256 ?? null,
        offset: part?.offset ?? null,
        total_size: part?.total_size ?? null,
        media_type: part?.media_type ?? null,
        bytes: bytes === null ? null : bytes.length,
        has_more: part?.has_more ?? null,
      });
      if (read.value?.isError === true || bytes === null) {
        stopped = 'error';
        error = textOf(read.value) || 'no structuredContent block';
        break;
      }
      chunks.push(bytes);
      offset += bytes.length;
      if (part.has_more !== true) stopped = 'finished';
      else if (bytes.length === 0) stopped = 'no_progress';
    }
    const merged = Buffer.concat(chunks);
    observed.object_reads.push({
      binding: binding.name,
      object: binding.materialized,
      blocks,
      stopped,
      ...(error === null ? {} : { error }),
      assembled_bytes: merged.length,
      assembled_sha256: sha256(merged),
    });
    if (observed.unknown_argument === null) {
      // The request the App sends, plus one key the product request type does not have.
      observed.unknown_argument = refusalOf(
        await attempt(() => client.callTool({ name: 'read_object', arguments: { ...request(0), not_a_product_argument: true } })),
      );
    }
  }

  // A digest nobody retains must come back as a tool error, never as bytes.
  const anySource = observed.present_bindings[0]?.source;
  if (anySource) {
    observed.unknown_digest = refusalOf(
      await attempt(() => client.callTool({ name: 'read_object', arguments: { source: anySource, object: '0'.repeat(64) } })),
    );
  }
  return observed;
}

/** Why a refusal that must be a tool error is not one; empty when it is. */
function refusalProblems(what, refusal) {
  if (refusal === null || refusal === undefined) return [`${what} was not attempted: the present result gave no binding to ask with`];
  if (refusal.error !== undefined) return [`${what}: the call failed instead of returning a tool error: ${refusal.error}`];
  const problems = [];
  if (refusal.is_error !== true) problems.push(`${what} was not answered with isError`);
  if (refusal.structured) problems.push(`${what} was answered with structuredContent`);
  return problems;
}

/**
 * The protocol criteria for `observed` (from observeProtocol) against `expected`:
 * `render_tools` and `app_only_tools` (names), `resource_uri`, `mime_type` (the SDK constant),
 * `bundle_sha256` (from this run's bundle manifest) and `product_read_object` (the product's
 * declaration, with inputSchema and outputSchema). Every criterion is judged: an answer that
 * never came fails the rule that needed it. `record` is the summary the receipt keeps.
 */
export function judgeProtocol(expected, observed) {
  const problems = Object.fromEntries(PROTOCOL_CRITERIA.map((id) => [id, []]));
  const add = (id, problem) => problems[`protocol/${id}`].push(problem);

  const tools = Array.isArray(observed.tools) ? observed.tools : [];
  const renderTools = tools.filter((tool) => typeof tool.ui?.resourceUri === 'string');
  const appOnly = tools.filter((tool) => JSON.stringify(tool.ui?.visibility ?? null) === '["app"]');
  if (!Array.isArray(observed.tools)) {
    for (const id of ['render_tools', 'app_only_tools', 'read_object_declaration']) add(id, `tools/list failed: ${observed.tools?.error ?? 'no answer'}`);
  } else {
    if (!sameList(names(renderTools), [...expected.render_tools].sort())) {
      add('render_tools', `tools/list render tools are ${JSON.stringify(names(renderTools))}, expected ${JSON.stringify([...expected.render_tools].sort())}`);
    }
    for (const tool of renderTools) {
      if (tool.ui.resourceUri !== expected.resource_uri) add('render_tools', `tool ${tool.name} resourceUri is ${tool.ui.resourceUri}, not ${expected.resource_uri}`);
    }
    if (!sameList(names(appOnly), [...expected.app_only_tools].sort())) {
      add('app_only_tools', `tools/list app-only tools are ${JSON.stringify(names(appOnly))}, expected ${JSON.stringify([...expected.app_only_tools].sort())}`);
    }
    if (tools.length !== renderTools.length + appOnly.length) {
      add('app_only_tools', `tools/list has tools that are not exactly one of render tool or app-only: ${JSON.stringify(names(tools))}`);
    }
    const readObject = tools.find((tool) => tool.name === 'read_object');
    if (!readObject) add('read_object_declaration', 'tools/list lists no read_object');
    else {
      for (const [key, product] of [['input_schema', expected.product_read_object?.inputSchema], ['output_schema', expected.product_read_object?.outputSchema]]) {
        if (product === undefined || product === null) add('read_object_declaration', `the product declaration has no ${key}`);
        else if (canonicalJson(readObject[key]) !== canonicalJson(product)) add('read_object_declaration', `the listed read_object ${key} is not the product's`);
      }
    }
  }

  const resources = Array.isArray(observed.resources) ? observed.resources : [];
  if (!Array.isArray(observed.resources)) add('resource_listed', `resources/list failed: ${observed.resources?.error ?? 'no answer'}`);
  else if (resources.length !== 1) add('resource_listed', `resources/list lists ${resources.length} resources, expected the one shared App resource`);
  else if (resources[0].uri !== expected.resource_uri) add('resource_listed', `resources/list uri is ${resources[0].uri}, not ${expected.resource_uri}`);
  const reads = observed.resource_reads ?? [];
  if (reads.length === 0) {
    for (const id of ['resource_mime_type', 'resource_meta_ui', 'resource_is_built_bundle']) add(id, 'no resource was read: resources/list listed none');
  }
  for (const read of reads) {
    if (read.error !== undefined) {
      for (const id of ['resource_mime_type', 'resource_meta_ui', 'resource_is_built_bundle']) add(id, `resources/read ${read.uri} failed: ${read.error}`);
      continue;
    }
    if (read.text_sha256 === null) add('resource_mime_type', `resources/read ${read.uri} returned no text`);
    if (read.mime_type !== expected.mime_type) add('resource_mime_type', `resources/read ${read.uri} mimeType is ${read.mime_type}, not ${expected.mime_type}`);
    const csp = read.csp;
    if (csp === null || typeof csp !== 'object' || Array.isArray(csp)) add('resource_meta_ui', `resources/read ${read.uri} _meta.ui.csp is not an object`);
    else if (!Array.isArray(csp.connectDomains) || !Array.isArray(csp.resourceDomains)) {
      add('resource_meta_ui', `resources/read ${read.uri} _meta.ui.csp lacks a connectDomains or resourceDomains array`);
    }
    if (typeof expected.bundle_sha256 !== 'string') add('resource_is_built_bundle', 'this run has no bundle manifest sha256 to compare with');
    else if (read.text_sha256 !== expected.bundle_sha256) {
      add('resource_is_built_bundle', `resources/read ${read.uri} text hashes to ${read.text_sha256}, the bundle manifest says ${expected.bundle_sha256}`);
    }
  }

  const renderCalls = observed.render_calls ?? [];
  for (const tool of expected.render_tools) {
    const call = renderCalls.find((item) => item.tool === tool);
    if (!call) add('render_tool_results', `${tool} was not called`);
    else if (call.error !== undefined) add('render_tool_results', `${tool} failed: ${call.error}`);
    else {
      if (!call.structured) add('render_tool_results', `${tool} returned no structuredContent`);
      if (typeof call.text_fallback !== 'string' || call.text_fallback.length === 0) add('render_tool_results', `${tool} returned no text fallback`);
    }
  }

  const shows = observed.show_calls ?? [];
  if (shows.length === 0) add('show_bound_sources', 'the present result has no resolved_bindings to show');
  for (const show of shows) {
    if (show.error !== undefined) add('show_bound_sources', `show for binding ${show.binding} failed: ${show.error}`);
    else if (show.is_error) add('show_bound_sources', `show refused binding ${show.binding}`);
    else if (show.returned?.item_id !== show.requested.item_id || show.returned?.revision !== show.requested.revision) {
      add('show_bound_sources', `show did not return the bound source for binding ${show.binding}: ${JSON.stringify(show.returned)}`);
    }
  }

  const objectReads = observed.object_reads ?? [];
  if (objectReads.length === 0) {
    add('read_object_ranged_loop', 'the present result has no materialized binding to read');
    add('read_object_digest', 'no dataset was read');
  }
  for (const read of objectReads) {
    const where = `read_object for binding ${read.binding}`;
    if (read.stopped === 'error') add('read_object_ranged_loop', `${where} failed: ${read.error ?? 'no detail'}`);
    if (read.stopped === 'no_progress') add('read_object_ranged_loop', `${where} made no progress`);
    if (read.stopped === 'block_limit') add('read_object_ranged_loop', `${where} did not finish in ${read.blocks.length} calls`);
    for (const block of read.blocks) {
      if (block.is_error) continue; // reported above as the failure that stopped the loop
      if (!block.text_fallback) add('read_object_ranged_loop', `${where} at ${block.requested_offset} has no text fallback`);
      if (block.sha256 !== read.object || block.offset !== block.requested_offset) {
        add('read_object_ranged_loop', `${where} changed identity or range at ${block.requested_offset}: sha256 ${block.sha256}, offset ${block.offset}`);
      }
    }
    if (read.stopped === 'finished' && read.blocks.length < 2) {
      add('read_object_ranged_loop', `${where} returned the dataset in one block; the ranged loop was not exercised`);
    }
    if (read.assembled_sha256 !== read.object) {
      add('read_object_digest', `${where}: the ${read.assembled_bytes} bytes read hash to ${read.assembled_sha256}, not ${read.object}`);
    }
    const last = read.blocks.at(-1);
    if (last?.total_size !== String(read.assembled_bytes)) {
      add('read_object_digest', `${where}: total_size is ${JSON.stringify(last?.total_size ?? null)}, ${read.assembled_bytes} bytes were read`);
    }
  }

  problems['protocol/read_object_unknown_digest_refused'].push(...refusalProblems('an unknown digest', observed.unknown_digest));
  problems['protocol/read_object_unknown_argument_refused'].push(...refusalProblems('an unknown argument', observed.unknown_argument));

  const criteria = PROTOCOL_CRITERIA.map((id) =>
    problems[id].length
      ? { id, required: true, result: 'fail', detail: problems[id].join('; ') }
      : { id, required: true, result: 'pass' },
  );
  const schemaSha = (schema) => (schema === null || schema === undefined ? null : sha256(canonicalJson(schema)));
  return {
    criteria,
    record: {
      server: observed.server,
      tools: Array.isArray(observed.tools)
        ? observed.tools.map((tool) => ({ name: tool.name, ui: tool.ui, input_schema_sha256: schemaSha(tool.input_schema), output_schema_sha256: schemaSha(tool.output_schema) }))
        : observed.tools,
      product_read_object: {
        input_schema_sha256: schemaSha(expected.product_read_object?.inputSchema),
        output_schema_sha256: schemaSha(expected.product_read_object?.outputSchema),
      },
      resources: observed.resources,
      resource_reads: observed.resource_reads,
      render_calls: observed.render_calls,
      show_calls: observed.show_calls,
      object_reads: objectReads.map((read) => ({ ...read, blocks: read.blocks.length, block_offsets: read.blocks.map((block) => block.offset) })),
      unknown_digest: observed.unknown_digest,
      unknown_argument: observed.unknown_argument,
    },
  };
}
