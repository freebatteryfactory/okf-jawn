/**
 * Receipt-backed gate statuses are derived, never typed.
 *
 * verification.json names, for each Phase 0 gate of kind `receipt`, the committed receipt that
 * closes it. `derivedRecord` computes each such gate's status and `phase_0_qualified` from those
 * receipts alone; `writeDerivedRecord` (run by `bun qualification/record.mjs`) is the only writer
 * of the two values, and `checkReceipts` fails when a typed value differs from the derived one.
 *
 * A receipt is trusted for the result it types only when it passes every check: it is a JSON
 * object with the shared header, the commit it cites is an ancestor, none of its inputs changed
 * after that commit, and its envelope is clean against the criteria its gate pins. One that
 * fails any of them leaves its gate `incomplete` in `derivedRecord`, and so in every reader of
 * it: `checkReceipts`, the `--head` path and `record.mjs`.
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
import { envelopeFailures, statusOf } from './receipt-envelope.mjs';

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

const byName = (left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0);

/**
 * Reads of one tree: the working tree when `head` is undefined, otherwise that commit, whatever
 * is checked out. `list` gives every entry of a directory as `{ name, directory }`, so that a
 * directory where a file belongs is something to report and not something to read.
 */
function treeAt(root, head) {
  if (head === undefined) {
    const local = path => join(root, ...path.split('/'));
    return {
      at: '',
      async read(path) {
        try { return await readFile(local(path), 'utf8'); }
        catch (error) { if (error.code === 'ENOENT') return null; throw error; }
      },
      async list(directory) {
        if (!await exists(local(directory))) return [];
        return (await readdir(local(directory), { withFileTypes: true })).map(entry => ({ name: entry.name, directory: entry.isDirectory() })).sort(byName);
      },
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
      // One "<mode> <type> <object>\t<path>" per entry; the type of a directory is `tree`.
      const listed = await git(['ls-tree', '-z', head, `${directory}/`]);
      if (listed.code !== 0) throw new Error(`check-receipts: could not list ${directory}/ at ${head} (git exit ${listed.code})`);
      return listed.stdout.split('\0').filter(Boolean).map(line => {
        const tab = line.indexOf('\t');
        return { name: line.slice(tab + 1 + directory.length + 1), directory: line.slice(0, tab).split(' ')[1] === 'tree' };
      }).sort(byName);
    },
  };
}

/** `text` as JSON, or the reason it is not. */
function parsed(text) {
  try { return { value: JSON.parse(text.replace(/^\uFEFF/, '')) }; }
  catch (error) { return { error: `not valid JSON (${error.message})` }; }
}

