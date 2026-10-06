/**
 * Copy finished qualification receipts into qualification/receipts/, then write what the
 * committed receipts derive into verification.json.
 *
 * Usage: bun qualification/record.mjs [<name>...]   (docling | mcp-apps)
 *
 * Each harness needs a clean tree and a recorded receipt is an untracked file, so two
 * harnesses cannot each record on the same commit. Run both first (their receipts
 * land in the ignored .artifacts/qualification/<name>/receipt.json), then record them
 * together. recordReceipt refuses a receipt without the shared header or one that cites
 * a commit other than HEAD.
 *
 * A FAIL or INCOMPLETE receipt is recorded like a PASS: a failed qualification is evidence,
 * and its gate then says `failed`. Nobody types a gate status or `phase_0_qualified`: this
 * script writes both from the receipts (scripts/lib/receipts.mjs derivedRecord), and
 * `bun scripts/dev.mjs check-receipts` fails when the file says anything else. With no name
 * it copies nothing and only rewrites those values.
 */

import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordReceipt } from '../scripts/lib/provenance.mjs';
import { derivedLines, derivedRecord, writeDerivedRecord } from '../scripts/lib/receipts.mjs';

/**
 * Why this receipt must not be recorded, or null. A protocol-only MCP Apps run never rendered
 * a View in the official basic-host, so it is not the gate receipt (the harness refuses
 * `--record` for the same reason; this covers a receipt copied from `.artifacts/`).
 */
export function recordRefusal(name, receipt) {
  if (name !== 'mcp-apps') return null;
  if (receipt.protocol_only === true) return 'a protocol-only MCP Apps receipt is not the gate receipt; rerun without OKF_MCP_APPS_PROTOCOL_ONLY';
  if (!receipt.basic_host || receipt.basic_host.status === 'not_run') return 'the MCP Apps receipt basic-host section was skipped; rerun the full qualification (`bun scripts/dev.mjs qualify mcp-apps`)';
  return null;
}

if (import.meta.main) {
  const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
  const names = process.argv.slice(2);
  // The harnesses are the ones the Phase 0 gates of kind receipt name in verification.json.
  const known = (await derivedRecord(root)).gates.map((gate) => gate.harness);
  if (names.some((name) => !known.includes(name))) {
    throw new Error(`Usage: bun qualification/record.mjs [<name>...] where name is one of ${known.join(', ')}; with no name only the derived statuses are rewritten`);
  }
  for (const name of names) {
    const source = join(root, '.artifacts', 'qualification', name, 'receipt.json');
    const receipt = JSON.parse((await readFile(source, 'utf8')).replace(/^\uFEFF/, ''));
    const refusal = recordRefusal(name, receipt);
    if (refusal) throw new Error(`record refused for ${name}: ${refusal}`);
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
