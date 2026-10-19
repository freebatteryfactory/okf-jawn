/**
 * The one parser of a View file in the browser, for showing and reviewing a View the server
 * stores (the Explorer's View and proposal pages read it).
 *
 * The file is the contract's (`ItemKind::View`): an OKF concept whose `title` property is the
 * View's title and whose body is the View's description as Markdown, then one fenced block whose
 * opening line is exactly ```` ```json okf-jawn-view ```` and which ends the body. The block is a
 * JSON object holding the rest of the document: `schema_version`, `mode`, `grammar`, `bindings`,
 * `spec` and `charts` (optional). A binding there names no `workspace_id` (the caller's workspace
 * is filled in), no `locations` and no `materialized`; the three are refused by name.
 *
 * Core is the authority, and this mirrors its reader (`crates/core/src/views/layout.rs`, core
 * M1.6, PR #48 at 8ce63d81): the same line rule, the same order of checks, the same JSON Pointers.
 * Core reads the file again whenever it matters (a proposal is accepted, a View is resolved), so a
 * failure here is shown in words and never decides anything: on a proposal page Approve stays
 * offered. The assembled document is checked with the generated `zViewDocument`.
 *
 * Limits, where core refuses what this reads:
 * - core also refuses a body on which pulldown-cmark or okf-core would not read the same block (a
 *   description that leaves a fence or an HTML block open, an indented code block holding a fence
 *   line); no Markdown parser of those rules is a dependency here, so that check is not made;
 * - a field is read with the generated schema, which is looser than core's types in places: a
 *   path segment `..` passes `zWorkspacePath`, and core refuses it;
 * - the wording of a refusal is this parser's own; its kind and pointer are core's.
 */
import type { z } from 'zod';
import { zSourceReference, zViewBinding, zViewDocument } from '../../api/generated/zod.gen';

type ViewDocument = z.infer<typeof zViewDocument>;

/** Why a file is not a View document of this build. */
export type ViewFileProblem =
  | 'no_block'
  | 'two_blocks'
  | 'not_closed'
  | 'not_last'
  | 'not_json'
  | 'repeated_key'
  | 'version'
  | 'schema';

/** A View file read: the document, or why not, at a JSON Pointer into the document when one applies. */
export type ViewFileResult =
  | { ok: true; document: ViewDocument }
  | { ok: false; problem: ViewFileProblem; message: string; pointer: string | null };

/** The file of one stored item: its properties and its body, as `get_item` answers them. */
export interface ViewFile {
  properties: Readonly<Record<string, unknown>>;
  body: string;
}

/** The info string of a View's block. */
export const VIEW_BLOCK_INFO = 'json okf-jawn-view';
/** The format version of the View documents this build reads. */
export const VIEW_SCHEMA_VERSION = 1;
const OPEN = `\`\`\`${VIEW_BLOCK_INFO}`;
const CLOSE = '```';
const DOCUMENT_FIELDS = ['schema_version', 'mode', 'grammar', 'bindings', 'spec', 'charts'];
const BINDING_FIELDS = ['name', 'source', 'units', 'transforms'];
const SOURCE_FIELDS = ['item_id', 'path', 'revision', 'digest', 'selection'];

/** A refusal, thrown inside the parser and answered as its result. */
class Refused extends Error {
  constructor(
    readonly problem: ViewFileProblem,
    readonly pointer: string,
    message: string,
  ) {
    super(message);
  }
}
type JsonObject = Record<string, unknown>;

/** The View a file holds, every binding in `workspaceId`. */
export function parseViewFile(file: ViewFile, workspaceId: string): ViewFileResult {
  try {
    return { ok: true, document: viewFrom(file, workspaceId) };
  } catch (cause) {
    if (!(cause instanceof Refused)) throw cause;
    return {
      ok: false,
      problem: cause.problem,
      message: cause.message,
      pointer: cause.pointer === '' ? null : cause.pointer,
    };
  }
}

