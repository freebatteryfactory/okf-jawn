/** check-receipts: receipt-backed statuses and phase_0_qualified are derived, and a receipt describes HEAD. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { cpSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from '../../scripts/lib/process.mjs';
import { acceptedFailureFields, acceptingRole, checkReceipts, derivedLines, derivedRecord, gateKinds, qualifyingStatuses, receiptGateStatuses, rewriteCommand, staleReceiptLines, writeDerivedRecord } from '../../scripts/lib/receipts.mjs';
import { commit, fixtureAcceptance, fixtureCriteria, fixtureGates, fixturePinned, fixtureReceipt, fixtureRecord, fixtureRecordFiles, fixtureRepo, fixtureTracker, git } from './fixture-repo.mjs';

const source = fileURLToPath(new URL('../../', import.meta.url));
const docling = fixtureGates.docling;
const apps = fixtureGates['mcp-apps'];
const at = harness => `qualification/receipts/${harness}.json`;
const failing = [{ id: 'corpus/a.pdf/content', required: true, result: 'pass' }, { id: 'corpus/a.pdf/provenance', required: true, result: 'fail' }];
const unjudged = [{ id: 'corpus/a.pdf/content', required: true, result: 'pass' }, { id: 'corpus/a.pdf/provenance', required: true, result: 'not_judged' }];
/** Status by gate id, and the derived qualification. */
const statuses = derived => ({ ...Object.fromEntries(derived.gates.map(gate => [gate.id, gate.status])), qualified: derived.phase_0_qualified });
/** A repository whose record types nothing the receipts do not support; returns the commit receipts cite. */
async function recorded(t, options) {
  const { base, root } = await fixtureRepo(t, fixtureRecordFiles(options));
  return { base, root, sha: await git(root, 'rev-parse', 'HEAD') };
}

test('a gate without a receipt is incomplete; otherwise the receipt result decides its status', async t => {
  const { root, sha } = await recorded(t);
  assert.deepEqual([...gateKinds], ['receipt', 'ci', 'decision']);
  let derived = await derivedRecord(root);
  assert.deepEqual(statuses(derived), { [docling]: 'incomplete', [apps]: 'incomplete', qualified: false });
  assert.deepEqual(derived.pending, [`${docling} is incomplete`, `${apps} is incomplete`]);
  assert.deepEqual(derived.mismatches, []);
  assert.deepEqual(derivedLines(derived), [`${docling}: incomplete (${at('docling')} is absent)`, `${apps}: incomplete (${at('mcp-apps')} is absent)`,
    `phase_0_qualified: false (${docling} is incomplete; ${apps} is incomplete)`]);

  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha) }, 'one PASS receipt');
  derived = await derivedRecord(root);
  assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'incomplete', qualified: false }, 'one passed gate does not qualify Phase 0');

  await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha, { criteria: failing }) }, 'one FAIL receipt');
  derived = await derivedRecord(root);
  assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'failed', qualified: false });
  assert.equal(derived.gates[1].basis, `${at('mcp-apps')} result FAIL`);

  await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha, { criteria: unjudged }) }, 'one INCOMPLETE receipt');
  assert.deepEqual(statuses(await derivedRecord(root)), { [docling]: 'passed', [apps]: 'incomplete', qualified: false });

  // A result that is not one of the three is not a status, even one named like an object property.
  for (const result of ['constructor', 'toString', 'passed', 'pass', null]) {
    await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha, { result }) }, `result ${result}`);
    derived = await derivedRecord(root);
    assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'incomplete', qualified: false }, String(result));
    assert.equal(derived.gates[1].basis, `${at('mcp-apps')} has no result of PASS, FAIL, INCOMPLETE`);
  }

  await commit(root, { [at('mcp-apps')]: '{not json' }, 'not JSON');
  derived = await derivedRecord(root);
  assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'incomplete', qualified: false });
  assert.match(derived.failures.map(failure => `${failure.name}: ${failure.message}`).join('\n'), /^mcp-apps\.json: not valid JSON/);

  await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha) }, 'both PASS');
  derived = await derivedRecord(root);
  assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'passed', qualified: true });
  assert.deepEqual(derived.pending, []);
});

test('derivedRecord reads the given commit, whatever is checked out', async t => {
  const { root, sha } = await recorded(t);
  const both = await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha),
    'verification.json': fixtureRecord({ statuses: { docling: 'passed', 'mcp-apps': 'passed' }, qualified: true }) }, 'record');
  await git(root, 'checkout', '--quiet', '--detach', sha);
  assert.deepEqual(statuses(await derivedRecord(root)), { [docling]: 'incomplete', [apps]: 'incomplete', qualified: false }, 'the working tree has no receipts');
  const later = await derivedRecord(root, both);
  assert.deepEqual(statuses(later), { [docling]: 'passed', [apps]: 'passed', qualified: true });
  assert.deepEqual(later.mismatches, []);
  assert.deepEqual(statuses(await derivedRecord(root, sha)), { [docling]: 'incomplete', [apps]: 'incomplete', qualified: false });
});

test('check-receipts passes when the record types exactly what the receipts derive', async t => {
  const { root, sha } = await recorded(t);
  assert.equal(await checkReceipts(root), `check-receipts: no receipts under qualification/receipts/; ${docling}: incomplete (${at('docling')} is absent); ${apps}: incomplete (${at('mcp-apps')} is absent); `
    + `phase_0_qualified: false (${docling} is incomplete; ${apps} is incomplete); verification.json agrees.`);
  // A failed qualification is evidence: its gate says failed, and that is a valid record.
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }), 'verification.json': fixtureRecord({ statuses: { docling: 'failed' } }) }, 'record a failure');
  assert.match(await checkReceipts(root), /^check-receipts: 1 receipt\(s\) valid against HEAD; docling-library-qualification: failed \(qualification\/receipts\/docling\.json result FAIL\); /);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha),
    'verification.json': fixtureRecord({ statuses: { docling: 'passed', 'mcp-apps': 'passed' }, qualified: true }) }, 'record both');
  assert.equal(await checkReceipts(root), `check-receipts: 2 receipt(s) valid against HEAD; ${docling}: passed (${at('docling')} result PASS); ${apps}: passed (${at('mcp-apps')} result PASS); phase_0_qualified: true; verification.json agrees.`);
});

