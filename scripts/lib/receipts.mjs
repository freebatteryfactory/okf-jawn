/**
 * Receipt-backed gate statuses are derived, never typed.
 *
 * verification.json names, for each Phase 0 gate of kind `receipt`, the committed receipt that
 * closes it. `derivedRecord` computes each such gate's status and `phase_0_qualified` from those
 * receipts alone; `writeDerivedRecord` (run by `bun qualification/record.mjs`) is the only writer
 * of the two values, and `checkReceipts` fails when a typed value differs from the derived one.
 * A committed receipt must also describe HEAD: its commit an ancestor, its inputs unchanged.
 *
 * A receipt gate may carry `accepted_failures`: criteria the owner has decided to accept as
 * failing, each tied to the gate that tracks its cure. A FAIL receipt whose every failing
 * required criterion is accepted, and whose run judged everything else (no harness error, no
 * required criterion `not_judged`), derives `accepted_with_limitations`, which qualifies Phase 0
 * as `passed` does. Nothing else is upgraded: not an INCOMPLETE receipt, not a FAIL from a run
 * that was cut short, not a FAIL with one failing criterion nobody accepted, not a receipt whose
 * envelope cannot be trusted. `receiptGateStatus` is that rule. An acceptance that names a
 * criterion its receipt no longer fails is stale and is itself a failure, so the limitation
 * cannot outlive its cause.
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
/** The statuses a receipt gate can derive. The last is a FAIL whose every failing required criterion the owner accepted. */
export const receiptGateStatuses = Object.freeze(['passed', 'failed', 'incomplete', 'accepted_with_limitations']);
/** The statuses with which a receipt gate no longer holds Phase 0 back. */
export const qualifyingStatuses = Object.freeze(['passed', 'accepted_with_limitations']);
/** The fields of one entry of a receipt gate's `accepted_failures`, in file order; every one is required. */
export const acceptedFailureFields = Object.freeze(['criterion', 'decision', 'decided_on', 'decided_by', 'tracked_by']);
/** The only role that may accept a failing criterion. */
export const acceptingRole = 'owner';

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

/** Every gate id of a record, in whichever group it stands. */
function gateIds(record) {
  const groups = record?.current?.gates;
  if (groups === null || typeof groups !== 'object') return [];
  return Object.values(groups).filter(Array.isArray).flat().map(gate => gate?.id).filter(id => typeof id === 'string');
}

/**
 * The accepted failures of a receipt gate: `{ accepted, problems }`. `accepted` holds the
 * criterion ids of the entries that are sound; `problems` says what is wrong with the others.
 * An entry is sound when it has exactly the fields of `acceptedFailureFields` as non-empty
 * strings, `decided_on` is a calendar day, `decided_by` is the owner, `tracked_by` is the id of
 * a gate of the same record, and no other entry names its criterion. When `pinned` is known
 * (the gate's criteria file was readable), the criterion must also be one that file pins.
 */
function acceptedFailures(gate, record, pinned) {
  const entries = gate.accepted_failures;
  if (entries === undefined) return { accepted: [], problems: [] };
  if (!Array.isArray(entries)) return { accepted: [], problems: ['accepted_failures must be an array of entries'] };
  const ids = gateIds(record);
  const accepted = [];
  const problems = [];
  for (const [index, entry] of entries.entries()) {
    const where = `accepted_failures[${index}]`;
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) { problems.push(`${where} must be an object with ${acceptedFailureFields.join(', ')}`); continue; }
    const lacking = acceptedFailureFields.filter(field => typeof entry[field] !== 'string' || entry[field].length === 0);
    const extra = Object.keys(entry).filter(field => !acceptedFailureFields.includes(field));
    const wrong = [];
    if (lacking.length) wrong.push(`${where} lacks ${lacking.join(', ')}`);
    if (extra.length) wrong.push(`${where} has the unknown field(s) ${extra.join(', ')}`);
    if (!lacking.includes('decided_on') && !(/^\d{4}-\d{2}-\d{2}$/.test(entry.decided_on) && new Date(`${entry.decided_on}T00:00:00Z`).toISOString().slice(0, 10) === entry.decided_on)) {
      wrong.push(`${where}.decided_on ${JSON.stringify(entry.decided_on)} is not a calendar day (YYYY-MM-DD)`);
    }
    if (!lacking.includes('decided_by') && entry.decided_by !== acceptingRole) wrong.push(`${where} is decided by ${JSON.stringify(entry.decided_by)}; only the ${acceptingRole} accepts a failing criterion`);
    if (!lacking.includes('tracked_by') && !ids.includes(entry.tracked_by)) wrong.push(`${where}.tracked_by ${JSON.stringify(entry.tracked_by)} is not the id of a gate in ${recordPath}`);
    if (!lacking.includes('criterion')) {
      if (entries.findIndex(other => other?.criterion === entry.criterion) !== index) wrong.push(`${where} repeats the criterion ${entry.criterion}`);
      else if (pinned !== null && !pinned.includes(entry.criterion)) wrong.push(`${where} accepts ${entry.criterion}, which ${gate.criteria} does not pin`);
    }
    if (wrong.length) problems.push(...wrong);
    else accepted.push(entry.criterion);
  }
  return { accepted, problems };
}

