/** Port signatures name resolved core types only; a dependency-free check that needs no Rust toolchain. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const source = join(root, 'crates/core/src');

/** Every file that declares a port trait, and whether its signatures may name `Principal`. */
const portFiles = new Map([
  ['access.rs', { principal: true }],
  ['confirmations.rs', { principal: false }],
  ['conversion.rs', { principal: false }],
  ['credentials.rs', { principal: false }],
  ['drafts.rs', { principal: false }],
  ['events.rs', { principal: false }],
  ['jobs.rs', { principal: false }],
  ['mutations.rs', { principal: false }],
  ['proposals.rs', { principal: false }],
  ['readiness.rs', { principal: false }],
  ['sandbox.rs', { principal: false }],
  ['search.rs', { principal: false }],
  ['storage.rs', { principal: false }],
  ['uploads.rs', { principal: false }],
]);
/** `ports.rs` declares `Application` by macro; its methods take the wire request by design. */
const exempt = new Set(['ports.rs']);

const rules = [
  { name: 'the `At` selector', pattern: /\bAt\b/ },
  { name: 'a wire `*Request` type', pattern: /\b\w+Request\b/ },
];
const principalRule = { name: '`Principal`', pattern: /\bPrincipal\b/ };

function stripComments(rust) {
  return rust.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
}