/** The text of a receipt file as the JSON object a receipt is, or the reason it is not one: `null`, an array and a bare value parse as JSON and are no receipt. */
function parsedReceipt(text) {
  const outcome = parsed(text);
  if (outcome.error) return outcome;
  const { value } = outcome;
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) return outcome;
  return { error: `not a receipt: its JSON is ${value === null ? 'null' : Array.isArray(value) ? 'an array' : `a ${typeof value}`}, not an object` };
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
 * The status a receipt gate derives once its receipt file exists, and why: `{ status, basis }`.
 * This function is the whole rule; nothing else names a status for such a gate.
 *
 * 1. A receipt that fails any check is never trusted, whatever result it types: the gate is
 *    `incomplete`. `untrusted` lists what it failed: it is not a JSON object, its header is
 *    malformed, it cites a commit that is not an ancestor, its envelope is not clean against
 *    the gate's pinned criteria, it is the receipt of another gate, or two gates name it.
 *    `stale` (the commit it cites and the inputs that changed after it) is the same for a
 *    receipt that was sound when recorded and is no longer about this tree.
 * 2. Otherwise a result of PASS is `passed` and INCOMPLETE is `incomplete`.
 * 3. A FAIL is `failed` when a failing required criterion is not one the owner soundly accepted:
 *    `accepted` (the criteria of the sound entries of the gate's `accepted_failures`) lacks one
 *    of them, or an entry is unsound or stale (`acceptanceSound` false).
 * 4. A FAIL whose every failing required criterion is accepted is `accepted_with_limitations`
 *    only when the run judged everything else: `harness_error` is null and NO required criterion
 *    is `not_judged`. Otherwise it is `incomplete`. A required failure outranks a harness error
 *    in `foldCriteria`, so while an accepted criterion fails on every run the result word cannot
 *    tell a run that was cut short (a converter process killed, a peak that could not be read, a
 *    single-fixture run) from a whole one; this rule can, and such a run never qualifies.
 *
 * `path` is the receipt's path, for the sentence.
 */
export function receiptGateStatus(receipt, { path, untrusted, stale, accepted, acceptanceSound }) {
  if (untrusted.length > 0) {
    const more = untrusted.length > 1 ? ` (and ${untrusted.length - 1} more)` : '';
    return { status: 'incomplete', basis: `${path} cannot be trusted: ${untrusted[0].replace(/\s*\n\s*/g, ' ')}${more}` };
  }
  if (stale) return { status: 'incomplete', basis: `${path} is stale: ${stale.paths.length} of its inputs changed after the commit it cites` };
  const result = receipt.result;
  const plain = { status: statusOf(result), basis: `${path} result ${result}` };
  if (result !== 'FAIL') return plain;
  const failing = failingRequired(receipt);
  const everyFailureAccepted = failing.length > 0 && failing.every(criterion => accepted.includes(criterion));
  if (!acceptanceSound || !everyFailureAccepted) return plain;
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
 * What the header of a parsed receipt says against the commit `head`: `{ failures, stale }`.
 * `failures` are reasons the header cannot be trusted at all: it is malformed, or the commit it
 * cites is not an ancestor of `head`. `stale` is `{ sha, paths }` when the header is sound and
 * one of its inputs changed between the commit it cites and `head`, otherwise null: such a
 * receipt was true of its commit and is no longer evidence about this one.
 */
async function headerProblems(root, receipt, head) {
  const untrusted = message => ({ failures: [message], stale: null });
  const sha = receipt.git_sha;
  if (typeof sha !== 'string' || !/^[0-9a-f]{40}$/.test(sha)) return untrusted('git_sha must be a full 40-hex commit');
  if (!Array.isArray(receipt.inputs) || !receipt.inputs.length || !receipt.inputs.every(item => typeof item === 'string' && item.length > 0)) {
    return untrusted('inputs must be a non-empty string[] of repo-relative paths');
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) return untrusted('produced_at must be an RFC 3339 time');
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['merge-base', '--is-ancestor', sha, head])).code !== 0) return untrusted(`git_sha ${sha} is not an ancestor of ${head}`);
  const changed = await git(['diff', '--name-only', sha, head, '--', ...receipt.inputs]);
  if (changed.code !== 0) return untrusted(`could not compare its inputs with ${head} (git exit ${changed.code})`);
  const paths = changed.stdout.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return { failures: [], stale: paths.length ? { sha, paths } : null };
}

