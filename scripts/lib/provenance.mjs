/**
 * Qualification harness provenance.
 *
 * Receipts must cite a clean committed tree. Dirty working trees fail before
 * any receipt is written so a SHA cannot be claimed for uncommitted work.
 *
 * Expected receipt shape under qualification/receipts/*.json (minimum):
 *   { "git_sha": "<40-hex>", "inputs": ["path/relative/to/repo", ...] }
 */

import { spawn } from 'node:child_process';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const defaultRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

function git(args, cwd) {
  return new Promise((resolveGit, reject) => {
    const child = spawn('git', args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolveGit({ stdout, stderr });
      else reject(new Error(`git ${args.join(' ')} exited ${code}: ${stderr.trim() || stdout.trim()}`));
    });
  });
}

/**
 * Fail if the working tree is dirty. Return the HEAD SHA when clean.
 * @param {string} [cwd] repository root
 * @returns {Promise<string>}
 */
export async function requireCleanTree(cwd = defaultRoot) {
  const status = await git(['status', '--porcelain'], cwd);
  if (status.stdout.trim()) {
    throw new Error(
      `Qualification requires a clean git tree before writing a receipt. Dirty paths:\n${status.stdout.trim()}`,
    );
  }
  const head = await git(['rev-parse', 'HEAD'], cwd);
  const sha = head.stdout.trim();
  if (!/^[0-9a-f]{40}$/i.test(sha)) {
    throw new Error(`git rev-parse HEAD did not return a full SHA: ${sha}`);
  }
  return sha;
}