test('a typed status the receipts do not derive is rejected, naming the gate, both values and the command', async t => {
  const { root, sha } = await recorded(t);
  assert.equal(rewriteCommand, 'bun qualification/record.mjs');
  await commit(root, { 'verification.json': fixtureRecord({ statuses: { docling: 'passed' } }) }, 'type a pass without a receipt');
  await assert.rejects(checkReceipts(root), error => error.message === 'check-receipts failed:\n'
    + `${docling}: verification.json types status "passed" but the derived status is "incomplete" (${at('docling')} is absent); run \`bun qualification/record.mjs\` to rewrite it`);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }) }, 'a FAIL receipt under a typed pass');
  await assert.rejects(checkReceipts(root), /docling-library-qualification: verification\.json types status "passed" but the derived status is "failed" \(qualification\/receipts\/docling\.json result FAIL\); run `bun qualification\/record\.mjs` to rewrite it/);
  // Typing less than the receipts support cannot survive either: the value is derived, not chosen.
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'verification.json': fixtureRecord() }, 'a PASS receipt under a typed incomplete');
  await assert.rejects(checkReceipts(root), /docling-library-qualification: verification\.json types status "incomplete" but the derived status is "passed" \(qualification\/receipts\/docling\.json result PASS\)/);
  await commit(root, { 'verification.json': fixtureRecord({ statuses: { docling: 'waived' } }) }, 'a status that is no status');
  await assert.rejects(checkReceipts(root), /types status "waived" but the derived status is "passed"/);
});

test('a typed phase_0_qualified the receipts do not derive is rejected with the gates that hold it back', async t => {
  const { root, sha } = await recorded(t);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'verification.json': fixtureRecord({ statuses: { docling: 'passed' }, qualified: true }) }, 'qualified with one gate incomplete');
  await assert.rejects(checkReceipts(root), error => error.message === 'check-receipts failed:\n'
    + `phase_0_qualified: verification.json types true but the derived value is false (${apps} is incomplete); run \`bun qualification/record.mjs\` to rewrite it`);
  await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha), 'verification.json': fixtureRecord({ statuses: { docling: 'passed', 'mcp-apps': 'passed' } }) }, 'both passed, typed unqualified');
  await assert.rejects(checkReceipts(root), /phase_0_qualified: verification\.json types false but the derived value is true \(every receipt gate passed\)/);
  const { phase_0_qualified: _dropped, ...withoutTheField } = JSON.parse(fixtureRecord({ statuses: { docling: 'passed', 'mcp-apps': 'passed' } }));
  await commit(root, { 'verification.json': `${JSON.stringify(withoutTheField, null, 2)}\n` }, 'no phase_0_qualified at all');
  await assert.rejects(checkReceipts(root), /phase_0_qualified: verification\.json types undefined but the derived value is true/);
});

test('a Phase 0 gate that is still hand-typed keeps Phase 0 unqualified; ci and decision gates do not', async t => {
  const { root, sha } = await recorded(t);
  const passed = { docling: 'passed', 'mcp-apps': 'passed' };
  const receipts = { [at('docling')]: fixtureReceipt('docling', sha), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha) };
  const kinds = [{ id: 'enforced', kind: 'ci', enforced_by: {}, covers: 'x' }, { id: 'decided', kind: 'decision', decision: 'x', covers: 'x' }];
  await commit(root, { ...receipts, 'verification.json': fixtureRecord({ statuses: passed, qualified: true, extra: kinds }) }, 'ci and decision gates beside the receipts');
  assert.equal((await derivedRecord(root)).phase_0_qualified, true);
  assert.match(await checkReceipts(root), /phase_0_qualified: true; verification\.json agrees\.$/);
  for (const legacy of [{ id: 'legacy-gate', status: 'passed', receipt: null, meaning: 'typed by hand' }, { id: 'legacy-gate', kind: 'waived', status: 'passed' }]) {
    await commit(root, { 'verification.json': fixtureRecord({ statuses: passed, qualified: true, extra: [...kinds, legacy] }) }, `a gate of kind ${legacy.kind}`);
    const derived = await derivedRecord(root);
    assert.deepEqual(derived.unconverted, ['legacy-gate']);
    assert.equal(derived.phase_0_qualified, false);
    await assert.rejects(checkReceipts(root), /phase_0_qualified: verification\.json types true but the derived value is false \(legacy-gate has no kind and is still hand-typed\)/);
  }
  const noReceiptGates = `${JSON.stringify({ phase_0_qualified: true, current: { gates: { phase_0: kinds } } }, null, 2)}\n`;
  await commit(root, { 'verification.json': noReceiptGates }, 'no receipt gate at all');
  await assert.rejects(checkReceipts(root), /phase_0_qualified: verification\.json types true but the derived value is false \(no Phase 0 gate is backed by a receipt; 2 receipt failure\(s\)\)/);
});

/** Everything check-receipts says about one tree, or '' when it passes. */
const complaint = root => checkReceipts(root).then(() => '', error => error.message);
/** Every failure of a derived record, one per line, as check-receipts prints them. */
const derivedFailureText = derived => derived.failures.map(failure => `${failure.name}: ${failure.message}`).join('\n');
const passedRecord = () => fixtureRecord({ statuses: { docling: 'passed', 'mcp-apps': 'passed' }, qualified: true });

test('check-receipts reads the receipt: a typed PASS over a required fail, a harness error, or an unjudged criterion is rejected', async t => {
  const { root, sha } = await recorded(t);
  const both = overrides => ({ [at('docling')]: fixtureReceipt('docling', sha, overrides), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha), 'verification.json': passedRecord() });
  await commit(root, both({ criteria: failing, result: 'PASS' }), 'PASS typed over a required fail');
  let said = await complaint(root);
  assert.match(said, /^docling\.json: result is PASS but its criteria fold to FAIL$/m);
  assert.match(said, /^phase_0_qualified: verification\.json types true but the derived value is false \(1 receipt failure\(s\)\)/m, 'an untrusted receipt qualifies nothing');
  assert.equal((await derivedRecord(root)).phase_0_qualified, false);
  await commit(root, both({ harness_error: 'the browser was missing', result: 'PASS' }), 'PASS typed over a harness error');
  assert.match(await complaint(root), /^docling\.json: result is PASS but its criteria fold to INCOMPLETE$/m);
  await commit(root, both({ criteria: unjudged, result: 'PASS' }), 'PASS typed over an unjudged criterion');
  assert.match(await complaint(root), /^docling\.json: result is PASS but its criteria fold to INCOMPLETE$/m);
  // The header alone, as receipts were before the envelope, is not a receipt of any result.
  await commit(root, { [at('docling')]: `${JSON.stringify({ git_sha: sha, inputs: ['harness/run.mjs'], produced_at: '2026-10-05T18:00:00Z' })}\n`, 'verification.json': fixtureRecord({ statuses: { 'mcp-apps': 'passed' } }) }, 'a header-only receipt');
  said = await complaint(root);
  for (const reason of ['gate is undefined, expected "docling-library-qualification"', 'result must be one of PASS, FAIL, INCOMPLETE', 'harness_error must be null or a string', 'criteria must be an array']) assert.ok(said.includes(`docling.json: ${reason}`), reason);
  await commit(root, both({}), 'the green case');
  assert.equal(await complaint(root), '');
});

