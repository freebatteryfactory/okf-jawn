/** Committed qualification receipts must describe HEAD, and a qualified Phase 0 must have them. */
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { run } from './process.mjs';
import { exists } from './files.mjs';

/** Phase 0 gates that are closed by an executed library qualification and its receipt. */
export const libraryGates = Object.freeze(['docling-library-qualification', 'iii-library-qualification', 'mcp-apps-protocol-qualification']);

const receiptsPath = 'qualification/receipts';

/** Reasons one receipt file cannot be trusted against HEAD; empty when it can. */
async function receiptFailures(root, directory, name) {
  let receipt;
  try { receipt = JSON.parse(await readFile(join(directory, name), 'utf8')); }
  catch (error) { return [`${name}: not valid JSON (${error.message})`]; }
  const sha = receipt.git_sha;
  if (typeof sha !== 'string' || !/^[0-9a-f]{40}$/.test(sha)) return [`${name}: git_sha must be a full 40-hex commit`];
  if (!Array.isArray(receipt.inputs) || !receipt.inputs.length || !receipt.inputs.every(item => typeof item === 'string' && item.length > 0)) {
    return [`${name}: inputs must be a non-empty string[] of repo-relative paths`];
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) return [`${name}: produced_at must be an RFC 3339 time`];
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['merge-base', '--is-ancestor', sha, 'HEAD'])).code !== 0) return [`${name}: git_sha ${sha} is not an ancestor of HEAD`];
  const changed = await git(['diff', '--name-only', sha, 'HEAD', '--', ...receipt.inputs]);
  if (changed.code !== 0) return [`${name}: could not compare its inputs with HEAD (git exit ${changed.code})`];
  const names = changed.stdout.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return names.length ? [`${name}: inputs changed after ${sha}:\n  ${names.join('\n  ')}`] : [];
}

export async function checkReceipts(root) {
  const verification = JSON.parse(await readFile(join(root, 'verification.json'), 'utf8'));
  const qualified = verification.phase_0_qualified === true;
  const directory = join(root, ...receiptsPath.split('/'));
  const entries = await exists(directory) ? (await readdir(directory)).filter(name => name.endsWith('.json')).sort() : [];
  const failures = [];
  for (const name of entries) failures.push(...await receiptFailures(root, directory, name));
  if (qualified) {
    const gates = verification.current?.gates?.phase_0 ?? [];
    for (const id of libraryGates) {
      const gate = gates.find(entry => entry.id === id);
      if (!gate) { failures.push(`${id}: phase_0_qualified is true but verification.json has no such Phase 0 gate`); continue; }
      const file = typeof gate.receipt === 'string' && gate.receipt.startsWith(`${receiptsPath}/`) ? gate.receipt.slice(receiptsPath.length + 1) : null;
      if (!file || file.includes('/') || !entries.includes(file)) {
        failures.push(`${id}: phase_0_qualified is true but its receipt (${gate.receipt ?? 'null'}) is not a file under ${receiptsPath}/`);
      }
    }
  }
  if (failures.length) throw new Error(`check-receipts failed:\n${failures.join('\n')}`);
  if (!entries.length) return `check-receipts: no receipts under ${receiptsPath}/; accepted only because verification.json records phase_0_qualified false.`;
  return `check-receipts: ${entries.length} receipt(s) valid against HEAD${qualified ? `; all ${libraryGates.length} Phase 0 library gates have one` : ''}.`;
}