function viewFrom({ properties, body }: ViewFile, workspaceId: string): ViewDocument {
  const title = properties.title;
  if (typeof title !== 'string')
    throw new Refused('schema', '/title', "a View's title is its `title` property, a string");
  const { description, block } = split(body);
  const object = parseUnique(block);
  const version = field(object, 'schema_version', '', zViewDocument.shape.schema_version);
  if (version !== VIEW_SCHEMA_VERSION)
    throw new Refused(
      'version',
      '/schema_version',
      `this build reads and writes View documents of version ${VIEW_SCHEMA_VERSION}; this one is version ${version}`,
    );
  refuseUnknown(object, DOCUMENT_FIELDS, '', 'a View document');
  const mode = field(object, 'mode', '', zViewDocument.shape.mode);
  const grammar = field(object, 'grammar', '', zViewDocument.shape.grammar);
  const spec = field(object, 'spec', '', zViewDocument.shape.spec);
  const charts =
    object.charts === undefined ? {} : field(object, 'charts', '', zViewDocument.shape.charts);
  if (!Array.isArray(object.bindings))
    throw new Refused('schema', '/bindings', '`bindings` is a list of bindings');
  const bindings = object.bindings.map((stored, index) =>
    binding(stored, `/bindings/${index}`, workspaceId),
  );
  const assembled = {
    schema_version: version,
    title,
    description,
    mode,
    grammar,
    bindings,
    spec,
    charts,
  };
  const checked = zViewDocument.safeParse(assembled);
  if (!checked.success) throw schemaRefusal('', checked.error);
  return checked.data;
}

/**
 * The description and the block text of a body. Lines are split as Rust's `str::lines` splits
 * them: at `\n`, with one `\r` before it dropped.
 */
function split(body: string): { description: string; block: string } {
  const lines = body
    .split('\n')
    .map((line, index, all) =>
      index < all.length - 1 && line.endsWith('\r') ? line.slice(0, -1) : line,
    );
  if (lines.at(-1) === '') lines.pop();
  const opens = lines.flatMap((line, index) => (line === OPEN ? [index] : []));
  const [open, ...others] = opens;
  if (open === undefined)
    throw new Refused(
      'no_block',
      '',
      "a View's body ends with one ```json okf-jawn-view block, which holds its document",
    );
  if (others.length > 0)
    throw new Refused(
      'two_blocks',
      '',
      `the body holds ${opens.length} okf-jawn-view blocks; a View has one`,
    );
  const close = lines.findIndex((line, index) => index > open && line === CLOSE);
  if (close < 0)
    throw new Refused('not_closed', '', 'the okf-jawn-view block is not closed by a line ```');
  if (lines.slice(close + 1).some((line) => line.trim() !== ''))
    throw new Refused('not_last', '', 'the okf-jawn-view block must end the body');
  const prose = lines.slice(0, open);
  const shown = (line: string) => line.trim() !== '';
  const start = prose.findIndex(shown);
  const end = prose.length - 1 - [...prose].reverse().findIndex(shown);
  return {
    description: start < 0 ? '' : prose.slice(start, end + 1).join('\n'),
    block: lines.slice(open + 1, close).join('\n'),
  };
}

/** The block's JSON object, refused where an object repeats a key (JSON.parse keeps the last). */
function parseUnique(text: string): JsonObject {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (cause) {
    throw new Refused(
      'not_json',
      '',
      `the View block is not JSON: ${cause instanceof Error ? cause.message : String(cause)}`,
    );
  }
  const repeated = repeatedKey(text);
  if (repeated !== undefined)
    throw new Refused(
      'repeated_key',
      repeated.at,
      `the key \`${repeated.key}\` is repeated in one object`,
    );
  if (!isObject(value)) throw new Refused('schema', '', 'the View block holds one JSON object');
  return value;
}

/**
 * The first object of a JSON text that holds a key twice: its pointer and the key. The text is
 * already known to be JSON, so only the structure is walked: strings (a key is decoded by
 * JSON.parse), brackets, braces, colons and commas.
 */