test('check-receipts rejects a receipt for another gate and a file under receipts/ that no gate names', async t => {
  const { root, sha } = await recorded(t);
  // The MCP Apps receipt, copied to the place the Docling gate reads.
  await commit(root, { [at('docling')]: fixtureReceipt('mcp-apps', sha), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha), 'verification.json': passedRecord() }, 'a receipt for the wrong gate');
  let said = await complaint(root);
  assert.match(said, /^docling\.json: gate is "mcp-apps-protocol-qualification", expected "docling-library-qualification"$/m);
  assert.doesNotMatch(said, /^mcp-apps\.json: /m, 'the receipt in its own place is not blamed');
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'qualification/receipts/docling-library-qualification.json': fixtureReceipt('docling', sha) }, 'an orphan receipt');
  said = await complaint(root);
  assert.match(said, /^docling-library-qualification\.json: no Phase 0 gate of kind receipt names this file under qualification\/receipts\/$/m);
  assert.match(said, /\(1 receipt failure\(s\)\)/);
  // Only the placeholder that keeps the directory tracked is not a receipt.
  const { root: other, sha: first } = await recorded(t);
  await commit(other, { 'qualification/receipts/.gitkeep': '' }, 'the placeholder');
  assert.equal(await complaint(other), '');
  await commit(other, { 'qualification/receipts/notes.txt': 'not a receipt\n' }, 'a stray file');
  said = await complaint(other);
  assert.match(said, /^notes\.txt: not valid JSON/m);
  assert.match(said, /^notes\.txt: no Phase 0 gate of kind receipt names this file/m);
  // Two gates cannot share one receipt.
  const shared = JSON.parse(fixtureRecord());
  shared.current.gates.phase_0[1].receipt = at('docling');
  await commit(other, { 'qualification/receipts/notes.txt': fixtureReceipt('docling', first), 'verification.json': `${JSON.stringify(shared, null, 2)}\n` }, 'two gates, one receipt');
  assert.match(await complaint(other), /^mcp-apps-protocol-qualification: its receipt qualification\/receipts\/docling\.json is named by more than one gate$/m);
});

test('check-receipts holds a receipt to the pinned criteria of its gate, read from the same tree', async t => {
  const { root, sha } = await recorded(t);
  const content = { id: 'corpus/a.pdf/content', required: true, result: 'pass' };
  const record = { 'verification.json': fixtureRecord({ statuses: { docling: 'passed' } }) };
  // The harness dropped a check: its result still folds to PASS, and only the pinned list notices.
  await commit(root, { ...record, [at('docling')]: fixtureReceipt('docling', sha, { criteria: [content] }) }, 'a pinned criterion is missing');
  assert.match(await complaint(root), /^docling\.json: pinned criterion corpus\/a\.pdf\/provenance is missing$/m);
  // The harness made a failing check optional: the fold ignores it, the pinned list does not.
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: [content, { id: 'corpus/a.pdf/provenance', required: false, result: 'fail' }] }) }, 'a pinned criterion is not required');
  assert.match(await complaint(root), /^docling\.json: pinned criterion corpus\/a\.pdf\/provenance is not marked required$/m);
  // A criterion the file pins later is demanded of the receipt already recorded.
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'qualification/docling/criteria.json': fixtureCriteria('docling', [...fixturePinned, 'corpus/a.pdf/tables']) }, 'the criteria file pins one more');
  assert.match(await complaint(root), /^docling\.json: pinned criterion corpus\/a\.pdf\/tables is missing$/m);
  for (const [why, text, expected] of [
    ['for another gate', fixtureCriteria('mcp-apps'), /^docling\.json: its criteria file qualification\/docling\/criteria\.json is for gate "mcp-apps-protocol-qualification", not docling-library-qualification$/m],
    ['not JSON', '{', /^docling\.json: its criteria file qualification\/docling\/criteria\.json is not valid JSON/m],
    ['an empty list', fixtureCriteria('docling', []), /^docling\.json: its criteria file qualification\/docling\/criteria\.json must list each required criterion id once in a non-empty "required" array$/m],
    ['a repeated id', fixtureCriteria('docling', ['a', 'a']), /must list each required criterion id once/],
    ['no list', `${JSON.stringify({ gate: docling })}\n`, /must list each required criterion id once/],
    ['not an object', '[]\n', /^docling\.json: its criteria file qualification\/docling\/criteria\.json must be an object with "gate" and "required"$/m],
  ]) {
    await commit(root, { 'qualification/docling/criteria.json': text }, `criteria file: ${why}`);
    assert.match(await complaint(root), expected, why);
  }
  await commit(root, { 'qualification/docling/criteria.json': fixtureCriteria('docling') }, 'the criteria file again');
  assert.equal(await complaint(root), '');
  // not_judged is derived from the criteria too; a list that hides an unjudged criterion is named.
  const optional = [...fixturePinned.map(id => ({ id, required: true, result: 'pass' })), { id: 'memory/peak', required: false, result: 'not_judged' }];
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: optional, not_judged: [] }) }, 'not_judged typed empty');
  assert.match(await complaint(root), /^docling\.json: not_judged is \[\] but its criteria derive \["memory\/peak"\]$/m);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: optional }) }, 'not_judged as derived');
  assert.equal(await complaint(root), '');
});

test('a receipt whose criteria file is missing is rejected, and one without a receipt needs none yet', async t => {
  // The harness packages add the criteria files; until then a gate without a receipt is simply incomplete.
  const { root } = await fixtureRepo(t, { 'verification.json': fixtureRecord(), 'harness/run.mjs': '// v1\n' });
  const sha = await git(root, 'rev-parse', 'HEAD');
  assert.equal(await complaint(root), '');
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'verification.json': fixtureRecord({ statuses: { docling: 'passed' } }) }, 'a receipt without its criteria file');
  const said = await complaint(root);
  assert.match(said, /^docling\.json: its criteria file qualification\/docling\/criteria\.json is missing, so nothing pins what this receipt must have judged$/m);
  assert.match(said, /^check-receipts failed:\n/);
  const head = await git(root, 'rev-parse', 'HEAD');
  assert.deepEqual(await staleReceiptLines(root, head), [
    `untrusted receipt docling.json: its criteria file qualification/docling/criteria.json is missing at ${head}, so nothing pins what this receipt must have judged -- re-run: bun qualification/docling/run.mjs, then bun qualification/record.mjs docling`]);
});

