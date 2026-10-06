/**
 * Copy finished qualification receipts into qualification/receipts/.
 *
 * Usage: bun qualification/record.mjs <name>...   (docling | mcp-apps)
 *
 * Each harness needs a clean tree and a recorded receipt is an untracked file, so two
 * harnesses cannot each record on the same commit. Run both first (their receipts
 * land in the ignored .artifacts/qualification/<name>/receipt.json), then record them
 * together. recordReceipt refuses a receipt without the shared header or one that cites
 * a commit other than HEAD.
 */

import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordReceipt } from '../scripts/lib/provenance.mjs';

const KNOWN = ['docling', 'mcp-apps'];

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
  if (names.length === 0 || names.some((name) => !KNOWN.includes(name))) {
    throw new Error(`Usage: bun qualification/record.mjs <name>... where name is one of ${KNOWN.join(', ')}`);
  }
  for (const name of names) {
    const source = join(root, '.artifacts', 'qualification', name, 'receipt.json');
    const receipt = JSON.parse((await readFile(source, 'utf8')).replace(/^\uFEFF/, ''));
    const refusal = recordRefusal(name, receipt);
    if (refusal) throw new Error(`record refused for ${name}: ${refusal}`);
    const target = await recordReceipt(root, name, receipt);
    process.stdout.write(`recorded ${source} -> ${target}
`);
  }
}