/**
 * What the receipts in a tree support, and where verification.json there types something else.
 * This is the one derivation: `checkReceipts`, `staleReceiptLines` (the `--head` path) and
 * `writeDerivedRecord` (`bun qualification/record.mjs`) all read it and derive nothing themselves.
 *
 * `head` is a commit, read with git and independent of the checkout; without it the working tree
 * is read and its receipts are held to HEAD. A receipt gate is `incomplete` when its receipt file
 * does not exist; otherwise its status is what `receiptGateStatus` gives. A receipt that fails
 * any check is never trusted for the result it types and leaves its gate `incomplete`:
 *
 * - it is not a JSON object (`null`, an array and a bare value are no receipt), or it is a
 *   directory;
 * - its header is malformed, or cites a commit that is not an ancestor of the commit judged;
 * - an input it lists changed after the commit it cites (it is stale);
 * - its content: `envelopeFailures` must be empty against the `required` list of the gate's
 *   criteria file in the same tree (a missing criteria file is a failure), so its result is the
 *   fold of its criteria, every pinned criterion is present and required and it is the receipt
 *   of this gate; and its `not_judged` must be the list its criteria derive;
 * - more than one gate names it.
 *
 * Every one of these but staleness is also a failure (`failures`): something is wrong with what
 * was committed, and `checkReceipts` fails until it is corrected. A stale receipt is no failure
 * of the tree: it is listed in `stale`, its gate is `incomplete`, and the record is right as
 * soon as it says so.
 *
 * `phase_0_qualified` is true only when every receipt gate is `passed` or
 * `accepted_with_limitations`, nothing has a failure and no Phase 0 gate is still hand-typed
 * (has none of `gateKinds`).
 *
 * An accepted failure is checked whether or not a receipt exists (its fields, the owner, the
 * tracking gate, a pinned criterion); once a receipt exists, an accepted criterion that the
 * receipt does not fail is stale and is a failure that says to remove the entry. Every entry
 * under qualification/receipts/ must be the one receipt of one gate.
 *
 * `history: false` is for a tree that has no history to hold a receipt to (a source copy without
 * `.git`, a shallow clone): ancestry and staleness are then not judged, so what comes back is an
 * upper bound of the real derivation and says so in `history`. No command uses it; a test that
 * must also run in such a tree does.
 *
 * Returns `{ missing, history, gates, receipts, stale, unconverted, phase_0_qualified, pending, limitations, failures, mismatches }`
 * (`limitations` names the gates that are `accepted_with_limitations`):
 * `gates` is one `{ id, harness, file, status, typed, basis }` per receipt gate; `receipts` is
 * every entry under qualification/receipts/ as `{ name, receipt }` (`receipt` null when it is no
 * JSON object); `stale` is one `{ name, message }` per stale receipt; `pending` says why
 * `phase_0_qualified` is false; `failures` are `{ name, message }` reasons a receipt or the
 * record cannot be trusted (a reason from the header also carries `header: true`); `mismatches`
 * are the typed values that differ from the derived ones, each naming `rewriteCommand`.
 */