/** Every `fn` signature declared inside a `pub trait` block. */
function traitSignatures(rust) {
  const code = stripComments(rust);
  const found = [];
  const header = /pub\s+trait\s+(\w+)[^{;]*\{/g;
  for (let match = header.exec(code); match; match = header.exec(code)) {
    let depth = 1;
    let end = header.lastIndex;
    while (end < code.length && depth > 0) {
      if (code[end] === '{') depth += 1;
      else if (code[end] === '}') depth -= 1;
      end += 1;
    }
    const body = code.slice(header.lastIndex, end - 1);
    const signature = /\bfn\s+(\w+)[^;{]*/g;
    for (let fn = signature.exec(body); fn; fn = signature.exec(body)) {
      found.push({ trait: match[1], name: fn[1], text: fn[0].replace(/\s+/g, ' ').trim() });
    }
    header.lastIndex = end;
  }
  return found;
}

function violations(file, rust, allowPrincipal) {
  const active = allowPrincipal ? rules : [...rules, principalRule];
  const out = [];
  for (const signature of traitSignatures(rust)) {
    for (const rule of active) {
      if (rule.pattern.test(signature.text)) out.push(`${file}: ${signature.trait}::${signature.name} mentions ${rule.name}`);
    }
  }
  return out;
}

async function rustFiles(directory, prefix = '') {
  const out = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...await rustFiles(join(directory, entry.name), relative));
    else if (entry.name.endsWith('.rs')) out.push(relative);
  }
  return out.sort();
}

/** A creating store returns the prior row for a repeated MutationId; a lookup by mutation is a second mechanism. */
function lookupsByMutation(file, rust) {
  return traitSignatures(rust)
    .filter(signature => /^find_(?:\w+_)?by_mutation$|^find_mutation$/.test(signature.name))
    .map(signature => `${file}: ${signature.trait}::${signature.name}`);
}

/** Every method that creates a durable row, or moves one to a new state, takes the write identity. */
const writes = new Map([
  ['confirmations.rs', [['ConfirmationStore', 'create', 'MutationId'], ['ConfirmationStore', 'consume', 'MutationId']]],
  ['credentials.rs', [['CredentialStore', 'create_connector', 'MutationId']]],
  ['drafts.rs', [['DraftStore', 'save', 'MutationId'], ['DraftStore', 'discard', 'MutationId']]],
  ['jobs.rs', [
    ['RecordStore', 'create_job', 'NewJob'],
    ['RecordStore', 'retry_job', 'MutationId'],
    ['RecordStore', 'cancel_job', 'MutationId'],
    ['RecordStore', 'record_artifact', 'MutationId'],
    ['RecordStore', 'insert_review', 'MutationId'],
  ]],
  ['proposals.rs', [['ProposalStore', 'insert', 'MutationId'], ['ProposalStore', 'add_comment', 'MutationId']]],
  ['storage.rs', [
    ['VersionStore', 'commit', 'CommitChanges'],
    ['VersionStore', 'create_candidate', 'CandidateChanges'],
    ['VersionStore', 'promote_candidate', 'Promotion'],
    ['WorkspaceCatalog', 'create', 'MutationId'],
    ['WorkspaceCatalog', 'update', 'MutationId'],
    ['WorkspaceCatalog', 'archive', 'MutationId'],
  ]],
  ['uploads.rs', [['UploadStore', 'create', 'MutationId']]],
]);

function writesWithoutIdentity(file, rust, expected) {
  const signatures = traitSignatures(rust);
  const out = [];
  for (const [trait, name, token] of expected) {
    const signature = signatures.find(candidate => candidate.trait === trait && candidate.name === name);
    if (!signature) out.push(`${file}: ${trait}::${name} is missing`);
    else if (!signature.text.split(/[^A-Za-z0-9_]+/).includes(token)) out.push(`${file}: ${trait}::${name} does not take ${token}`);
  }
  return out;
}

const sample = `
/// Docs may say At, Principal and SearchRequest freely.
pub trait Sample: Send + Sync {
    /// Resolve At once.
    fn resolve<'a>(&'a self, at: &'a At) -> PortFuture<'a, Revision>;
    fn list<'a>(&'a self, principal: &'a Principal) -> PortFuture<'a, Vec<Workspace>>;
    fn log<'a>(
        &'a self,
        request: LogRequest,
    ) -> PortFuture<'a, LogResponse>;
    fn head<'a>(&'a self, scope: &'a StorageScope, created_at: String) -> PortFuture<'a, Revision>;
    fn find_by_mutation<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<Row>>;
    fn find_comment_by_mutation<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<Row>>;
    fn find_commit<'a>(&'a self, mutation_id: MutationId) -> PortFuture<'a, Option<Revision>>;
}
pub struct Record { pub principal: Principal, pub at: At }
pub fn free(request: LogRequest) {}
`;

test('a trait signature naming At, Principal or a request type is reported; docs, fields and free functions are not', () => {
  assert.deepEqual(violations('sample.rs', sample, false), [
    'sample.rs: Sample::resolve mentions the `At` selector',
    'sample.rs: Sample::list mentions `Principal`',
    'sample.rs: Sample::log mentions a wire `*Request` type',
  ]);
});

test('Principal is tolerated only in a file that says so', () => {
  assert.deepEqual(violations('access.rs', sample, true), [
    'access.rs: Sample::resolve mentions the `At` selector',
    'access.rs: Sample::log mentions a wire `*Request` type',
  ]);
});

test('a lookup by mutation is reported by name; other finders are not', () => {
  assert.deepEqual(lookupsByMutation('sample.rs', sample), [
    'sample.rs: Sample::find_by_mutation',
    'sample.rs: Sample::find_comment_by_mutation',
  ]);
});

test('every core file that declares a pub trait is covered by this guard', async () => {
  const present = await rustFiles(source);
  const uncovered = [];
  for (const file of present) {
    const declares = /pub\s+trait\s+\w+/.test(stripComments(await readFile(join(source, file), 'utf8')));
    if (declares && !portFiles.has(file) && !exempt.has(file)) uncovered.push(file);
  }
  assert.deepEqual(uncovered, []);
  for (const file of portFiles.keys()) assert.ok(present.includes(file), `${file} is listed but missing`);
});

test('no port signature mentions At, Principal or a wire request type', async () => {
  const found = [];
  for (const [file, { principal }] of portFiles) {
    found.push(...violations(file, await readFile(join(source, file), 'utf8'), principal));
  }
  assert.deepEqual(found, []);
});

test('no store keeps a lookup by mutation beside its idempotent insert', async () => {
  const found = [];
  for (const file of portFiles.keys()) {
    found.push(...lookupsByMutation(file, await readFile(join(source, file), 'utf8')));
  }
  assert.deepEqual(found, []);
});

test('a write that lacks the write identity, or is missing, is reported', () => {
  const expected = [['Sample', 'find_commit', 'MutationId'], ['Sample', 'head', 'MutationId'], ['Sample', 'record', 'MutationId']];
  assert.deepEqual(writesWithoutIdentity('sample.rs', sample, expected), [
    'sample.rs: Sample::head does not take MutationId',
    'sample.rs: Sample::record is missing',
  ]);
});

test('every method that writes a durable row takes the write identity', async () => {
  const found = [];
  for (const [file, expected] of writes) {
    found.push(...writesWithoutIdentity(file, await readFile(join(source, file), 'utf8'), expected));
  }
  assert.deepEqual(found, []);
});
