/**
 * What a View reads, whichever host shows it: one cited source, and one block of a retained
 * object. A host gives the views components a `ViewData` (the MCP App over its server tools in
 * `mcp-apps/view-data.ts`; the Explorer over the generated client); no views component calls a
 * host itself, and none chooses a revision: a source is read at the revision its binding cites.
 *
 * A provider is transport only. What a View may trust is checked here, the same whatever the
 * provider: an answer is read with the generated response schema, a source must come back at the
 * revision its binding cites, and a dataset is read in blocks of `DATASET_BLOCK_BYTES`, never past
 * `DATASET_BOUND_BYTES`, and only when its bytes hash to the digest the binding names. A provider
 * that cannot read rejects; its error's message is shown as the read's refusal.
 *
 * Requests are the generated wire types, so a provider hands them on unchanged; an answer is typed
 * as what the generated schema reads, which both the generated client's data and a parsed tool
 * result satisfy.
 */
import type { z } from 'zod';
import type {
  GetObjectRequest,
  ReadItemRequest,
  SourceReference,
} from '../../api/generated/types.gen';
import {
  zGetObjectResponse,
  zReadItemResponse,
  type zViewBinding,
} from '../../api/generated/zod.gen';

/** The two reads a View makes. */
export interface ViewData {
  /** Read one cited source at the revision its binding pins, with its selection (`read_item`). */
  readItem(request: ReadItemRequest): Promise<z.input<typeof zReadItemResponse>>;
  /** Read one block of a retained object of a cited source (`get_object`). */
  getObject(request: GetObjectRequest): Promise<z.input<typeof zGetObjectResponse>>;
}

type Binding = z.infer<typeof zViewBinding>;

/** The most bytes one block of a dataset is asked for. */
export const DATASET_BLOCK_BYTES = 1_048_576;
/** The most bytes of one dataset a View reads to show it inline. */
export const DATASET_BOUND_BYTES = 4_194_304;
/** The most bytes of a source's text read for its excerpt. */
const EXCERPT_BYTES = 65_536;

/**
 * A binding's citation as the generated wire type spells it. A parsed citation's type allows an
 * absent optional to read `undefined`, which the wire type does not; a parsed value never holds
 * one, so nothing a server sent is left out here.
 */
function citation(source: Binding['source']): SourceReference {
  const { digest, locations, ...required } = source;
  return {
    ...required,
    ...(digest === undefined ? {} : { digest }),
    ...(locations === undefined ? {} : { locations }),
  };
}

/** The source a binding cites, read at the revision it cites, with its selection. */
export async function readSource(data: ViewData, binding: Binding) {
  const source = zReadItemResponse.parse(
    await data.readItem({
      workspace_id: binding.source.workspace_id,
      item_id: binding.source.item_id,
      at: { kind: 'revision', revision: binding.source.revision },
      view: 'text',
      selection: binding.source.selection,
      max_bytes: EXCERPT_BYTES,
      max_images: 0,
    }),
  );
  if (source.source.revision !== binding.source.revision)
    throw new Error('Returned source revision differs from the binding');
  return source;
}

/**
 * The text of the dataset a binding retains, or undefined when it retains none. Each block must
 * be of the object asked for and start where the previous one ended; the whole is refused past
 * the bound and unless its SHA-256 is the binding's digest.
 */
export async function readDataset(data: ViewData, binding: Binding): Promise<string | undefined> {
  const object = binding.materialized;
  if (!object) return undefined;
  const chunks: Uint8Array[] = [];
  let offset = 0n;
  let more = true;
  while (more) {
    const part = zGetObjectResponse.parse(
      await data.getObject({
        source: citation(binding.source),
        object,
        offset: offset.toString(),
        length: DATASET_BLOCK_BYTES,
      }),
    );
    if (part.sha256 !== object || BigInt(part.offset) !== offset)
      throw new Error('Dataset identity or range changed');
    const bytes = Uint8Array.from(atob(part.data_base64), (character) => character.charCodeAt(0));
    offset += BigInt(bytes.byteLength);
    if (offset > BigInt(DATASET_BOUND_BYTES))
      throw new Error('Dataset exceeds the inline display budget');
    if (bytes.length === 0 && part.has_more) throw new Error('Dataset read made no progress');
    chunks.push(bytes);
    more = part.has_more;
  }
  const merged = new Uint8Array(Number(offset));
  let cursor = 0;
  for (const chunk of chunks) {
    merged.set(chunk, cursor);
    cursor += chunk.byteLength;
  }
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', merged));
  const hex = Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  if (hex !== object) throw new Error('Dataset digest verification failed');
  return new TextDecoder('utf-8', { fatal: true }).decode(merged);
}