test('staleReceiptLines reports a hand-edited receipt at the pushed commit like a stale one', async t => {
  const { root, sha } = await recorded(t);
  const honest = await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }), 'verification.json': fixtureRecord({ statuses: { docling: 'failed' } }) }, 'record a failure');
  assert.deepEqual(await staleReceiptLines(root, honest), []);
  // The result and the status are both edited to a pass; the criteria still say what was judged.
  const edited = await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing, result: 'PASS' }), 'verification.json': fixtureRecord({ statuses: { docling: 'passed' } }) }, 'edit the receipt to PASS');
  assert.deepEqual(await staleReceiptLines(root, edited), ['untrusted receipt docling.json: result is PASS but its criteria fold to FAIL -- re-run: bun qualification/docling/run.mjs, then bun qualification/record.mjs docling']);
  const orphan = await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }), 'verification.json': fixtureRecord({ statuses: { docling: 'failed' } }), 'qualification/receipts/extra.json': fixtureReceipt('docling', sha) }, 'an orphan receipt');
  assert.deepEqual(await staleReceiptLines(root, orphan), ['untrusted receipt extra.json: no Phase 0 gate of kind receipt names this file under qualification/receipts/ -- re-run: rerun the harness that produced it, then bun qualification/record.mjs <name>']);
  // Judged at the earlier commits whatever is checked out.
  await git(root, 'checkout', '--quiet', '--detach', sha);
  assert.deepEqual(await staleReceiptLines(root, honest), []);
  assert.equal((await staleReceiptLines(root, edited)).length, 1);
});

test('writeDerivedRecord rewrites only the derived values and keeps the form of the file', async t => {
  const { root, sha } = await recorded(t);
  const typed = JSON.parse(fixtureRecord({ statuses: { docling: 'incomplete', 'mcp-apps': 'passed' }, qualified: true, extra: [{ id: 'decided', kind: 'decision', decision: 'kept', covers: 'x' }] }));
  const before = { project: 'fixture', ...typed, historical_archive_record: { note: 'kept unchanged', phase_0_qualified: false, checks: [{ status: 'passed', command: ['node', 'x'] }], counts: { '.toml': 15 } } };
  await commit(root, { 'verification.json': `${JSON.stringify(before, null, 2)}\n`, [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }) }, 'a record typed wrongly');
  const path = join(root, 'verification.json');
  const { changed, derived } = await writeDerivedRecord(root);
  assert.equal(changed, true);
  assert.deepEqual(statuses(derived), { [docling]: 'failed', [apps]: 'incomplete', qualified: false });
  assert.deepEqual(derived.mismatches, [], 'what it returns is the tree as rewritten');
  const expected = structuredClone(before);
  expected.phase_0_qualified = false;
  expected.current.gates.phase_0[0].status = 'failed';
  expected.current.gates.phase_0[1].status = 'incomplete';
  const text = await readFile(path, 'utf8');
  assert.equal(text, `${JSON.stringify(expected, null, 2)}\n`, 'only the three derived values differ, in the same key order and indentation');
  assert.deepEqual(JSON.parse(text).historical_archive_record, before.historical_archive_record);
  assert.match(await checkReceipts(root), /verification\.json agrees\.$/, 'the rewritten record is what check-receipts expects');
  // Rewriting again changes nothing, byte for byte.
  assert.equal((await writeDerivedRecord(root)).changed, false);
  assert.equal(await readFile(path, 'utf8'), text);
  // A file in another form is refused rather than reformatted.
  const reformatted = `${JSON.stringify(expected, null, 4)}\n`;
  await writeFile(path, reformatted);
  await assert.rejects(writeDerivedRecord(root), /verification\.json is not in its two-space JSON form/);
  assert.equal(await readFile(path, 'utf8'), reformatted);
  // A receipt gate without a status field has nowhere to write the value.
  const { status: _dropped, ...statusless } = expected.current.gates.phase_0[0];
  await writeFile(path, `${JSON.stringify({ ...expected, current: { gates: { phase_0: [statusless, expected.current.gates.phase_0[1]] } } }, null, 2)}\n`);
  await assert.rejects(writeDerivedRecord(root), /gate docling-library-qualification of kind receipt has no status field to write/);
});

test('record.mjs copies a receipt of any result and writes the derived statuses, one line per gate', async t => {
  const { root, sha } = await recorded(t);
  // The real script and library run inside the fixture; the copies are untracked, which recording allows.
  cpSync(join(source, 'scripts'), join(root, 'scripts'), { recursive: true });
  await mkdir(join(root, 'qualification'), { recursive: true });
  cpSync(join(source, 'qualification', 'record.mjs'), join(root, 'qualification', 'record.mjs'));
  const artifact = join(root, '.artifacts', 'qualification', 'docling');
  await mkdir(artifact, { recursive: true });
  const receipt = fixtureReceipt('docling', sha, { criteria: failing });
  await writeFile(join(artifact, 'receipt.json'), receipt);
  const record = (...names) => run(process.execPath, ['qualification/record.mjs', ...names], { cwd: root, capture: true, allowFailure: true });
  const first = await record('docling');
  assert.equal(first.code, 0, first.stderr);
  const lines = first.stdout.trim().split(/\r?\n/);
  assert.match(lines[0], /^recorded .*receipt\.json -> .*docling\.json$/);
  assert.deepEqual(lines.slice(1), [`${docling}: failed (${at('docling')} result FAIL)`, `${apps}: incomplete (${at('mcp-apps')} is absent)`,
    `phase_0_qualified: false (${docling} is failed; ${apps} is incomplete)`, 'verification.json rewritten from the receipts.']);
  assert.deepEqual(JSON.parse(await readFile(join(root, at('docling')), 'utf8')), JSON.parse(receipt), 'a FAIL receipt is recorded as it was written');
  assert.equal(await readFile(join(root, 'verification.json'), 'utf8'), fixtureRecord({ statuses: { docling: 'failed' } }));
  // With no name nothing is copied and the values are only derived again.
  const again = await record();
  assert.equal(again.code, 0, again.stderr);
  assert.deepEqual(again.stdout.trim().split(/\r?\n/).at(-1), 'verification.json already agrees with the receipts.');
  assert.doesNotMatch(again.stdout, /^recorded /m);
  const unknown = await record('no-such-harness');
  assert.notEqual(unknown.code, 0);
  assert.match(unknown.stderr, /Usage: bun qualification\/record\.mjs \[<name>\.\.\.\] where name is one of docling, mcp-apps/);
  // Committed as the record commit, the tree is what check-receipts accepts.
  const head = await commit(root, {}, 'record');
  assert.match(await checkReceipts(root), /^check-receipts: 1 receipt\(s\) valid against HEAD; docling-library-qualification: failed /);
  // An INCOMPLETE receipt is recorded like any other, whichever of its sections ran; its gate says incomplete.
  const appsArtifact = join(root, '.artifacts', 'qualification', 'mcp-apps', 'receipt.json');
  await mkdir(join(root, '.artifacts', 'qualification', 'mcp-apps'), { recursive: true });
  await writeFile(appsArtifact, fixtureReceipt('mcp-apps', head, { criteria: unjudged, protocol_only: true, basic_host: { status: 'not_run' } }));
  const partial = await record('mcp-apps');
  assert.equal(partial.code, 0, partial.stderr);
  assert.match(partial.stdout, new RegExp(`^${apps}: incomplete \\(${at('mcp-apps')} result INCOMPLETE\\)$`, 'm'));
  assert.equal(await readFile(join(root, 'verification.json'), 'utf8'), fixtureRecord({ statuses: { docling: 'failed' } }), 'incomplete was already the typed status');
  // Right after recording, the tree is one check-receipts accepts: the statuses were written with the copy.
  const second = await commit(root, {}, 'record the second receipt');
  assert.match(await checkReceipts(root), /^check-receipts: 2 receipt\(s\) valid against HEAD; /);
  // A receipt whose envelope cannot be trusted is refused: nothing is copied, not even a sound one named beside it.
  const recordedApps = await readFile(join(root, at('mcp-apps')), 'utf8');
  const recordedDocling = await readFile(join(root, at('docling')), 'utf8');
  await writeFile(appsArtifact, fixtureReceipt('mcp-apps', second, { criteria: failing, result: 'PASS' }));
  await writeFile(join(artifact, 'receipt.json'), fixtureReceipt('docling', second));
  const untrusted = await record('docling', 'mcp-apps');
  assert.notEqual(untrusted.code, 0, untrusted.stdout);
  assert.match(untrusted.stderr, /record refused for mcp-apps: its envelope cannot be trusted \(result is PASS but its criteria fold to FAIL\)/);
  assert.doesNotMatch(untrusted.stdout, /^recorded /m);
  assert.equal(await readFile(join(root, at('mcp-apps')), 'utf8'), recordedApps);
  assert.equal(await readFile(join(root, at('docling')), 'utf8'), recordedDocling, 'the sound receipt named in the same command is not copied either');
  // So is one that cites another commit, before anything is copied.
  await writeFile(appsArtifact, fixtureReceipt('mcp-apps', head));
  const stale = await record('docling', 'mcp-apps');
  assert.notEqual(stale.code, 0);
  assert.match(stale.stderr, new RegExp(`record refused for mcp-apps: receipt cites ${head} but HEAD is ${second}`));
  assert.equal(await readFile(join(root, at('docling')), 'utf8'), recordedDocling);
  // A committed receipt that was edited afterwards is not this command's to refuse: it reports it and fails.
  await writeFile(join(root, at('mcp-apps')), fixtureReceipt('mcp-apps', head, { criteria: failing, result: 'PASS' }));
  const edited = await record();
  assert.equal(edited.code, 1, edited.stdout);
  assert.match(edited.stderr, /^mcp-apps\.json: result is PASS but its criteria fold to FAIL$/m);
  assert.match(edited.stdout, /^phase_0_qualified: false \(.*1 receipt failure\(s\)\)$/m);
});