/** The ids of the required criteria a receipt fails; empty when its criteria are not a list. */
function failingRequired(receipt) {
  if (!Array.isArray(receipt?.criteria)) return [];
  return receipt.criteria.filter(entry => entry !== null && typeof entry === 'object' && entry.required === true && entry.result === 'fail').map(entry => entry.id);
}

/**
 * The status a receipt gate derives from its receipt, and why: `{ status, basis }`. This
 * function is the whole rule, and nothing else names a status for a receipt that exists.
 *
 * - A result of PASS is `passed`, INCOMPLETE is `incomplete`, and anything that is not one of
 *   `receiptResults` is `incomplete`.
 * - A FAIL is `failed` when a failing required criterion is not one the owner soundly accepted:
 *   `accepted` (the criteria of the sound entries of the gate's `accepted_failures`) lacks one
 *   of them, an entry is unsound or stale (`acceptanceSound` false), or the receipt cannot be
 *   trusted (`trusted` false), so that what it says is accepted proves nothing.
 * - A FAIL whose every failing required criterion is accepted is `accepted_with_limitations`
 *   only when the run judged everything else: `harness_error` is null and NO required criterion
 *   is `not_judged`. Otherwise it is `incomplete`. A required failure outranks a harness error
 *   in `foldCriteria`, so while an accepted criterion fails on every run the result word cannot
 *   tell a run that was cut short (a converter process killed, a peak that could not be read, a
 *   single-fixture run) from a whole one; this rule can, and such a run never qualifies.
 *
 * `path` is the receipt's path, for the sentence.
 */
export function receiptGateStatus(receipt, { path, trusted, accepted, acceptanceSound }) {
  const result = receipt?.result;
  if (!receiptResults.includes(result)) return { status: 'incomplete', basis: `${path} has no result of ${receiptResults.join(', ')}` };
  const plain = { status: statusOf(result), basis: `${path} result ${result}` };
  if (result !== 'FAIL') return plain;
  const failing = failingRequired(receipt);
  const everyFailureAccepted = failing.length > 0 && failing.every(criterion => accepted.includes(criterion));
  if (!trusted || !acceptanceSound || !everyFailureAccepted) return plain;
  const unjudged = receipt.criteria.filter(entry => entry.required === true && entry.result === 'not_judged').length;
  const stopped = receipt.harness_error !== null;
  if (stopped || unjudged > 0) {
    const why = [...(stopped ? ['it records a harness error'] : []), ...(unjudged > 0 ? [`${unjudged} required criteri${unjudged === 1 ? 'on was' : 'a were'} not judged`] : [])];
    return { status: 'incomplete', basis: `${path} result FAIL with every failing criterion accepted, but the run did not judge everything: ${why.join(' and ')}` };
  }
  return { status: 'accepted_with_limitations', basis: `${path} result FAIL; the owner accepted every failing criterion: ${failing.join(', ')}` };
}

