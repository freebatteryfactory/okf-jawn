/** Committed qualification receipts must describe HEAD, and a qualified Phase 0 must have them. */
import { readdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { run } from './process.mjs';
import { exists } from './files.mjs';

/** Phase 0 gates that are closed by an executed library qualification and its receipt. */
export const libraryGates = Object.freeze(['docling-library-qualification', 'mcp-apps-protocol-qualification']);

const receiptsPath = 'qualification/receipts';

/** Reasons one receipt file cannot be trusted against `head`; empty when it can. `text` is the receipt as written. */
async function receiptFailures(root, name, text, head) {
  let receipt;
  try { receipt = JSON.parse(text); }
  catch (error) { return [`${name}: not valid JSON (${error.message})`]; }
  const sha = receipt.git_sha;
  if (typeof sha !== 'string' || !/^[0-9a-f]{40}$/.test(sha)) return [`${name}: git_sha must be a full 40-hex commit`];
  if (!Array.isArray(receipt.inputs) || !receipt.inputs.length || !receipt.inputs.every(item => typeof item === 'string' && item.length > 0)) {
    return [`${name}: inputs must be a non-empty string[] of repo-relative paths`];
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) return [`${name}: produced_at must be an RFC 3339 time`];
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['merge-base', '--is-ancestor', sha, head])).code !== 0) return [`${name}: git_sha ${sha} is not an ancestor of ${head}`];
  const changed = await git(['diff', '--name-only', sha, head, '--', ...receipt.inputs]);
  if (changed.code !== 0) return [`${name}: could not compare its inputs with ${head} (git exit ${changed.code})`];
  const names = changed.stdout.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return names.length ? [`${name}: inputs changed after ${sha}:\n  ${names.join('\n  ')}`] : [];
}

export async function checkReceipts(root) {
  const verification = JSON.parse(await readFile(join(root, 'verification.json'), 'utf8'));
  const qualified = verification.phase_0_qualified === true;
  const directory = join(root, ...receiptsPath.split('/'));
  const entries = await exists(directory) ? (await readdir(directory)).filter(name => name.endsWith('.json')).sort() : [];
  const failures = [];
  for (const name of entries) failures.push(...await receiptFailures(root, name, await readFile(join(directory, name), 'utf8'), 'HEAD'));
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

/** The harness command that regenerates a receipt, by the harness name in its file name. */
function rerunCommand(name) {
  const harness = ['docling', 'mcp-apps'].find(candidate => name.includes(candidate));
  return harness ? `bun qualification/${harness}/run.mjs, then bun qualification/record.mjs ${harness}` : 'rerun the harness that produced it, then bun qualification/record.mjs <name>';
}

/**
 * One line per receipt that `head` (a commit, evaluated independent of the checkout) cannot
 * trust: stale input, non-ancestor commit or malformed header. Receipts are read from that
 * commit's tree. The Phase 0 gate requirements stay in `checkReceipts`; this is the early
 * warning the pre-push hook shows.
 */
export async function staleReceiptLines(root, head) {
  const git = args => run('git', args, { cwd: root, capture: true, allowFailure: true });
  if ((await git(['rev-parse', '--verify', '--quiet', `${head}^{commit}`])).code !== 0) throw new Error(`check-receipts: ${head} is not a commit in this repository.`);
  const listed = await git(['ls-tree', '--name-only', head, `${receiptsPath}/`]);
  if (listed.code !== 0) throw new Error(`check-receipts: could not list ${receiptsPath}/ at ${head} (git exit ${listed.code})`);
  const names = listed.stdout.split(/\r?\n/).map(line => line.trim()).filter(line => line.endsWith('.json')).map(line => line.slice(receiptsPath.length + 1)).sort();
  const lines = [];
  for (const name of names) {
    const shown = await git(['show', `${head}:${receiptsPath}/${name}`]);
    const failures = shown.code === 0 ? await receiptFailures(root, name, shown.stdout, head) : [`${name}: could not be read at ${head}`];
    if (failures.length) lines.push(`stale receipt ${failures.join(' ').replace(/\s*\n\s*/g, ' ')} -- re-run: ${rerunCommand(name)}`);
  }
  return lines;
}