/** A record whose Docling gate accepts `entries`, tracked by the fixture construction gate. */
const accepting = (entries, options = {}) => fixtureRecord({ accepted: { docling: entries }, construction: [fixtureTracker], ...options });
const failingBoth = [{ id: 'corpus/a.pdf/content', required: true, result: 'fail' }, { id: 'corpus/a.pdf/provenance', required: true, result: 'fail' }];
const accepted = 'accepted_with_limitations';

test('a FAIL receipt whose every failing criterion the owner accepted derives accepted_with_limitations, and that qualifies Phase 0', async t => {
  const { root, sha } = await recorded(t);
  assert.deepEqual([...receiptGateStatuses], ['passed', 'failed', 'incomplete', accepted]);
  assert.deepEqual([...qualifyingStatuses], ['passed', accepted]);
  assert.deepEqual([...acceptedFailureFields], ['criterion', 'decision', 'decided_on', 'decided_by', 'tracked_by']);
  assert.equal(acceptingRole, 'owner');

  // Without an acceptance a failing receipt is failed, as before.
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha), 'verification.json': fixtureRecord({ construction: [fixtureTracker] }) }, 'a failing receipt');
  assert.deepEqual(statuses(await derivedRecord(root)), { [docling]: 'failed', [apps]: 'passed', qualified: false });

  // The owner accepts the one failing criterion.
  await commit(root, { 'verification.json': accepting([fixtureAcceptance()]) }, 'accept the failing criterion');
  let derived = await derivedRecord(root);
  assert.deepEqual(derived.failures, []);
  assert.deepEqual(statuses(derived), { [docling]: accepted, [apps]: 'passed', qualified: true });
  assert.deepEqual(derived.pending, []);
  assert.deepEqual(derived.limitations, [docling]);
  assert.equal(derived.gates[0].basis, `${at('docling')} result FAIL; the owner accepted every failing criterion: corpus/a.pdf/provenance`);
  assert.deepEqual(derivedLines(derived), [`${docling}: ${accepted} (${at('docling')} result FAIL; the owner accepted every failing criterion: corpus/a.pdf/provenance)`,
    `${apps}: passed (${at('mcp-apps')} result PASS)`, `phase_0_qualified: true (with accepted limitations: ${docling})`]);
  // The record still types the old values, and is told the derived ones.
  const said = await complaint(root);
  assert.match(said, new RegExp(`^${docling}: verification\\.json types status "incomplete" but the derived status is "${accepted}" `, 'm'));
  assert.match(said, new RegExp(`^phase_0_qualified: verification\\.json types false but the derived value is true \\(every receipt gate passed or was accepted with limitations: ${docling}\\)`, 'm'));
  // record.mjs writes them, and the tree is then one check-receipts accepts.
  assert.equal((await writeDerivedRecord(root)).changed, true);
  assert.equal(await readFile(join(root, 'verification.json'), 'utf8'), accepting([fixtureAcceptance()], { statuses: { docling: accepted, 'mcp-apps': 'passed' }, qualified: true }));
  const head = await commit(root, {}, 'record');
  assert.match(await checkReceipts(root), new RegExp(`^check-receipts: 2 receipt\\(s\\) valid against HEAD; ${docling}: ${accepted} \\(.*phase_0_qualified: true \\(with accepted limitations: ${docling}\\); verification\\.json agrees\\.$`));
  assert.deepEqual(await staleReceiptLines(root, head), []);

  // The other gate still has to qualify on its own.
  await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha, { criteria: unjudged }) }, 'the other gate is incomplete');
  assert.deepEqual(statuses(await derivedRecord(root)), { [docling]: accepted, [apps]: 'incomplete', qualified: false });
});

