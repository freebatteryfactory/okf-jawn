/**
 * Receipt-backed gate statuses are derived, never typed.
 *
 * verification.json names, for each Phase 0 gate of kind `receipt`, the committed receipt that
 * closes it. `derivedRecord` computes each such gate's status and `phase_0_qualified` from those
 * receipts alone; `writeDerivedRecord` (run by `bun qualification/record.mjs`) is the only writer
 * of the two values, and `checkReceipts` fails when a typed value differs from the derived one.
 * A committed receipt must also describe HEAD: its commit an ancestor, its inputs unchanged.
 */
import { readdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { run } from './process.mjs';
import { exists } from './files.mjs';
import { envelopeFailures, receiptResults, statusOf } from './receipt-envelope.mjs';

/** The kinds a Phase 0 gate can have. A gate with none of them is still hand-typed. */
export const gateKinds = Object.freeze(['receipt', 'ci', 'decision']);
/** The command that rewrites the derived values; every message about a typed value names it. */
export const rewriteCommand = 'bun qualification/record.mjs';

const receiptsPath = 'qualification/receipts';
const recordPath = 'verification.json';
/** Keeps the otherwise empty directory tracked; it is not a receipt. */
const placeholder = '.gitkeep';

/** Reads of one tree: the working tree when `head` is undefined, otherwise that commit, whatever is checked out. */
function treeAt(root, head) {
  if (head === undefined) {
    const local = path => join(root, ...path.split('/'));
    return {
      at: '',
      async read(path) {
        try { return await readFile(local(path), 'utf8'); }
        catch (error) { if (error.code === 'ENOENT') return null; throw error; }
      },
      async list(directory) { return await exists(local(directory)) ? (await readdir(local(directory))).sort() : []; },
    };
  }
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  return {
    at: ` at ${head}`,
    async read(path) {
      const shown = await git(['show', `${head}:${path}`]);
      return shown.code === 0 ? shown.stdout : null;
    },
    async list(directory) {
      const listed = await git(['ls-tree', '-z', '--name-only', head, `${directory}/`]);
      if (listed.code !== 0) throw new Error(`check-receipts: could not list ${directory}/ at ${head} (git exit ${listed.code})`);
      return listed.stdout.split('\0').filter(Boolean).map(path => path.slice(directory.length + 1)).sort();
    },
  };
}

/** `text` as JSON, or the reason it is not. */
function parsed(text) {
  try { return { value: JSON.parse(text.replace(/^\uFEFF/, '')) }; }
  catch (error) { return { error: `not valid JSON (${error.message})` }; }
}

/** The file name under qualification/receipts/ a receipt gate names, or null when it names anything else. */
function receiptFile(gate) {
  if (typeof gate.receipt !== 'string' || !gate.receipt.startsWith(`${receiptsPath}/`)) return null;
  const file = gate.receipt.slice(receiptsPath.length + 1);
  return file.length === 0 || file.includes('/') ? null : file;
}

/**
 * The criterion ids a gate pins, from its criteria file in `tree`
 * (`{ "gate": "<id>", "required": ["<criterion id>", ...] }`). When the file cannot supply them,
 * `problems` says why and `pinned` is empty, so the rest of the envelope is still judged.
 */
async function pinnedCriteria(tree, gate) {
  const unusable = message => ({ pinned: [], problems: [message] });
  if (typeof gate.criteria !== 'string' || gate.criteria.length === 0) return unusable(`gate ${gate.id} names no criteria file`);
  const text = await tree.read(gate.criteria);
  if (text === null) return unusable(`its criteria file ${gate.criteria} is missing${tree.at}, so nothing pins what this receipt must have judged`);
  const criteria = parsed(text);
  if (criteria.error) return unusable(`its criteria file ${gate.criteria} is ${criteria.error}`);
  const { value } = criteria;
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return unusable(`its criteria file ${gate.criteria} must be an object with "gate" and "required"`);
  if (value.gate !== gate.id) return unusable(`its criteria file ${gate.criteria} is for gate ${JSON.stringify(value.gate)}, not ${gate.id}`);
  const required = value.required;
  if (!Array.isArray(required) || required.length === 0 || !required.every(id => typeof id === 'string' && id.length > 0) || new Set(required).size !== required.length) {
    return unusable(`its criteria file ${gate.criteria} must list each required criterion id once in a non-empty "required" array`);
  }
  return { pinned: required, problems: [] };
}

/** Why a receipt's `not_judged` is not the list its criteria derive; empty when it is, or when the criteria are themselves malformed. */
function notJudgedFailures(receipt) {
  if (receipt === null || typeof receipt !== 'object' || !Array.isArray(receipt.criteria) || receipt.criteria.some(entry => entry === null || typeof entry !== 'object')) return [];
  const derived = receipt.criteria.filter(entry => entry.result === 'not_judged' || entry.result === 'not_applicable').map(entry => entry.id);
  const typed = receipt.not_judged;
  const same = Array.isArray(typed) && typed.length === derived.length && derived.every(id => typed.includes(id));
  return same ? [] : [`not_judged is ${JSON.stringify(typed)} but its criteria derive ${JSON.stringify(derived)}`];
}

/**
 * What the receipts in a tree support, and where verification.json there types something else.
 *
 * `head` is a commit, read with git and independent of the checkout; without it the working tree
 * is read. A receipt gate is `incomplete` when its receipt file does not exist, otherwise the
 * status its receipt's result supports. `phase_0_qualified` is true only when every receipt gate
 * is `passed`, no receipt has a failure and no Phase 0 gate is still hand-typed (has none of
 * `gateKinds`).
 *
 * A receipt that exists is read, not only its header: `envelopeFailures` must be empty against
 * the `required` list of the gate's criteria file in the same tree (a missing criteria file is
 * a failure), its `not_judged` must be the list its criteria derive, and every file under
 * qualification/receipts/ must be the one receipt of one gate.
 *
 * Returns `{ missing, gates, receipts, unconverted, phase_0_qualified, pending, failures, mismatches }`:
 * `gates` is one `{ id, harness, file, status, typed, basis }` per receipt gate; `receipts` is
 * every file under qualification/receipts/ as `{ name, receipt }` (`receipt` null when it is not
 * JSON); `pending` says why `phase_0_qualified` is false; `failures` are `{ name, message }`
 * reasons a receipt or the record cannot be trusted; `mismatches` are the typed values that
 * differ from the derived ones, each naming `rewriteCommand`.
 */
export async function derivedRecord(root, head) {
  const tree = treeAt(root, head);
  const names = (await tree.list(receiptsPath)).filter(name => name !== placeholder);
  const receipts = [];
  const failures = [];
  for (const name of names) {
    const text = await tree.read(`${receiptsPath}/${name}`);
    const outcome = text === null ? { error: `could not be read${tree.at}` } : parsed(text);
    if (outcome.error) failures.push({ name, message: outcome.error });
    receipts.push({ name, receipt: outcome.error ? null : outcome.value });
  }
  const result = { missing: false, gates: [], receipts, unconverted: [], phase_0_qualified: false, pending: [], failures, mismatches: [] };
  const recordText = await tree.read(recordPath);
  if (recordText === null) {
    for (const { name } of receipts) failures.push({ name, message: `${recordPath} is missing${tree.at}, so no gate names this receipt` });
    return { ...result, missing: true, pending: [`${recordPath} is missing${tree.at}`] };
  }
  const record = parsed(recordText);
  if (record.error) {
    failures.push({ name: recordPath, message: record.error });
    return { ...result, pending: [`${recordPath} is ${record.error}`] };
  }
  const phase0 = record.value?.current?.gates?.phase_0;
  if (!Array.isArray(phase0)) {
    failures.push({ name: recordPath, message: 'current.gates.phase_0 must be an array of gates' });
    return { ...result, pending: [`${recordPath} has no Phase 0 gates`] };
  }
  for (const gate of phase0) {
    if (!gateKinds.includes(gate?.kind)) { result.unconverted.push(String(gate?.id)); continue; }
    if (gate.kind !== 'receipt') continue;
    const file = receiptFile(gate);
    const entry = { id: gate.id, harness: gate.harness, file, status: 'incomplete', typed: gate.status, basis: `${gate.receipt} is absent` };
    result.gates.push(entry);
    if (file === null) {
      entry.basis = `its receipt (${JSON.stringify(gate.receipt)}) is not a file under ${receiptsPath}/`;
      failures.push({ name: gate.id, message: entry.basis });
      continue;
    }
    const found = receipts.find(candidate => candidate.name === file);
    if (!found) continue;
    if (found.receipt === null) { entry.basis = `${gate.receipt} is not valid JSON`; continue; }
    // statusOf is asked only about a known result: a result named like an inherited object
    // property would otherwise come back as something that is not a status.
    const known = receiptResults.includes(found.receipt?.result);
    entry.status = known ? statusOf(found.receipt.result) : 'incomplete';
    entry.basis = known ? `${gate.receipt} result ${found.receipt.result}` : `${gate.receipt} has no result of ${receiptResults.join(', ')}`;
    // The receipt is read, not only its header: its result must be the fold of its criteria,
    // and every criterion its harness pins must be present and required.
    const { pinned, problems } = await pinnedCriteria(tree, gate);
    for (const message of [...problems, ...envelopeFailures(found.receipt, gate.id, pinned), ...notJudgedFailures(found.receipt)]) failures.push({ name: file, message });
  }
  // A receipt is recorded only for the gate it closes: one file, one gate.
  const named = result.gates.map(gate => gate.file).filter(file => file !== null);
  for (const gate of result.gates) {
    if (gate.file !== null && named.indexOf(gate.file) !== named.lastIndexOf(gate.file)) failures.push({ name: gate.id, message: `its receipt ${receiptsPath}/${gate.file} is named by more than one gate` });
  }
  for (const { name } of receipts) {
    if (!named.includes(name)) failures.push({ name, message: `no Phase 0 gate of kind receipt names this file under ${receiptsPath}/` });
  }
  for (const gate of result.gates) if (gate.status !== 'passed') result.pending.push(`${gate.id} is ${gate.status}`);
  for (const id of result.unconverted) result.pending.push(`${id} has no kind and is still hand-typed`);
  if (result.gates.length === 0) result.pending.push('no Phase 0 gate is backed by a receipt');
  if (failures.length) result.pending.push(`${failures.length} receipt failure(s)`);
  result.phase_0_qualified = result.pending.length === 0;
  for (const gate of result.gates) {
    if (gate.typed !== gate.status) result.mismatches.push(`${gate.id}: ${recordPath}${tree.at} types status ${JSON.stringify(gate.typed)} but the derived status is "${gate.status}" (${gate.basis}); run \`${rewriteCommand}\` to rewrite it`);
  }
  const typed = record.value.phase_0_qualified;
  if (typed !== result.phase_0_qualified) {
    const why = result.pending.length ? ` (${result.pending.join('; ')})` : ' (every receipt gate passed)';
    result.mismatches.push(`phase_0_qualified: ${recordPath}${tree.at} types ${JSON.stringify(typed)} but the derived value is ${result.phase_0_qualified}${why}; run \`${rewriteCommand}\` to rewrite it`);
  }
  return result;
}

/** One line per receipt gate and one for `phase_0_qualified`, as derived. */
export function derivedLines(derived) {
  return [...derived.gates.map(gate => `${gate.id}: ${gate.status} (${gate.basis})`),
    `phase_0_qualified: ${derived.phase_0_qualified}${derived.pending.length ? ` (${derived.pending.join('; ')})` : ''}`];
}

/**
 * Write the derived statuses and `phase_0_qualified` into verification.json. Only those values
 * change: the file is kept in the two-space form `JSON.stringify` gives, with its key order.
 * Returns `{ changed, derived }`.
 */
export async function writeDerivedRecord(root) {
  const path = join(root, recordPath);
  const text = await readFile(path, 'utf8');
  const record = JSON.parse(text);
  if (`${JSON.stringify(record, null, 2)}\n` !== text) throw new Error(`${recordPath} is not in its two-space JSON form, so only the derived values cannot be rewritten; restore its formatting first.`);
  const derived = await derivedRecord(root);
  if (!Object.hasOwn(record, 'phase_0_qualified')) throw new Error(`${recordPath} has no phase_0_qualified field to write.`);
  for (const gate of record.current.gates.phase_0) {
    const entry = derived.gates.find(candidate => candidate.id === gate.id);
    if (!entry) continue;
    if (!Object.hasOwn(gate, 'status')) throw new Error(`${recordPath}: gate ${gate.id} of kind receipt has no status field to write.`);
    gate.status = entry.status;
  }
  record.phase_0_qualified = derived.phase_0_qualified;
  const next = `${JSON.stringify(record, null, 2)}\n`;
  if (next !== text) await writeFile(path, next);
  return { changed: next !== text, derived: await derivedRecord(root) };
}

/** Reasons a parsed receipt's header cannot be trusted against `head`; empty when it can. */
async function headerFailures(root, name, receipt, head) {
  const sha = receipt?.git_sha;
  if (typeof sha !== 'string' || !/^[0-9a-f]{40}$/.test(sha)) return [`${name}: git_sha must be a full 40-hex commit`];
  if (!Array.isArray(receipt.inputs) || !receipt.inputs.length || !receipt.inputs.every(item => typeof item === 'string' && item.length > 0)) {
    return [`${name}: inputs must be a non-empty string[] of repo-relative paths`];
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) return [`${name}: produced_at must be an RFC 3339 time`];
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['merge-base', '--is-ancestor', sha, head])).code !== 0) return [`${name}: git_sha ${sha} is not an ancestor of ${head}`];
  const changed = await git(['diff', '--name-only', sha, head, '--', ...receipt.inputs]);
  if (changed.code !== 0) return [`${name}: could not compare its inputs with ${head} (git exit ${changed.code})`];
  const names = changed.stdout.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return names.length ? [`${name}: inputs changed after ${sha}:\n  ${names.join('\n  ')}`] : [];
}