/**
 * Why the content of `receipt` cannot be trusted as the receipt of `gate` in `tree`; empty when
 * it can. This is the whole content rule, for a receipt about to be recorded and for one already
 * committed: the gate's criteria file is usable, the envelope is one `envelopeFailures` accepts
 * against the ids that file pins, and `not_judged` is the list the criteria derive.
 */
async function contentFailures(tree, gate, receipt) {
  const { pinned, problems } = await pinnedCriteria(tree, gate);
  return [...problems, ...envelopeFailures(receipt, gate.id, pinned), ...notJudgedFailures(receipt)];
}

/**
 * Why a finished receipt must not be recorded for harness `name`: the content failures of
 * `receipt` against the Phase 0 receipt gate that names the harness in the working tree's
 * verification.json. Empty when it may be recorded, whatever its result: a FAIL or INCOMPLETE
 * receipt with a clean envelope is evidence, and its gate then says failed or incomplete.
 */
export async function recordFailures(root, name, receipt) {
  const tree = treeAt(root);
  const text = await tree.read(recordPath);
  if (text === null) return [`${recordPath} is missing, so no gate names the harness ${name}`];
  const record = parsed(text);
  if (record.error) return [`${recordPath} is ${record.error}`];
  const phase0 = record.value?.current?.gates?.phase_0;
  const gate = Array.isArray(phase0) ? phase0.find(candidate => candidate?.kind === 'receipt' && candidate.harness === name) : undefined;
  if (!gate) return [`no Phase 0 gate of kind receipt names the harness ${name} in ${recordPath}`];
  return contentFailures(tree, gate, receipt);
}