test('nothing but a trusted FAIL with every failing criterion accepted is upgraded', async t => {
  const { root, sha } = await recorded(t);
  const other = { [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha) };
  const derive = async (receipt, entries, why) => {
    await commit(root, { ...other, [at('docling')]: receipt, 'verification.json': accepting(entries) }, why);
    return derivedRecord(root);
  };

  // One of two failing criteria is not accepted: the gate is failed.
  let derived = await derive(fixtureReceipt('docling', sha, { criteria: failingBoth }), [fixtureAcceptance()], 'one failure nobody accepted');
  assert.deepEqual(statuses(derived), { [docling]: 'failed', [apps]: 'passed', qualified: false });
  assert.deepEqual(derived.failures, [], 'the acceptance itself is sound; the receipt simply fails more');
  assert.deepEqual(derived.pending, [`${docling} is failed`]);
  // Both accepted: upgraded.
  derived = await derive(fixtureReceipt('docling', sha, { criteria: failingBoth }), [fixtureAcceptance(), fixtureAcceptance('corpus/a.pdf/content')], 'both accepted');
  assert.deepEqual(statuses(derived), { [docling]: accepted, [apps]: 'passed', qualified: true });
  assert.match(derived.gates[0].basis, /the owner accepted every failing criterion: corpus\/a\.pdf\/content, corpus\/a\.pdf\/provenance$/);

  // An INCOMPLETE receipt is never upgraded. A criterion it did not judge says nothing about the acceptance.
  derived = await derive(fixtureReceipt('docling', sha, { criteria: unjudged }), [fixtureAcceptance()], 'an unfinished run');
  assert.deepEqual(statuses(derived), { [docling]: 'incomplete', [apps]: 'passed', qualified: false });
  assert.deepEqual(derived.failures, []);
  derived = await derive(fixtureReceipt('docling', sha, { criteria: failing, harness_error: 'the model file was missing', result: 'INCOMPLETE' }), [fixtureAcceptance()], 'INCOMPLETE typed over a failing criterion');
  assert.equal(derived.gates[0].status, 'incomplete');
  assert.match(derivedFailureText(derived), /^docling\.json: result is INCOMPLETE but its criteria fold to FAIL$/m);

  // A FAIL whose one failure is accepted is still not upgraded when its run did not judge everything:
  // a required failure outranks a harness error in the fold, so the result word cannot tell the two apart.
  for (const [why, overrides, basis] of [
    ['a harness error beside the accepted failure', { criteria: failing, harness_error: 'the converter process for b.pdf was killed at the harness timeout' }, 'it records a harness error'],
    ['a required criterion nobody judged', { criteria: [{ id: 'corpus/a.pdf/content', required: true, result: 'not_judged' }, failing[1]] }, '1 required criterion was not judged'],
    ['both', { criteria: [{ id: 'corpus/a.pdf/content', required: true, result: 'not_judged' }, failing[1]], harness_error: 'the peak memory was not measured' }, 'it records a harness error and 1 required criterion was not judged'],
  ]) {
    const receipt = fixtureReceipt('docling', sha, overrides);
    assert.equal(JSON.parse(receipt).result, 'FAIL', `${why}: the fold says FAIL, as the harness wrote it`);
    derived = await derive(receipt, [fixtureAcceptance()], why);
    assert.deepEqual(derived.failures, [], `${why}: the receipt is clean evidence`);
    assert.deepEqual(statuses(derived), { [docling]: 'incomplete', [apps]: 'passed', qualified: false }, why);
    assert.deepEqual(derived.limitations, [], why);
    assert.equal(derived.gates[0].basis, `${at('docling')} result FAIL with every failing criterion accepted, but the run did not judge everything: ${basis}`, why);
  }
  // A failure nobody accepted is a failure whatever else the run did not judge.
  derived = await derive(fixtureReceipt('docling', sha, { criteria: failingBoth, harness_error: 'the peak memory was not measured' }), [fixtureAcceptance()], 'a harness error beside a failure nobody accepted');
  assert.deepEqual(statuses(derived), { [docling]: 'failed', [apps]: 'passed', qualified: false });
  // A measurement recorded without a verdict (not required) holds nothing back.
  const measured = [...failing, { id: 'memory/peak', required: false, result: 'not_judged' }];
  derived = await derive(fixtureReceipt('docling', sha, { criteria: measured }), [fixtureAcceptance()], 'an optional criterion not judged');
  assert.deepEqual(statuses(derived), { [docling]: accepted, [apps]: 'passed', qualified: true });

  // A receipt whose envelope cannot be trusted is not upgraded, whatever is accepted.
  derived = await derive(fixtureReceipt('docling', sha, { criteria: failing, not_judged: ['corpus/a.pdf/content'] }), [fixtureAcceptance()], 'a FAIL with a wrong not_judged list');
  assert.deepEqual(statuses(derived), { [docling]: 'failed', [apps]: 'passed', qualified: false });
  assert.match(derivedFailureText(derived), /^docling\.json: not_judged is \["corpus\/a\.pdf\/content"\] but its criteria derive \[\]$/m);
  derived = await derive(fixtureReceipt('docling', sha, { criteria: [failing[1]] }), [fixtureAcceptance()], 'a FAIL that lacks a pinned criterion');
  assert.equal(derived.gates[0].status, 'failed');
  assert.match(derivedFailureText(derived), /^docling\.json: pinned criterion corpus\/a\.pdf\/content is missing$/m);

  // A failing criterion that is not required does not fail the receipt, so there is nothing to accept.
  const optionalFail = [{ id: 'corpus/a.pdf/content', required: true, result: 'pass' }, { id: 'corpus/a.pdf/provenance', required: true, result: 'pass' }, { id: 'memory/peak', required: false, result: 'fail' }];
  derived = await derive(fixtureReceipt('docling', sha, { criteria: optionalFail }), [], 'an optional failure');
  assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'passed', qualified: true });
});

