/**
 * The one way a qualification receipt is recorded: copy finished receipts into
 * qualification/receipts/, then write what the committed receipts derive into verification.json.
 *
 * Usage: bun qualification/record.mjs [<name>...]   (docling | mcp-apps)
 *
 * Each harness needs a clean tree and a recorded receipt is an untracked file, so two
 * harnesses cannot each record on the same commit. Run both first (their receipts
 * land in the ignored .artifacts/qualification/<name>/receipt.json), then record them
 * together. A harness has no flag that records: copying a receipt without writing the derived
 * statuses left a tree `check-receipts` rejects, so there is this command and no other.
 *
 * A receipt is refused for one reason only: it cannot be trusted. Its header must be the shared
 * one and cite HEAD (scripts/lib/provenance.mjs recordProblems), and its envelope must be clean
 * against its gate's tracked criteria.json (scripts/lib/receipts.mjs recordFailures: the result
 * is the fold of the criteria, every pinned criterion is present and required, `not_judged` is
 * the derived list). What the result is, and which sections of the run were reached, is never a
 * reason: a FAIL or INCOMPLETE receipt is evidence, is recorded like a PASS, and its gate then
 * says `failed` or `incomplete`. When any named receipt is refused, none is copied.
 *
 * Nobody types a gate status or `phase_0_qualified`: this script writes both from the receipts
 * (scripts/lib/receipts.mjs derivedRecord), and `bun scripts/dev.mjs check-receipts` fails when
 * the file says anything else. With no name it copies nothing and only rewrites those values.
 * That is also how a gate is written back to `incomplete` when an input of its recorded receipt
 * changed: the stale receipt stays where it is, is named on stdout with the harness to re-run,
 * and qualifies nothing until that harness is run and recorded again.
 *
 * What this command refuses it says in one sentence each on stderr, with exit 1: a name that is
 * no harness, a receipt that is missing (with the command that writes it) or cannot be trusted,
 * a record that cannot be rewritten.
 */

import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordProblems, recordReceipt } from '../scripts/lib/provenance.mjs';
import { derivedLines, derivedRecord, recordFailures, staleLines, writeDerivedRecord } from '../scripts/lib/receipts.mjs';

/**
 * Why the receipt of harness `name` must not be recorded in the repository at `root`, or null.
 * The reason is always about trust in the receipt, never about its result.
 */
export async function recordRefusal(root, name, receipt) {
  const failures = await recordFailures(root, name, receipt);
  return failures.length ? `its envelope cannot be trusted (${failures.join('; ')}); rerun the harness on a clean tree` : null;
}

/**
 * Something this command expects and says in words: a name that is no harness, a receipt that
 * is missing or refused, a record that cannot be rewritten. Its message is everything the user
 * is shown; any other error is a defect of this script and keeps its stack.
 */
class Refused extends Error {}

/**
 * The finished receipt of harness `name` under .artifacts/, or the sentence that says why there
 * is none to record: `{ receipt }` or `{ refusal }`.
 */
async function finishedReceipt(root, name) {
  const path = `.artifacts/qualification/${name}/receipt.json`;
  const produce = `\`bun qualification/${name}/run.mjs\` writes it, on a clean tree at this commit`;
  let text;
  try {
    text = await readFile(join(root, ...path.split('/')), 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    return { refusal: `${path} is missing; ${produce}` };
  }
  try {
    return { receipt: JSON.parse(text.replace(/^\uFEFF/, '')) };
  } catch (error) {
    return { refusal: `${path} is not valid JSON (${error.message}); ${produce}` };
  }
}

async function record(root, names) {
  // The harnesses are the ones the Phase 0 gates of kind receipt name in verification.json.
  const known = (await derivedRecord(root)).gates.map((gate) => gate.harness);
  const unknown = names.filter((name) => !known.includes(name));
  if (unknown.length) {
    throw new Refused(`${unknown.join(', ')}: no harness of that name. Usage: bun qualification/record.mjs [<name>...] where name is one of ${known.join(', ')}; with no name only the derived statuses are rewritten`);
  }
  // Every named receipt is judged before any is copied.
  const finished = [];
  const refusals = [];
  for (const name of names) {
    const { receipt, refusal: absent } = await finishedReceipt(root, name);
    if (absent) {
      refusals.push(`record refused for ${name}: ${absent}`);
      continue;
    }
    const problems = await recordProblems(root, name, receipt);
    const refusal = problems.length ? problems.join('; ') : await recordRefusal(root, name, receipt);
    if (refusal) refusals.push(`record refused for ${name}: ${refusal}`);
    finished.push({ name, receipt });
  }
  if (refusals.length) throw new Refused(`${refusals.join('\n')}\nNothing was copied.`);
  for (const { name, receipt } of finished) {
    const target = await recordReceipt(root, name, receipt);
    process.stdout.write(`recorded ${join(root, '.artifacts', 'qualification', name, 'receipt.json')} -> ${target}\n`);
  }
  let written;
  try {
    written = await writeDerivedRecord(root);
  } catch (error) {
    throw new Refused(`verification.json was not rewritten: ${error.message}`);
  }
  const { changed, derived } = written;
  for (const line of derivedLines(derived)) process.stdout.write(`${line}\n`);
  // A stale receipt is not refused and not removed: its gate is written back to incomplete.
  for (const line of staleLines(derived)) process.stdout.write(`stale: ${line.replace(/\s*\n\s*/g, ' ')}\n`);
  process.stdout.write(changed ? 'verification.json rewritten from the receipts.\n' : 'verification.json already agrees with the receipts.\n');
  if (derived.failures.length) {
    process.stderr.write(`${derived.failures.map((failure) => `${failure.name}: ${failure.message}`).join('\n')}\n`);
    process.exitCode = 1;
  }
}

if (import.meta.main) {
  try {
    await record(resolve(fileURLToPath(new URL('..', import.meta.url))), process.argv.slice(2));
  } catch (error) {
    if (!(error instanceof Refused)) throw error;
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
