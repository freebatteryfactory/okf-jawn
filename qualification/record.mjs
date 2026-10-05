/**
 * Copy finished qualification receipts into qualification/receipts/.
 *
 * Usage: bun qualification/record.mjs <name>...   (docling | mcp-apps | iii)
 *
 * Each harness needs a clean tree and a recorded receipt is an untracked file, so three
 * harnesses cannot each record on the same commit. Run all three first (their receipts
 * land in the ignored .artifacts/qualification/<name>/receipt.json), then record them
 * together. recordReceipt refuses a receipt without the shared header or one that cites
 * a commit other than HEAD.
 */

import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { recordReceipt } from '../scripts/lib/provenance.mjs';

const KNOWN = ['docling', 'mcp-apps', 'iii'];
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const names = process.argv.slice(2);

if (names.length === 0 || names.some((name) => !KNOWN.includes(name))) {
  throw new Error(`Usage: bun qualification/record.mjs <name>... where name is one of ${KNOWN.join(', ')}`);
}
for (const name of names) {
  const source = join(root, '.artifacts', 'qualification', name, 'receipt.json');
  const receipt = JSON.parse((await readFile(source, 'utf8')).replace(/^\uFEFF/, ''));
  const target = await recordReceipt(root, name, receipt);
  process.stdout.write(`recorded ${source} -> ${target}\n`);
}