/**
 * What the receipts in a tree support, and where verification.json there types something else.
 *
 * `head` is a commit, read with git and independent of the checkout; without it the working tree
 * is read. A receipt gate is `incomplete` when its receipt file does not exist, otherwise the
 * status `receiptGateStatus` gives: the one its receipt's result supports, except that a FAIL
 * with a clean envelope whose every failing required criterion is a sound entry of the gate's
 * `accepted_failures` is `accepted_with_limitations` when the run judged everything else and
 * `incomplete` when it did not. `phase_0_qualified` is true only when every receipt gate is
 * `passed` or `accepted_with_limitations`, nothing has a failure and no Phase 0 gate is still
 * hand-typed (has none of `gateKinds`).
 *
 * An accepted failure is checked whether or not a receipt exists (its fields, the owner, the
 * tracking gate, a pinned criterion); once a receipt exists, an accepted criterion that the
 * receipt does not fail is stale and is a failure that says to remove the entry.
 *
 * A receipt that exists is read, not only its header: `envelopeFailures` must be empty against
 * the `required` list of the gate's criteria file in the same tree (a missing criteria file is
 * a failure), its `not_judged` must be the list its criteria derive, and every file under
 * qualification/receipts/ must be the one receipt of one gate.
 *
 * Returns `{ missing, gates, receipts, unconverted, phase_0_qualified, pending, limitations, failures, mismatches }`
 * (`limitations` names the gates that are `accepted_with_limitations`):
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
  const result = { missing: false, gates: [], receipts, unconverted: [], phase_0_qualified: false, pending: [], limitations: [], failures, mismatches: [] };
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
    // What the owner accepted is checked with or without a receipt; a criterion can only be
    // held to the pinned list when the criteria file is one that can be read.
    const criteriaFile = Object.hasOwn(gate, 'accepted_failures') ? await pinnedCriteria(tree, gate) : null;
    const acceptance = acceptedFailures(gate, record.value, criteriaFile && criteriaFile.problems.length === 0 ? criteriaFile.pinned : null);
    for (const message of acceptance.problems) failures.push({ name: gate.id, message });
    if (file === null) {
      entry.basis = `its receipt (${JSON.stringify(gate.receipt)}) is not a file under ${receiptsPath}/`;
      failures.push({ name: gate.id, message: entry.basis });
      continue;
    }
    const found = receipts.find(candidate => candidate.name === file);
    if (!found) continue;
    if (found.receipt === null) { entry.basis = `${gate.receipt} is not valid JSON`; continue; }
    // The receipt is read, not only its header: its result must be the fold of its criteria,
    // and every criterion its harness pins must be present and required.
    const untrusted = await contentFailures(tree, gate, found.receipt);
    for (const message of untrusted) failures.push({ name: file, message });
    // An acceptance outlives its cause when the receipt judged the criterion it names and did
    // not fail it (or no longer has it). A criterion a run did not judge says nothing either way,
    // so an unfinished run can still be recorded beside the acceptance.
    const failing = failingRequired(found.receipt);
    const resultOf = criterion => (Array.isArray(found.receipt?.criteria) ? found.receipt.criteria.find(candidate => candidate?.id === criterion)?.result : undefined);
    const stale = acceptance.accepted.filter(criterion => !failing.includes(criterion) && resultOf(criterion) !== 'not_judged');
    for (const criterion of stale) {
      const now = resultOf(criterion);
      failures.push({ name: gate.id, message: `accepted failure ${criterion} is not failing in ${gate.receipt} (${now === undefined ? 'the receipt has no such criterion' : `its result there is ${now}`}); the acceptance is stale: remove the entry from accepted_failures` });
    }
    const derived = receiptGateStatus(found.receipt, { path: gate.receipt, trusted: untrusted.length === 0, accepted: acceptance.accepted, acceptanceSound: acceptance.problems.length === 0 && stale.length === 0 });
    entry.status = derived.status;
    entry.basis = derived.basis;
  }
  // A receipt is recorded only for the gate it closes: one file, one gate.
  const named = result.gates.map(gate => gate.file).filter(file => file !== null);
  for (const gate of result.gates) {
    if (gate.file !== null && named.indexOf(gate.file) !== named.lastIndexOf(gate.file)) failures.push({ name: gate.id, message: `its receipt ${receiptsPath}/${gate.file} is named by more than one gate` });
  }
  for (const { name } of receipts) {
    if (!named.includes(name)) failures.push({ name, message: `no Phase 0 gate of kind receipt names this file under ${receiptsPath}/` });
  }
  for (const gate of result.gates) if (!qualifyingStatuses.includes(gate.status)) result.pending.push(`${gate.id} is ${gate.status}`);
  for (const id of result.unconverted) result.pending.push(`${id} has no kind and is still hand-typed`);
  if (result.gates.length === 0) result.pending.push('no Phase 0 gate is backed by a receipt');
  if (failures.length) result.pending.push(`${failures.length} receipt failure(s)`);
  result.phase_0_qualified = result.pending.length === 0;
  result.limitations = result.gates.filter(gate => gate.status === 'accepted_with_limitations').map(gate => gate.id);
  for (const gate of result.gates) {
    if (gate.typed !== gate.status) result.mismatches.push(`${gate.id}: ${recordPath}${tree.at} types status ${JSON.stringify(gate.typed)} but the derived status is "${gate.status}" (${gate.basis}); run \`${rewriteCommand}\` to rewrite it`);
  }
  const typed = record.value.phase_0_qualified;
  if (typed !== result.phase_0_qualified) {
    const why = result.pending.length ? ` (${result.pending.join('; ')})` : result.limitations.length ? ` (every receipt gate passed or was accepted with limitations: ${result.limitations.join(', ')})` : ' (every receipt gate passed)';
    result.mismatches.push(`phase_0_qualified: ${recordPath}${tree.at} types ${JSON.stringify(typed)} but the derived value is ${result.phase_0_qualified}${why}; run \`${rewriteCommand}\` to rewrite it`);
  }
  return result;
}

/** One line per receipt gate and one for `phase_0_qualified`, as derived. */
export function derivedLines(derived) {
  const withLimitations = derived.phase_0_qualified && derived.limitations.length ? ` (with accepted limitations: ${derived.limitations.join(', ')})` : '';
  return [...derived.gates.map(gate => `${gate.id}: ${gate.status} (${gate.basis})`),
    `phase_0_qualified: ${derived.phase_0_qualified}${derived.pending.length ? ` (${derived.pending.join('; ')})` : withLimitations}`];
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