/**
 * The working tree against HEAD: every receipt describes HEAD, and the typed statuses and
 * `phase_0_qualified` are the derived ones. Throws with every reason; returns one summary line.
 */
export async function checkReceipts(root) {
  const derived = await derivedRecord(root);
  const failures = derived.failures.map(failure => `${failure.name}: ${failure.message}`);
  if (derived.missing) failures.unshift(`${recordPath} is missing`);
  for (const { name, receipt } of derived.receipts) if (receipt !== null) failures.push(...await headerFailures(root, name, receipt, 'HEAD'));
  failures.push(...derived.mismatches);
  if (failures.length) throw new Error(`check-receipts failed:\n${failures.join('\n')}`);
  const summary = derivedLines(derived).join('; ');
  if (!derived.receipts.length) return `check-receipts: no receipts under ${receiptsPath}/; ${summary}; ${recordPath} agrees.`;
  return `check-receipts: ${derived.receipts.length} receipt(s) valid against HEAD; ${summary}; ${recordPath} agrees.`;
}

/** The harness command that regenerates a receipt file, from the gate that names it. */
function rerunCommand(derived, name) {
  const harness = derived.gates.find(gate => gate.file === name)?.harness;
  return typeof harness === 'string' ? `bun qualification/${harness}/run.mjs, then bun qualification/record.mjs ${harness}` : 'rerun the harness that produced it, then bun qualification/record.mjs <name>';
}

