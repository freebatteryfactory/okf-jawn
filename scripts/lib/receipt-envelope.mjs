/**
 * One receipt shape and one fold for every qualification harness.
 *
 * A harness judges criteria; it never types its own result. `foldCriteria` is the only place a
 * result is decided, and `check-receipts` runs it again over a committed receipt, so a result
 * that its criteria do not support cannot be recorded.
 *
 * An environment failure (missing browser, missing model file, a build that did not finish) is
 * a harness error, never a product verdict: it folds to INCOMPLETE unless a required criterion
 * had already failed.
 */

/** What one criterion can report. `not_applicable` must carry a `detail` saying why. */
export const criterionResults = Object.freeze(['pass', 'fail', 'not_judged', 'not_applicable']);

/** What a receipt can report. */
export const receiptResults = Object.freeze(['PASS', 'FAIL', 'INCOMPLETE']);

const gateStatuses = Object.freeze({ PASS: 'passed', FAIL: 'failed', INCOMPLETE: 'incomplete' });

/** The gate status a receipt result supports; a gate without a receipt is `incomplete`. */
export function statusOf(result) {
  return gateStatuses[result] ?? 'incomplete';
}

/**
 * The exit code of a harness run that wrote a receipt, by the receipt's result. Every harness
 * ends with these and no other numbers, so a caller reads one convention.
 */
export const EXIT_CODES = Object.freeze({ PASS: 0, FAIL: 2, INCOMPLETE: 3 });

/**
 * The exit code of a run that was refused before it started and wrote no receipt (a dirty
 * tree, an unknown option). It is what an uncaught error exits with, and no result has it.
 */
export const EXIT_REFUSED = 1;

/** The exit code for a receipt result; anything that is not a known result is INCOMPLETE. */
export function exitCodeFor(result) {
  return Object.hasOwn(EXIT_CODES, result) ? EXIT_CODES[result] : EXIT_CODES.INCOMPLETE;
}

/** Reasons `criteria` is not a well-formed list; empty when it is. */
export function criteriaFailures(criteria) {
  if (!Array.isArray(criteria)) return ['criteria must be an array'];
  const failures = [];
  const seen = new Set();
  for (const [index, criterion] of criteria.entries()) {
    const where = `criteria[${index}]`;
    if (criterion === null || typeof criterion !== 'object' || Array.isArray(criterion)) { failures.push(`${where} must be an object`); continue; }
    if (typeof criterion.id !== 'string' || criterion.id.length === 0) failures.push(`${where}.id must be a non-empty string`);
    else if (seen.has(criterion.id)) failures.push(`${where}.id ${criterion.id} is repeated`);
    else seen.add(criterion.id);
    if (typeof criterion.required !== 'boolean') failures.push(`${where}.required must be a boolean`);
    if (!criterionResults.includes(criterion.result)) failures.push(`${where}.result must be one of ${criterionResults.join(', ')}`);
    if (criterion.result === 'not_applicable' && (typeof criterion.detail !== 'string' || criterion.detail.length === 0)) failures.push(`${where} is not_applicable without a detail saying why`);
  }
  return failures;
}

/**
 * The result a list of criteria supports.
 *
 * FAIL when a required criterion failed. Otherwise INCOMPLETE when the harness itself failed
 * (`harnessError` is a non-empty string), when a required criterion was not judged, when the
 * list is malformed, or when no required criterion passed (nothing was proven). Otherwise PASS.
 */
export function foldCriteria(criteria, harnessError = null) {
  if (criteriaFailures(criteria).length > 0) return 'INCOMPLETE';
  const required = criteria.filter(criterion => criterion.required);
  if (required.some(criterion => criterion.result === 'fail')) return 'FAIL';
  if (typeof harnessError === 'string' && harnessError.length > 0) return 'INCOMPLETE';
  if (required.some(criterion => criterion.result === 'not_judged')) return 'INCOMPLETE';
  if (!required.some(criterion => criterion.result === 'pass')) return 'INCOMPLETE';
  return 'PASS';
}

/**
 * Reasons a receipt's envelope cannot be trusted; empty when it can.
 *
 * `gate` is the verification.json gate the receipt claims to close. `pinned` is the list of
 * criterion ids that gate requires (from the harness's tracked criteria.json): each must be
 * present and marked required, so a harness edit cannot drop a criterion and still pass.
 */
export function envelopeFailures(receipt, gate, pinned) {
  if (receipt === null || typeof receipt !== 'object' || Array.isArray(receipt)) return ['receipt must be an object'];
  const failures = [];
  if (receipt.gate !== gate) failures.push(`gate is ${JSON.stringify(receipt.gate)}, expected ${JSON.stringify(gate)}`);
  if (!receiptResults.includes(receipt.result)) failures.push(`result must be one of ${receiptResults.join(', ')}`);
  if (receipt.harness_error !== null && typeof receipt.harness_error !== 'string') failures.push('harness_error must be null or a string');
  const malformed = criteriaFailures(receipt.criteria);
  failures.push(...malformed);
  if (malformed.length === 0) {
    const folded = foldCriteria(receipt.criteria, receipt.harness_error);
    if (receiptResults.includes(receipt.result) && receipt.result !== folded) failures.push(`result is ${receipt.result} but its criteria fold to ${folded}`);
    const byId = new Map(receipt.criteria.map(criterion => [criterion.id, criterion]));
    for (const id of pinned) {
      const criterion = byId.get(id);
      if (!criterion) failures.push(`pinned criterion ${id} is missing`);
      else if (!criterion.required) failures.push(`pinned criterion ${id} is not marked required`);
    }
  }
  return failures;
}