test('an acceptance that outlived its cause is a failure that says to remove it', async t => {
  const { root, sha } = await recorded(t);
  const record = { 'verification.json': accepting([fixtureAcceptance()], { statuses: { docling: 'passed', 'mcp-apps': 'passed' }, qualified: true }), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha) };
  // The library was fixed: the receipt passes the criterion the record still accepts as failing.
  const head = await commit(root, { ...record, [at('docling')]: fixtureReceipt('docling', sha) }, 'the criterion passes now');
  const stale = `${docling}: accepted failure corpus/a.pdf/provenance is not failing in ${at('docling')} (its result there is pass); the acceptance is stale: remove the entry from accepted_failures`;
  let derived = await derivedRecord(root);
  assert.deepEqual(derived.failures, [{ name: docling, message: stale.slice(docling.length + 2) }]);
  assert.deepEqual(statuses(derived), { [docling]: 'passed', [apps]: 'passed', qualified: false }, 'a stale acceptance keeps Phase 0 unqualified until it is removed');
  await assert.rejects(checkReceipts(root), error => error.message.split('\n').includes(stale));
  assert.deepEqual(await staleReceiptLines(root, head), [`untrusted record ${stale}`, `typed record phase_0_qualified: verification.json at ${head} types true but the derived value is false (1 receipt failure(s)); run \`bun qualification/record.mjs\` to rewrite it`]);
  // Removing the entry is the cure.
  await commit(root, { 'verification.json': fixtureRecord({ statuses: { docling: 'passed', 'mcp-apps': 'passed' }, qualified: true, construction: [fixtureTracker] }) }, 'remove the acceptance');
  assert.equal(await complaint(root), '');

  // Still failing, but one accepted criterion among two no longer fails: stale, and no upgrade.
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }), 'verification.json': accepting([fixtureAcceptance(), fixtureAcceptance('corpus/a.pdf/content')]) }, 'one of two acceptances is stale');
  derived = await derivedRecord(root);
  assert.equal(derived.gates[0].status, 'failed');
  assert.deepEqual(derived.failures.map(failure => failure.message), [`accepted failure corpus/a.pdf/content is not failing in ${at('docling')} (its result there is pass); the acceptance is stale: remove the entry from accepted_failures`]);
  // Not applicable by design is not failing either.
  const notApplicable = [{ id: 'corpus/a.pdf/content', required: true, result: 'fail' }, { id: 'corpus/a.pdf/provenance', required: true, result: 'not_applicable', detail: 'the library gives no locator' }];
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { criteria: notApplicable }), 'verification.json': accepting([fixtureAcceptance(), fixtureAcceptance('corpus/a.pdf/content')]) }, 'accepted and not applicable');
  assert.match(derivedFailureText(await derivedRecord(root)), /accepted failure corpus\/a\.pdf\/provenance is not failing in .* \(its result there is not_applicable\); the acceptance is stale/);
});

test('an accepted failure must be whole, decided by the owner and tracked by a gate that exists', async t => {
  const { root, sha } = await recorded(t);
  const receipts = { [at('docling')]: fixtureReceipt('docling', sha, { criteria: failing }), [at('mcp-apps')]: fixtureReceipt('mcp-apps', sha) };
  const problems = async (entries, why, files = receipts) => {
    await commit(root, { ...files, 'verification.json': accepting(entries) }, why);
    const derived = await derivedRecord(root);
    // An acceptance that is not sound upgrades nothing.
    assert.equal(derived.gates[0].status, Object.hasOwn(files, at('docling')) ? 'failed' : derived.gates[0].status, why);
    assert.equal(derived.phase_0_qualified, false, why);
    await assert.rejects(checkReceipts(root), /check-receipts failed:/, why);
    return derived.failures.filter(failure => failure.name === docling).map(failure => failure.message);
  };
  const without = field => { const { [field]: _dropped, ...rest } = fixtureAcceptance(); return rest; };

  for (const field of acceptedFailureFields) {
    assert.deepEqual(await problems([without(field)], `no ${field}`), [`accepted_failures[0] lacks ${field}`]);
    assert.deepEqual(await problems([fixtureAcceptance(undefined, { [field]: '' })], `empty ${field}`), [`accepted_failures[0] lacks ${field}`]);
  }
  assert.deepEqual(await problems([fixtureAcceptance(undefined, { decided_by: 'integration-owner' })], 'decided by a lane'), ['accepted_failures[0] is decided by "integration-owner"; only the owner accepts a failing criterion']);
  assert.deepEqual(await problems([fixtureAcceptance(undefined, { decided_by: 'Owner' })], 'decided by a name'), ['accepted_failures[0] is decided by "Owner"; only the owner accepts a failing criterion']);
  assert.deepEqual(await problems([fixtureAcceptance(undefined, { tracked_by: 'no-such-gate' })], 'tracked by nothing'), ['accepted_failures[0].tracked_by "no-such-gate" is not the id of a gate in verification.json']);
  assert.deepEqual(await problems([fixtureAcceptance(undefined, { decided_on: 'yesterday' })], 'no date'), ['accepted_failures[0].decided_on "yesterday" is not a calendar day (YYYY-MM-DD)']);
  assert.deepEqual(await problems([fixtureAcceptance(undefined, { decided_on: '2026-02-30' })], 'no such day'), ['accepted_failures[0].decided_on "2026-02-30" is not a calendar day (YYYY-MM-DD)']);
  assert.deepEqual(await problems([fixtureAcceptance(undefined, { waived: 'yes' })], 'an unknown field'), ['accepted_failures[0] has the unknown field(s) waived']);
  assert.deepEqual(await problems(['corpus/a.pdf/provenance'], 'a bare id'), ['accepted_failures[0] must be an object with criterion, decision, decided_on, decided_by, tracked_by']);
  assert.deepEqual(await problems({ criterion: 'corpus/a.pdf/provenance' }, 'not a list'), ['accepted_failures must be an array of entries']);
  assert.deepEqual(await problems([fixtureAcceptance(), fixtureAcceptance()], 'the same criterion twice'), ['accepted_failures[1] repeats the criterion corpus/a.pdf/provenance']);
  assert.deepEqual(await problems([fixtureAcceptance('corpus/a.pdf/tables')], 'a criterion nothing pins'), ['accepted_failures[0] accepts corpus/a.pdf/tables, which qualification/docling/criteria.json does not pin']);
  // Several things wrong with one entry are all said.
  assert.deepEqual(await problems([{ criterion: 'corpus/a.pdf/provenance', decided_by: 'agent', tracked_by: 'nothing' }], 'three things wrong'),
    ['accepted_failures[0] lacks decision, decided_on', 'accepted_failures[0] is decided by "agent"; only the owner accepts a failing criterion', 'accepted_failures[0].tracked_by "nothing" is not the id of a gate in verification.json']);
  // The tracking gate may stand in any group of the record, and it must be there.
  await commit(root, { ...receipts, 'verification.json': fixtureRecord({ accepted: { docling: [fixtureAcceptance()] } }) }, 'the tracking gate is gone');
  assert.deepEqual((await derivedRecord(root)).failures.map(failure => failure.message), ['accepted_failures[0].tracked_by "converter-font-run-spacing" is not the id of a gate in verification.json']);
  await commit(root, { 'verification.json': fixtureRecord({ accepted: { docling: [fixtureAcceptance(undefined, { tracked_by: apps })] } }) }, 'tracked by a Phase 0 gate');
  assert.equal((await derivedRecord(root)).gates[0].status, accepted);

  // Before any receipt exists an acceptance is checked all the same, and a sound one is no failure.
  const { root: bare } = await recorded(t);
  await commit(bare, { 'verification.json': accepting([fixtureAcceptance()]) }, 'accepted before any run');
  assert.match(await checkReceipts(bare), new RegExp(`^check-receipts: no receipts under qualification/receipts/; ${docling}: incomplete `));
  await commit(bare, { 'verification.json': accepting([fixtureAcceptance(undefined, { decided_by: 'agent' })]) }, 'accepted by an agent before any run');
  await assert.rejects(checkReceipts(bare), /docling-library-qualification: accepted_failures\[0\] is decided by "agent"; only the owner accepts a failing criterion/);
  // An acceptance on a gate that is not the one whose receipt fails changes nothing for that gate.
  await commit(root, { ...receipts, 'verification.json': fixtureRecord({ accepted: { 'mcp-apps': [fixtureAcceptance()] }, construction: [fixtureTracker] }) }, 'accepted on the other gate');
  const elsewhere = await derivedRecord(root);
  assert.equal(elsewhere.gates[0].status, 'failed');
  assert.deepEqual(elsewhere.failures.map(failure => `${failure.name}: ${failure.message}`), [`${apps}: accepted failure corpus/a.pdf/provenance is not failing in ${at('mcp-apps')} (its result there is pass); the acceptance is stale: remove the entry from accepted_failures`]);
});

