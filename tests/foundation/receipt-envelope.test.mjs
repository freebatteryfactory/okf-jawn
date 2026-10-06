/** The receipt envelope: one fold decides a result, and a typed result its criteria do not support is named. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { criteriaFailures, criterionResults, envelopeFailures, foldCriteria, receiptResults, statusOf } from '../../scripts/lib/receipt-envelope.mjs';

const gate = 'docling-library-qualification';
const criterion = (id, result, required = true, detail) => ({ id, required, result, ...(detail === undefined ? {} : { detail }) });
const passing = [criterion('corpus/a.pdf/content', 'pass'), criterion('corpus/a.pdf/provenance', 'pass')];
const receipt = (overrides = {}) => ({ gate, result: 'PASS', harness_error: null, criteria: passing, ...overrides });
const pinned = passing.map(entry => entry.id);

test('the result and criterion vocabularies are closed', () => {
  assert.deepEqual([...criterionResults], ['pass', 'fail', 'not_judged', 'not_applicable']);
  assert.deepEqual([...receiptResults], ['PASS', 'FAIL', 'INCOMPLETE']);
  assert.ok(Object.isFrozen(criterionResults) && Object.isFrozen(receiptResults));
});

test('foldCriteria: every required criterion passed and nothing went wrong is the only PASS', () => {
  assert.equal(foldCriteria(passing), 'PASS');
  assert.equal(foldCriteria(passing, null), 'PASS');
  assert.equal(foldCriteria(passing, ''), 'PASS', 'an empty harness error is no harness error');
  assert.equal(foldCriteria([...passing, criterion('corpus/b.docx/page_locator', 'not_applicable', true, 'DOCX has no page locator')]), 'PASS');
});

test('foldCriteria: a required fail is FAIL, even when the harness also reported an error', () => {
  const failed = [passing[0], criterion('corpus/a.pdf/provenance', 'fail')];
  assert.equal(foldCriteria(failed), 'FAIL');
  assert.equal(foldCriteria(failed, 'the browser was missing'), 'FAIL');
  assert.equal(foldCriteria([...failed, criterion('later/check', 'not_judged')]), 'FAIL', 'a fail outranks a criterion that was not judged');
});

test('foldCriteria: a harness error without a required fail is INCOMPLETE', () => {
  assert.equal(foldCriteria(passing, 'the model file was missing'), 'INCOMPLETE');
  assert.equal(foldCriteria([passing[0], criterion('render/present', 'not_judged')], 'no browser'), 'INCOMPLETE');
});

test('foldCriteria: a required criterion that was not judged is INCOMPLETE', () => {
  assert.equal(foldCriteria([passing[0], criterion('render/present', 'not_judged')]), 'INCOMPLETE');
});

test('foldCriteria: no required pass proves nothing and is INCOMPLETE', () => {
  assert.equal(foldCriteria([]), 'INCOMPLETE');
  assert.equal(foldCriteria([criterion('memory/peak', 'pass', false)]), 'INCOMPLETE', 'an optional pass is a measurement, not a verdict');
  assert.equal(foldCriteria([criterion('corpus/b.docx/page_locator', 'not_applicable', true, 'DOCX has no page locator')]), 'INCOMPLETE');
});

test('foldCriteria: an optional fail or an optional not_judged is ignored', () => {
  assert.equal(foldCriteria([...passing, criterion('memory/peak', 'fail', false)]), 'PASS');
  assert.equal(foldCriteria([...passing, criterion('memory/peak', 'not_judged', false)]), 'PASS');
});

test('foldCriteria: a malformed list is INCOMPLETE, whatever it claims', () => {
  for (const malformed of [undefined, null, 'PASS', { id: 'a', required: true, result: 'pass' }]) assert.equal(foldCriteria(malformed), 'INCOMPLETE', JSON.stringify(malformed));
  assert.equal(foldCriteria([...passing, null]), 'INCOMPLETE');
  assert.equal(foldCriteria([...passing, { id: 'x', required: 'yes', result: 'pass' }]), 'INCOMPLETE');
  assert.equal(foldCriteria([...passing, { id: 'x', required: true, result: 'passed' }]), 'INCOMPLETE');
  assert.equal(foldCriteria([...passing, { id: '', required: true, result: 'pass' }]), 'INCOMPLETE');
  assert.equal(foldCriteria([...passing, criterion('x', 'not_applicable')]), 'INCOMPLETE', 'not_applicable without a detail is malformed');
});

test('foldCriteria: a repeated id is INCOMPLETE, so a second entry cannot overwrite a fail', () => {
  assert.equal(foldCriteria([...passing, passing[0]]), 'INCOMPLETE');
  assert.equal(foldCriteria([criterion('a', 'pass'), criterion('a', 'fail')]), 'INCOMPLETE');
});

test('criteriaFailures names each malformed entry by position and says nothing about a well-formed list', () => {
  assert.deepEqual(criteriaFailures(passing), []);
  assert.deepEqual(criteriaFailures([]), []);
  assert.deepEqual(criteriaFailures({}), ['criteria must be an array']);
  assert.deepEqual(criteriaFailures(undefined), ['criteria must be an array']);
  assert.deepEqual(criteriaFailures([null, [], 'pass']), ['criteria[0] must be an object', 'criteria[1] must be an object', 'criteria[2] must be an object']);
  assert.deepEqual(criteriaFailures([{ required: true, result: 'pass' }]), ['criteria[0].id must be a non-empty string']);
  assert.deepEqual(criteriaFailures([{ id: 7, required: true, result: 'pass' }]), ['criteria[0].id must be a non-empty string']);
  assert.deepEqual(criteriaFailures([passing[0], passing[0]]), ['criteria[1].id corpus/a.pdf/content is repeated']);
  assert.deepEqual(criteriaFailures([{ id: 'a', result: 'pass' }]), ['criteria[0].required must be a boolean']);
  assert.deepEqual(criteriaFailures([{ id: 'a', required: true, result: 'PASS' }]), ['criteria[0].result must be one of pass, fail, not_judged, not_applicable']);
  assert.deepEqual(criteriaFailures([{ id: 'a', required: true }]), ['criteria[0].result must be one of pass, fail, not_judged, not_applicable']);
  assert.deepEqual(criteriaFailures([criterion('a', 'not_applicable')]), ['criteria[0] is not_applicable without a detail saying why']);
  assert.deepEqual(criteriaFailures([criterion('a', 'not_applicable', true, '')]), ['criteria[0] is not_applicable without a detail saying why']);
  assert.deepEqual(criteriaFailures([criterion('a', 'not_applicable', true, 'DOCX has no page locator')]), []);
  // Every defect of one entry is reported, not only the first.
  assert.deepEqual(criteriaFailures([{ id: '', required: 1, result: 'no' }]), ['criteria[0].id must be a non-empty string', 'criteria[0].required must be a boolean', 'criteria[0].result must be one of pass, fail, not_judged, not_applicable']);
});

test('envelopeFailures is empty for a receipt whose result is the fold of its criteria and whose pinned ids are all required', () => {
  assert.deepEqual(envelopeFailures(receipt(), gate, pinned), []);
  assert.deepEqual(envelopeFailures(receipt({ git_sha: 'a'.repeat(40), inputs: ['x'], produced_at: '2026-10-05T18:00:00Z', anything_else: {} }), gate, pinned), [], 'the header and the detailed sections are not its concern');
  const failed = [passing[0], criterion('corpus/a.pdf/provenance', 'fail')];
  assert.deepEqual(envelopeFailures(receipt({ result: 'FAIL', criteria: failed }), gate, pinned), [], 'a FAIL receipt is trustworthy evidence too');
  assert.deepEqual(envelopeFailures(receipt({ result: 'INCOMPLETE', harness_error: 'no browser' }), gate, pinned), []);
  assert.deepEqual(envelopeFailures(receipt({ criteria: [...passing, criterion('memory/peak', 'fail', false)] }), gate, pinned), [], 'an unpinned optional measurement may be present');
});

test('envelopeFailures: a receipt for another gate', () => {
  assert.deepEqual(envelopeFailures(receipt({ gate: 'mcp-apps-protocol-qualification' }), gate, pinned), ['gate is "mcp-apps-protocol-qualification", expected "docling-library-qualification"']);
  const { gate: _dropped, ...withoutGate } = receipt();
  assert.deepEqual(envelopeFailures(withoutGate, gate, pinned), ['gate is undefined, expected "docling-library-qualification"']);
});

test('envelopeFailures: a result that is not the fold of the criteria', () => {
  const failed = [passing[0], criterion('corpus/a.pdf/provenance', 'fail')];
  assert.deepEqual(envelopeFailures(receipt({ criteria: failed }), gate, pinned), ['result is PASS but its criteria fold to FAIL']);
  assert.deepEqual(envelopeFailures(receipt({ harness_error: 'no browser' }), gate, pinned), ['result is PASS but its criteria fold to INCOMPLETE']);
  assert.deepEqual(envelopeFailures(receipt({ result: 'FAIL' }), gate, pinned), ['result is FAIL but its criteria fold to PASS'], 'a typed FAIL is as unsupported as a typed PASS');
  assert.deepEqual(envelopeFailures(receipt({ result: 'passed' }), gate, pinned), ['result must be one of PASS, FAIL, INCOMPLETE']);
  const { result: _dropped, ...withoutResult } = receipt();
  assert.deepEqual(envelopeFailures(withoutResult, gate, pinned), ['result must be one of PASS, FAIL, INCOMPLETE']);
});

test('envelopeFailures: a pinned criterion that is missing, or present but not required', () => {
  assert.deepEqual(envelopeFailures(receipt(), gate, [...pinned, 'corpus/a.pdf/tables']), ['pinned criterion corpus/a.pdf/tables is missing']);
  const demoted = [passing[0], criterion('corpus/a.pdf/provenance', 'fail', false)];
  assert.deepEqual(envelopeFailures(receipt({ criteria: demoted }), gate, pinned), ['pinned criterion corpus/a.pdf/provenance is not marked required'], 'a failing check cannot be made optional to pass');
  assert.deepEqual(envelopeFailures(receipt({ criteria: [passing[0]] }), gate, pinned), ['pinned criterion corpus/a.pdf/provenance is missing'], 'a dropped check cannot pass');
});

test('envelopeFailures: not_applicable without a detail, and every other malformed criterion', () => {
  const bare = [...passing, criterion('corpus/b.docx/page_locator', 'not_applicable')];
  assert.deepEqual(envelopeFailures(receipt({ criteria: bare }), gate, pinned), ['criteria[2] is not_applicable without a detail saying why']);
  assert.deepEqual(envelopeFailures(receipt({ criteria: undefined }), gate, pinned), ['criteria must be an array']);
  // A malformed list is reported as such; the pinned ids are not also reported as missing.
  assert.deepEqual(envelopeFailures(receipt({ criteria: [passing[0], passing[0]] }), gate, pinned), ['criteria[1].id corpus/a.pdf/content is repeated']);
});

test('envelopeFailures: a harness_error that is neither null nor a string, and a receipt that is not an object', () => {
  assert.deepEqual(envelopeFailures(receipt({ harness_error: false }), gate, pinned), ['harness_error must be null or a string']);
  const { harness_error: _dropped, ...withoutError } = receipt();
  assert.deepEqual(envelopeFailures(withoutError, gate, pinned), ['harness_error must be null or a string'], 'an absent harness_error is not the same as none');
  for (const value of [null, [], 'PASS', 7]) assert.deepEqual(envelopeFailures(value, gate, pinned), ['receipt must be an object'], JSON.stringify(value));
  // A header-only receipt (the shape before the envelope) is untrustworthy on every count.
  assert.deepEqual(envelopeFailures({ git_sha: 'a'.repeat(40), inputs: ['x'], produced_at: '2026-10-05T18:00:00Z' }, gate, pinned),
    ['gate is undefined, expected "docling-library-qualification"', 'result must be one of PASS, FAIL, INCOMPLETE', 'harness_error must be null or a string', 'criteria must be an array']);
});

test('statusOf maps a receipt result to a gate status, and anything else to incomplete', () => {
  assert.equal(statusOf('PASS'), 'passed');
  assert.equal(statusOf('FAIL'), 'failed');
  assert.equal(statusOf('INCOMPLETE'), 'incomplete');
  for (const value of [undefined, null, '', 'pass', 'passed', 'Pass', 'UNKNOWN', 7]) assert.equal(statusOf(value), 'incomplete', String(value));
  // Not asserted here: a result spelled like an inherited object property ('constructor',
  // 'toString') is not mapped to 'incomplete' by the shared module. derivedRecord
  // (scripts/lib/receipts.mjs) therefore calls statusOf only for a result in receiptResults, and
  // tests/foundation/receipts.test.mjs holds that.
});