export async function derivedRecord(root, head, { history = true } = {}) {
  const tree = treeAt(root, head);
  const entries = (await tree.list(receiptsPath)).filter(entry => entry.name !== placeholder);
  const receipts = [];
  const failures = [];
  const stale = [];
  // Per receipt file: every reason it cannot be trusted, and the inputs that went stale.
  const untrusted = new Map(entries.map(entry => [entry.name, []]));
  const staleBy = new Map();
  // A header failure is reported after what is wrong with the record and with the content.
  const headerFailures = [];
  for (const { name, directory } of entries) {
    const distrust = (message, list = failures, extra = {}) => {
      untrusted.get(name).push(message);
      list.push({ name, message, ...extra });
    };
    if (directory) {
      distrust(`is a directory; only receipt files belong under ${receiptsPath}/`);
      receipts.push({ name, receipt: null });
      continue;
    }
    const text = await tree.read(`${receiptsPath}/${name}`);
    const outcome = text === null ? { error: `could not be read${tree.at}` } : parsedReceipt(text);
    receipts.push({ name, receipt: outcome.error ? null : outcome.value });
    if (outcome.error) { distrust(outcome.error); continue; }
    if (!history) continue;
    const header = await headerProblems(root, outcome.value, head ?? 'HEAD');
    for (const message of header.failures) distrust(message, headerFailures, { header: true });
    if (header.stale) {
      staleBy.set(name, header.stale);
      stale.push({ name, message: `inputs changed after ${header.stale.sha}:\n  ${header.stale.paths.join('\n  ')}` });
    }
  }
  const result = { missing: false, history, gates: [], receipts, stale, unconverted: [], phase_0_qualified: false, pending: [], limitations: [], failures, mismatches: [] };
  const unfinished = (pending, extra = {}) => { failures.push(...headerFailures); return { ...result, ...extra, pending }; };
  const recordText = await tree.read(recordPath);
  if (recordText === null) {
    for (const { name } of receipts) failures.push({ name, message: `${recordPath} is missing${tree.at}, so no gate names this receipt` });
    return unfinished([`${recordPath} is missing${tree.at}`], { missing: true });
  }
  const record = parsed(recordText);
  if (record.error) {
    failures.push({ name: recordPath, message: record.error });
    return unfinished([`${recordPath} is ${record.error}`]);
  }
  const phase0 = record.value?.current?.gates?.phase_0;
  if (!Array.isArray(phase0)) {
    failures.push({ name: recordPath, message: 'current.gates.phase_0 must be an array of gates' });
    return unfinished([`${recordPath} has no Phase 0 gates`]);
  }
  // A receipt is recorded only for the gate it closes: one file, one gate.
  const named = phase0.filter(gate => gate?.kind === 'receipt').map(receiptFile).filter(file => file !== null);
  const shared = file => named.indexOf(file) !== named.lastIndexOf(file);
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
    const distrusted = [...untrusted.get(file)];
    if (shared(file)) distrusted.push('more than one gate names it');
    let outlived = [];
    if (found.receipt !== null) {
      // The receipt is read, not only its header: its result must be the fold of its criteria,
      // and every criterion its harness pins must be present and required.
      for (const message of await contentFailures(tree, gate, found.receipt)) {
        failures.push({ name: file, message });
        distrusted.push(message);
      }
      // An acceptance outlives its cause when the receipt judged the criterion it names and did
      // not fail it (or no longer has it). A criterion a run did not judge says nothing either way,
      // so an unfinished run can still be recorded beside the acceptance.
      const failing = failingRequired(found.receipt);
      const resultOf = criterion => (Array.isArray(found.receipt.criteria) ? found.receipt.criteria.find(candidate => candidate?.id === criterion)?.result : undefined);
      outlived = acceptance.accepted.filter(criterion => !failing.includes(criterion) && resultOf(criterion) !== 'not_judged');
      for (const criterion of outlived) {
        const now = resultOf(criterion);
        failures.push({ name: gate.id, message: `accepted failure ${criterion} is not failing in ${gate.receipt} (${now === undefined ? 'the receipt has no such criterion' : `its result there is ${now}`}); the acceptance is stale: remove the entry from accepted_failures` });
      }
    }
    const derived = receiptGateStatus(found.receipt, { path: gate.receipt, untrusted: distrusted, stale: staleBy.get(file) ?? null, accepted: acceptance.accepted, acceptanceSound: acceptance.problems.length === 0 && outlived.length === 0 });
    entry.status = derived.status;
    entry.basis = derived.basis;
  }
  for (const gate of result.gates) {
    if (gate.file !== null && shared(gate.file)) failures.push({ name: gate.id, message: `its receipt ${receiptsPath}/${gate.file} is named by more than one gate` });
  }
  // A directory has already been reported as what it is.
  const directories = entries.filter(entry => entry.directory).map(entry => entry.name);
  for (const { name } of receipts) {
    if (!named.includes(name) && !directories.includes(name)) failures.push({ name, message: `no Phase 0 gate of kind receipt names this file under ${receiptsPath}/` });
  }
  failures.push(...headerFailures);
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

/** The harness command that regenerates a receipt file, from the gate that names it. */
function rerunCommand(derived, name) {
  const harness = derived.gates.find(gate => gate.file === name)?.harness;
  return typeof harness === 'string' ? `bun qualification/${harness}/run.mjs, then bun qualification/record.mjs ${harness}` : 'rerun the harness that produced it, then bun qualification/record.mjs <name>';
}