test('a receipt is rejected when an input changed, its commit is foreign, or its header is incomplete', async t => {
  const { root, sha } = await recorded(t, { statuses: { docling: 'passed' } });
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha) }, 'receipt');
  assert.match(await checkReceipts(root), /1 receipt\(s\) valid against HEAD; /);
  await commit(root, { 'harness/run.mjs': '// v2\n' }, 'change an input');
  await assert.rejects(checkReceipts(root), /docling\.json: inputs changed after [0-9a-f]{40}:\n  harness\/run\.mjs/);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', '0'.repeat(40)) }, 'foreign commit');
  await assert.rejects(checkReceipts(root), /docling\.json: git_sha 0{40} is not an ancestor of HEAD/);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { inputs: [] }) }, 'no inputs');
  await assert.rejects(checkReceipts(root), /docling\.json: inputs must be a non-empty string\[\]/);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', sha, { produced_at: undefined }) }, 'no time');
  await assert.rejects(checkReceipts(root), /docling\.json: produced_at must be an RFC 3339 time/);
  await commit(root, { [at('docling')]: fixtureReceipt('docling', 'abc') }, 'short sha');
  await assert.rejects(checkReceipts(root), /docling\.json: git_sha must be a full 40-hex commit/);
  await commit(root, { [at('docling')]: '{not json' }, 'not json');
  await assert.rejects(checkReceipts(root), /docling\.json: not valid JSON/);
});

test('staleReceiptLines judges the given commit and its receipts, whatever is checked out', async t => {
  const { root, sha } = await recorded(t);
  const early = await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'verification.json': fixtureRecord({ statuses: { docling: 'passed' } }) }, 'record');
  assert.deepEqual(await staleReceiptLines(root, early), []);
  const late = await commit(root, { 'harness/run.mjs': '// v2\n' }, 'change an input');
  const lines = await staleReceiptLines(root, late);
  assert.equal(lines.length, 1);
  assert.match(lines[0], /^stale receipt docling\.json: inputs changed after [0-9a-f]{40}: harness\/run\.mjs -- re-run: bun qualification\/docling\/run\.mjs, then bun qualification\/record\.mjs docling$/);
  // Checked out at the earlier commit, the later one is still stale and the earlier one still valid.
  await git(root, 'checkout', '--quiet', '--detach', early);
  assert.deepEqual(await staleReceiptLines(root, early), []);
  assert.equal((await staleReceiptLines(root, late)).length, 1);
  // Checked out before any receipt existed, receipts are still read from the given commit.
  await git(root, 'checkout', '--quiet', '--detach', sha);
  assert.equal((await staleReceiptLines(root, late)).length, 1);
  assert.deepEqual(await staleReceiptLines(root, sha), [], 'no receipts at that commit');
  await assert.rejects(staleReceiptLines(root, 'f'.repeat(40)), /not a commit/);
});

test('staleReceiptLines names a non-ancestor commit and the harness to re-run', async t => {
  const { root } = await recorded(t, { statuses: { docling: 'passed', 'mcp-apps': 'passed' }, qualified: true });
  const head = await commit(root, { [at('mcp-apps')]: fixtureReceipt('mcp-apps', '0'.repeat(40)), [at('docling')]: fixtureReceipt('docling', '1'.repeat(40)) }, 'foreign');
  const lines = await staleReceiptLines(root, head);
  assert.equal(lines.length, 2);
  assert.match(lines.join('\n'), /^stale receipt docling\.json: git_sha 1{40} is not an ancestor of [0-9a-f]{40} -- re-run: bun qualification\/docling\/run\.mjs, then bun qualification\/record\.mjs docling$/m);
  assert.match(lines.join('\n'), /^stale receipt mcp-apps\.json: .* re-run: bun qualification\/mcp-apps\/run\.mjs, then bun qualification\/record\.mjs mcp-apps$/m);
});

test('staleReceiptLines reports a typed status or phase_0_qualified the receipts at that commit do not derive', async t => {
  const { root, sha } = await recorded(t);
  const typedPass = await commit(root, { 'verification.json': fixtureRecord({ statuses: { docling: 'passed' } }) }, 'hand-edit a status');
  assert.deepEqual(await staleReceiptLines(root, typedPass), [`typed record ${docling}: verification.json at ${typedPass} types status "passed" but the derived status is "incomplete" (${at('docling')} is absent); run \`bun qualification/record.mjs\` to rewrite it`]);
  const typedQualified = await commit(root, { [at('docling')]: fixtureReceipt('docling', sha), 'verification.json': fixtureRecord({ statuses: { docling: 'passed' }, qualified: true }) }, 'hand-edit phase_0_qualified');
  assert.deepEqual(await staleReceiptLines(root, typedQualified), [`typed record phase_0_qualified: verification.json at ${typedQualified} types true but the derived value is false (${apps} is incomplete); run \`bun qualification/record.mjs\` to rewrite it`]);
  // The earlier commit is judged by its own record, not by the one checked out.
  assert.equal((await staleReceiptLines(root, typedPass)).length, 1);
  assert.deepEqual(await staleReceiptLines(root, sha), []);
  // A commit with neither a record nor a receipt has nothing to check; one with a receipt but no record does.
  const { root: bare } = await fixtureRepo(t, { 'harness/run.mjs': '// v1\n' });
  const first = await git(bare, 'rev-parse', 'HEAD');
  assert.deepEqual(await staleReceiptLines(bare, first), []);
  const orphaned = await commit(bare, { [at('docling')]: fixtureReceipt('docling', first) }, 'a receipt without a record');
  assert.deepEqual(await staleReceiptLines(bare, orphaned), [`untrusted receipt docling.json: verification.json is missing at ${orphaned}, so no gate names this receipt -- re-run: rerun the harness that produced it, then bun qualification/record.mjs <name>`]);
  await assert.rejects(checkReceipts(bare), error => error.message === 'check-receipts failed:\nverification.json is missing\ndocling.json: verification.json is missing, so no gate names this receipt');
});