function repeatedKey(text: string): { at: string; key: string } | undefined {
  let at = 0;
  const space = () => {
    while (at < text.length && ' \t\n\r'.includes(text.charAt(at))) at += 1;
  };
  const string = () => {
    const start = at;
    at += 1;
    while (text.charAt(at) !== '"') at += text.charAt(at) === '\\' ? 2 : 1;
    at += 1;
    return text.slice(start, at);
  };
  const value = (pointer: string): { at: string; key: string } | undefined => {
    space();
    const first = text.charAt(at);
    if (first === '{') {
      at += 1;
      const keys = new Set<string>();
      space();
      if (text.charAt(at) === '}') {
        at += 1;
        return undefined;
      }
      for (;;) {
        space();
        const key: unknown = JSON.parse(string());
        if (typeof key !== 'string') return undefined;
        if (keys.has(key)) return { at: pointer, key };
        keys.add(key);
        space();
        at += 1; // the colon
        const inner = value(`${pointer}/${token(key)}`);
        if (inner !== undefined) return inner;
        space();
        const next = text.charAt(at);
        at += 1;
        if (next === '}') return undefined;
      }
    }
    if (first === '[') {
      at += 1;
      space();
      if (text.charAt(at) === ']') {
        at += 1;
        return undefined;
      }
      for (let index = 0; ; index += 1) {
        const inner = value(`${pointer}/${index}`);
        if (inner !== undefined) return inner;
        space();
        const next = text.charAt(at);
        at += 1;
        if (next === ']') return undefined;
      }
    }
    if (first === '"') {
      string();
      return undefined;
    }
    while (at < text.length && !' \t\n\r,]}'.includes(text.charAt(at))) at += 1;
    return undefined;
  };
  return value('');
}

/** One binding of the block, read into the wire's `ViewBinding` in `workspaceId`. */
function binding(stored: unknown, at: string, workspaceId: string) {
  if (!isObject(stored)) throw new Refused('schema', at, 'a binding is an object');
  if (Object.hasOwn(stored, 'materialized'))
    throw new Refused(
      'schema',
      `${at}/materialized`,
      "a binding's dataset is produced when the View is resolved; a saved View keeps none",
    );
  refuseUnknown(stored, BINDING_FIELDS, at, 'a binding');
  const sourceAt = `${at}/source`;
  const source = stored.source;
  if (!isObject(source))
    throw new Refused('schema', sourceAt, "a binding's `source` is its citation, an object");
  if (Object.hasOwn(source, 'workspace_id'))
    throw new Refused(
      'schema',
      `${sourceAt}/workspace_id`,
      'a saved View binds only to its own workspace and does not name it',
    );
  if (Object.hasOwn(source, 'locations'))
    throw new Refused(
      'schema',
      `${sourceAt}/locations`,
      'locations are derived from the citation; a saved View keeps none',
    );
  refuseUnknown(source, SOURCE_FIELDS, sourceAt, "a binding's source");
  const shape = zSourceReference.shape;
  const item_id = field(source, 'item_id', sourceAt, shape.item_id);
  const path = field(source, 'path', sourceAt, shape.path);
  const revision = field(source, 'revision', sourceAt, shape.revision);
  const digest =
    source.digest === undefined ? undefined : field(source, 'digest', sourceAt, shape.digest);
  const selection = field(source, 'selection', sourceAt, shape.selection);
  return {
    name: field(stored, 'name', at, zViewBinding.shape.name),
    source: {
      workspace_id: workspaceId,
      item_id,
      path,
      revision,
      ...(digest === undefined ? {} : { digest }),
      selection,
    },
    units: field(stored, 'units', at, zViewBinding.shape.units),
    transforms: field(stored, 'transforms', at, zViewBinding.shape.transforms),
  };
}

/** Refuse the first key of `object` (in key order) that `known` does not hold. */
function refuseUnknown(object: JsonObject, known: readonly string[], at: string, what: string) {
  const unknown = Object.keys(object)
    .sort()
    .find((key) => !known.includes(key));
  if (unknown !== undefined)
    throw new Refused(
      'schema',
      `${at}/${token(unknown)}`,
      `\`${unknown}\` is not a field of ${what}`,
    );
}

/** One field of an object, read with its generated schema; refused at its pointer when missing or mistyped. */
function field<T extends z.ZodType>(
  object: JsonObject,
  key: string,
  at: string,
  schema: T,
): z.infer<T> {
  const pointer = `${at}/${token(key)}`;
  if (object[key] === undefined) throw new Refused('schema', pointer, `\`${key}\` is missing`);
  const read = schema.safeParse(object[key]);
  if (!read.success) throw schemaRefusal(pointer, read.error);
  return read.data;
}

/** A schema's first issue, at its pointer under `at`. */
function schemaRefusal(at: string, error: z.ZodError): Refused {
  const issue = error.issues.at(0);
  const path = (issue?.path ?? []).map((part) => `/${token(String(part))}`).join('');
  return new Refused('schema', `${at}${path}`, issue?.message ?? 'the View document is malformed');
}

/** One JSON Pointer reference token (RFC 6901): `~` as `~0`, `/` as `~1`. */
function token(key: string): string {
  return key.replaceAll('~', '~0').replaceAll('/', '~1');
}

function isObject(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
