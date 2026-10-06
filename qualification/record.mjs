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
 */

import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordProblems, recordReceipt } from '../scripts/lib/provenance.mjs';
import { derivedLines, derivedRecord, recordFailures, writeDerivedRecord } from '../scripts/lib/receipts.mjs';

/**
 * Why the receipt of harness `name` must not be recorded in the repository at `root`, or null.
 * The reason is always about trust in the receipt, never about its result.
 */
export async function recordRefusal(root, name, receipt) {
  const failures = await recordFailures(root, name, receipt);
  return failures.length ? `its envelope cannot be trusted (${failures.join('; ')}); rerun the harness on a clean tree` : null;
}

if (import.meta.main) {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const names = process.argv.slice(2);
  // The harnesses are the ones the Phase 0 gates of kind receipt name in verification.json.
  const known = (await derivedRecord(root)).gates.map((gate) => gate.harness);
  if (names.some((name) => !known.includes(name))) {
    throw new Error(`Usage: bun qualification/record.mjs [<name>...] where name is one of ${known.join(', ')}; with no name only the derived statuses are rewritten`);
  }
  // Every named receipt is judged before any is copied.
  const finished = [];
  const refusals = [];
  for (const name of names) {
    const source = join(root, '.artifacts', 'qualification', name, 'receipt.json');
    const receipt = JSON.parse((await readFile(source, 'utf8')).replace(/^﻿/, ''));
    const problems = await recordProblems(root, name, receipt);
    const refusal = problems.length ? problems.join('; ') : await recordRefusal(root, name, receipt);
    if (refusal) refusals.push(`record refused for ${name}: ${refusal}`);
    finished.push({ name, source, receipt });
  }
  if (refusals.length) throw new Error(`${refusals.join('\n')}\nNothing was copied.`);
  for (const { name, source, receipt } of finished) {
    const target = await recordReceipt(root, name, receipt);
    process.stdout.write(`recorded ${source} -> ${target}\n`);
  }
  const { changed, derived } = await writeDerivedRecord(root);
  for (const line of derivedLines(derived)) process.stdout.write(`${line}\n`);
  process.stdout.write(changed ? 'verification.json rewritten from the receipts.\n' : 'verification.json already agrees with the receipts.\n');
  if (derived.failures.length) {
    process.stderr.write(`${derived.failures.map((failure) => `${failure.name}: ${failure.message}`).join('\n')}\n`);
    process.exitCode = 1;
  }
}
