/**
 * Qualification harness provenance.
 *
 * Receipts must cite a clean committed tree. Dirty working trees fail before
 * any receipt is written so a SHA cannot be claimed for uncommitted work.
 *
 * Every receipt starts with the same top-level header, produced only by receiptHeader:
 *   { "git_sha": "<40-hex>", "inputs": ["path/relative/to/repo", ...], "produced_at": "<ISO-8601 UTC>" }
 * produced_at is the moment the clean tree was observed, before the harness ran.
 * `bun scripts/dev.mjs check-receipts` validates git_sha and inputs of every file
 * under qualification/receipts/; recordReceipt is the only writer of that directory.
 */

import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const defaultRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const FULL_SHA = /^[0-9a-f]{40}$/i;
const RECEIPT_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

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
 * Any porcelain output is dirty; any non-zero git exit rejects.
 * @param {string} [cwd] repository root
 * @returns {Promise<string>}
 */
export async function requireCleanTree(cwd = defaultRoot) {
  // --untracked-files=all: a user-level status.showUntrackedFiles=no must not hide new files.
  const status = await git(['status', '--porcelain', '--untracked-files=all'], cwd);
  if (status.stdout.trim()) {
    throw new Error(
      `Qualification requires a clean git tree before writing a receipt. Dirty paths:\n${status.stdout.trim()}`,
    );
  }
  const head = await git(['rev-parse', 'HEAD'], cwd);
  const sha = head.stdout.trim();
  if (!FULL_SHA.test(sha)) {
    throw new Error(`git rev-parse HEAD did not return a full SHA: ${sha}`);
  }
  return sha;
}

function inputProblem(input) {
  if (typeof input !== 'string' || input.length === 0) return 'must be a non-empty string';
  if (input.includes('\\')) return 'must use forward slashes';
  if (input.startsWith('/') || /^[A-Za-z]:/.test(input)) {
    return 'must be relative to the repository root';
  }
  if (input.split('/').some((part) => part === '' || part === '.' || part === '..')) {
    return 'must not contain empty, "." or ".." segments';
  }
  return null;
}

/**
 * The header every qualification receipt carries at its top level.
 * @param {string} root repository root
 * @param {string[]} inputs repo-relative paths whose later change invalidates the receipt
 * @returns {Promise<{ git_sha: string, inputs: string[], produced_at: string }>}
 */
export async function receiptHeader(root, inputs) {
  if (!Array.isArray(inputs) || inputs.length === 0) {
    throw new Error('receiptHeader: inputs must be a non-empty array of repo-relative paths');
  }
  for (const input of inputs) {
    const problem = inputProblem(input);
    if (problem) throw new Error(`receiptHeader: input ${JSON.stringify(input)} ${problem}`);
  }
  const git_sha = await requireCleanTree(root);
  for (const input of inputs) {
    try {
      await git(['cat-file', '-e', `${git_sha}:${input}`], root);
    } catch {
      throw new Error(
        `receiptHeader: input ${input} does not exist at ${git_sha}; check-receipts could never see it change`,
      );
    }
  }
  return { git_sha, inputs: [...inputs], produced_at: new Date().toISOString() };
}

/**
 * Why `receipt` does not carry the shared header at its top level; empty when it does.
 * @param {unknown} receipt
 * @returns {string[]}
 */
export function receiptHeaderProblems(receipt) {
  if (receipt === null || typeof receipt !== 'object' || Array.isArray(receipt)) {
    return ['receipt must be a JSON object'];
  }
  const problems = [];
  if (typeof receipt.git_sha !== 'string' || !FULL_SHA.test(receipt.git_sha)) {
    problems.push('git_sha must be a 40-hex commit SHA at the top level');
  }
  if (
    !Array.isArray(receipt.inputs) ||
    receipt.inputs.length === 0 ||
    !receipt.inputs.every((item) => typeof item === 'string')
  ) {
    problems.push('inputs must be a non-empty string[] of repo-relative paths at the top level');
  }
  if (typeof receipt.produced_at !== 'string' || Number.isNaN(Date.parse(receipt.produced_at))) {
    problems.push('produced_at must be an ISO-8601 timestamp at the top level');
  }
  return problems;
}

/**
 * Copy a finished receipt to qualification/receipts/<name>.json.
 * The tree need not be clean here (an earlier recorded receipt is itself untracked),
 * but HEAD must still be the commit the receipt cites.
 * @param {string} root repository root
 * @param {string} name receipt name, e.g. "docling"
 * @param {object} receipt finished receipt with the shared header
 * @returns {Promise<string>} the written path
 */
export async function recordReceipt(root, name, receipt) {
  if (typeof name !== 'string' || !RECEIPT_NAME.test(name)) {
    throw new Error(`recordReceipt: name ${JSON.stringify(name)} must be lower-case words joined by hyphens`);
  }
  const problems = receiptHeaderProblems(receipt);
  if (problems.length) throw new Error(`recordReceipt(${name}): ${problems.join('; ')}`);
  const head = (await git(['rev-parse', 'HEAD'], root)).stdout.trim();
  if (head !== receipt.git_sha) {
    throw new Error(
      `recordReceipt(${name}): receipt cites ${receipt.git_sha} but HEAD is ${head}; requalify on the current commit`,
    );
  }
  const target = join(root, 'qualification', 'receipts', `${name}.json`);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, `${JSON.stringify(receipt, null, 2)}\n`);
  return target;
}