/**
 * One line per thing `head` (a commit, evaluated independent of the checkout) cannot be trusted
 * on: a receipt that is stale, cites a non-ancestor commit or has a malformed header, and a
 * typed status or `phase_0_qualified` that differs from what the receipts at that commit derive.
 * Receipts and verification.json are read from that commit's tree. This is what the pre-push
 * hook shows; it blocks `main` and `integration/*` and warns elsewhere.
 */
export async function staleReceiptLines(root, head) {
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['rev-parse', '--verify', '--quiet', `${head}^{commit}`])).code !== 0) throw new Error(`check-receipts: ${head} is not a commit in this repository.`);
  const derived = await derivedRecord(root, head);
  // A commit that has neither a record nor a receipt has nothing to check.
  if (derived.missing && derived.receipts.length === 0) return [];
  const flat = text => text.replace(/\s*\n\s*/g, ' ');
  const lines = [];
  for (const { name, receipt } of derived.receipts) {
    if (receipt === null) continue;
    const failures = await headerFailures(root, name, receipt, head);
    if (failures.length) lines.push(`stale receipt ${flat(failures.join(' '))} -- re-run: ${rerunCommand(derived, name)}`);
  }
  for (const failure of derived.failures) {
    const text = flat(`${failure.name}: ${failure.message}`);
    const isReceipt = derived.receipts.some(entry => entry.name === failure.name);
    lines.push(isReceipt ? `untrusted receipt ${text} -- re-run: ${rerunCommand(derived, failure.name)}` : `untrusted record ${text}`);
  }
  for (const mismatch of derived.mismatches) lines.push(`typed record ${flat(mismatch)}`);
  return lines;
}