/**
 * One line per stale receipt: the commit it cites, the inputs that changed after it and the
 * commands that replace it. With `onlyWhereTyped`, only the receipts whose gate the record still
 * types a status that the stale receipt no longer supports.
 */
export function staleLines(derived, { onlyWhereTyped = false } = {}) {
  const matters = name => !onlyWhereTyped || derived.gates.some(gate => gate.file === name && gate.typed !== gate.status);
  return derived.stale.filter(entry => matters(entry.name)).map(entry => `${entry.name}: ${entry.message} -- re-run: ${rerunCommand(derived, entry.name)}`);
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

/**
 * The working tree against HEAD: nothing under qualification/receipts/ or in the record has a
 * failure, and the typed statuses and `phase_0_qualified` are the derived ones. Throws with
 * every reason; returns one summary line.
 *
 * A stale receipt alone does not fail: its gate derives `incomplete`, and the check fails for
 * as long as the record types anything else. The cure is to re-run and record the harness, or
 * to run `rewriteCommand`, which writes the gate back to `incomplete`.
 */
export async function checkReceipts(root) {
  const derived = await derivedRecord(root);
  const failures = derived.failures.map(failure => `${failure.name}: ${failure.message}`);
  if (derived.missing) failures.unshift(`${recordPath} is missing`);
  // A stale receipt is named beside the typed value it no longer supports.
  failures.push(...staleLines(derived, { onlyWhereTyped: true }));
  failures.push(...derived.mismatches);
  if (failures.length) throw new Error(`check-receipts failed:\n${failures.join('\n')}`);
  const summary = derivedLines(derived).join('; ');
  const count = derived.receipts.length;
  if (!count) return `check-receipts: no receipts under ${receiptsPath}/; ${summary}; ${recordPath} agrees.`;
  const state = derived.stale.length ? `${count} receipt(s), ${derived.stale.length} stale against HEAD` : `${count} receipt(s) valid against HEAD`;
  return `check-receipts: ${state}; ${summary}; ${recordPath} agrees.`;
}

/**
 * One line per thing `head` (a commit, evaluated independent of the checkout) cannot be trusted
 * on: a receipt that is stale while the record still relies on it, one that cites a non-ancestor
 * commit or has a malformed header, any other failure of a receipt or of the record, a missing
 * verification.json, and a typed status or `phase_0_qualified` that differs from what the
 * receipts at that commit derive. Receipts and verification.json are read from that commit's
 * tree. This is what the pre-push hook shows; it blocks `main` and `integration/*` and warns
 * elsewhere.
 */
export async function staleReceiptLines(root, head) {
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['rev-parse', '--verify', '--quiet', `${head}^{commit}`])).code !== 0) throw new Error(`check-receipts: ${head} is not a commit in this repository.`);
  const derived = await derivedRecord(root, head);
  const flat = text => text.replace(/\s*\n\s*/g, ' ');
  const lines = staleLines(derived, { onlyWhereTyped: true }).map(line => `stale receipt ${flat(line)}`);
  for (const failure of derived.failures.filter(entry => entry.header === true)) lines.push(`stale receipt ${flat(`${failure.name}: ${failure.message}`)} -- re-run: ${rerunCommand(derived, failure.name)}`);
  // A commit without a record fails here as the working tree does.
  if (derived.missing) lines.push(`untrusted record ${recordPath} is missing at ${head}`);
  for (const failure of derived.failures.filter(entry => entry.header !== true)) {
    const text = flat(`${failure.name}: ${failure.message}`);
    const isReceipt = derived.receipts.some(entry => entry.name === failure.name);
    lines.push(isReceipt ? `untrusted receipt ${text} -- re-run: ${rerunCommand(derived, failure.name)}` : `untrusted record ${text}`);
  }
  for (const mismatch of derived.mismatches) lines.push(`typed record ${flat(mismatch)}`);
  return lines;
}
