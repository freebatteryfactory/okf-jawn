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
 * under qualification/receipts/; recordReceipt is the only writer of that directory, and
 * `bun qualification/record.mjs` is its only caller: a harness writes its receipt under
 * .artifacts/ and records nothing. It copies a receipt of any result; the gate status that
 * follows from it is derived by scripts/lib/receipts.mjs and written by record.mjs, never typed.
 */

import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
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
 * Why recordReceipt would refuse this receipt under this name; empty when it would copy it.
 * The tree need not be clean (an earlier recorded receipt is itself untracked), but HEAD must
 * still be the commit the receipt cites.
 * @param {string} root repository root
 * @param {string} name receipt name, e.g. "docling"
 * @param {unknown} receipt
 * @returns {Promise<string[]>}
 */
export async function recordProblems(root, name, receipt) {
  if (typeof name !== 'string' || !RECEIPT_NAME.test(name)) {
    return [`name ${JSON.stringify(name)} must be lower-case words joined by hyphens`];
  }
  const problems = receiptHeaderProblems(receipt);
  if (problems.length) return problems;
  const head = (await git(['rev-parse', 'HEAD'], root)).stdout.trim();
  return head === receipt.git_sha ? [] : [`receipt cites ${receipt.git_sha} but HEAD is ${head}; requalify on the current commit`];
}

/**
 * Copy a finished receipt to qualification/receipts/<name>.json; throws with recordProblems
 * when there are any.
 * @param {string} root repository root
 * @param {string} name receipt name, e.g. "docling"
 * @param {object} receipt finished receipt with the shared header
 * @returns {Promise<string>} the written path
 */
export async function recordReceipt(root, name, receipt) {
  const problems = await recordProblems(root, name, receipt);
  if (problems.length) throw new Error(`recordReceipt(${name}): ${problems.join('; ')}`);
  const target = join(root, 'qualification', 'receipts', `${name}.json`);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, `${JSON.stringify(receipt, null, 2)}\n`);
  return target;
}

/**
 * The path rule of every receipt. A receipt is committed to a public repository, so it carries
 * no path that names this machine or its user. scrubReceiptPaths is the single place that
 * rewrites them, applied by each harness to its finished receipt; the harness binaries and
 * the evidence files under .artifacts/ keep their raw paths.
 *   inside the repository       -> repo-relative, `/` separators
 *   inside the home directory   -> `~/...`
 *   inside the temp directory   -> `<tmp>/...`
 *   any other absolute path     -> `<abs>/<last two segments>`, also listed in `paths.unmapped`
 * The longest matching root wins, so a worktree or temp directory under home is not `~`.
 * A root matches only on a segment boundary: `C:\Users\eayou2` is not inside `C:\Users\eayou`.
 * Only path text changes; hashes, lengths and every other value are copied as they are.
 */

const WINDOWS_PATH = /(?:\\{2,4}\?\\{1,2})?(?<![A-Za-z0-9])[A-Za-z]:(?:\\{1,4}|\/(?!\/))[^\s"'<>|*?`,;]*/g;
const POSIX_ROOTS = ['home', 'Users', 'tmp', 'root', 'var', 'private', 'opt', 'mnt'];
const TRAILING_PUNCTUATION = /[.:)\]}]+$/;

/** Windows drive paths are compared without case, POSIX paths with it. */
const folded = (path) => (/^[A-Za-z]:/.test(path) ? path.toLowerCase() : path);

/** `path` with `/` separators, no verbatim prefix and no trailing separator. */
function slashed(path) {
  const plain = path.replace(/^\\{2,4}\?\\{1,2}/, '').replace(/\\+/g, '/').replace(/\/+/g, '/');
  return plain.length > 1 ? plain.replace(/\/$/, '') : plain;
}

/**
 * The roots a receipt path is judged against.
 * @param {{ root: string, home?: string, tmp?: string }} roots absolute directories; home and
 *   tmp default to this machine's
 */
export function pathContext({ root, home = homedir(), tmp = tmpdir() }) {
  const entries = [
    { root: slashed(root), label: null },
    { root: slashed(home), label: '~' },
    { root: slashed(tmp), label: '<tmp>' },
  ];
  return { entries: entries.map((entry) => ({ ...entry, key: folded(entry.root) })) };
}

/**
 * The receipt form of one absolute path, as `{ path, mapped }`; `mapped` is false when the path
 * is in none of the known roots and only its last two segments are kept.
 */
export function mapReceiptPath(path, context) {
  const plain = slashed(path);
  const lower = folded(plain);
  let best = null;
  for (const entry of context.entries) {
    const inside = lower === entry.key || lower.startsWith(`${entry.key}/`);
    if (inside && (best === null || entry.key.length > best.key.length)) best = entry;
  }
  if (best !== null) {
    const rest = plain.slice(best.root.length).replace(/^\//, '');
    if (best.label === null) return { path: rest === '' ? '.' : rest, mapped: true };
    return { path: rest === '' ? best.label : `${best.label}/${rest}`, mapped: true };
  }
  const segments = plain.split('/').filter((part) => part !== '' && !/^[A-Za-z]:$/.test(part));
  return { path: `<abs>/${segments.slice(-2).join('/')}`, mapped: false };
}

function posixPath(context) {
  const roots = context.entries.filter((entry) => entry.root.startsWith('/')).map((entry) => entry.root.slice(1).split('/')[0]);
  const names = [...new Set([...POSIX_ROOTS, ...roots])].map((name) => name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  return new RegExp(`(?<![\\w.\\-:/~])/(?:${names.join('|')})(?:/[^\\s"'<>|*?\`,;]*)?`, 'g');
}

/** `text` with every absolute path in it rewritten; unmapped results are added to `unmapped`. */
function scrubText(text, context, posix, unmapped) {
  const rewrite = (token) => {
    const trailing = token.match(TRAILING_PUNCTUATION)?.[0] ?? '';
    const found = mapReceiptPath(trailing ? token.slice(0, -trailing.length) : token, context);
    if (!found.mapped) unmapped.add(found.path);
    return `${found.path}${trailing}`;
  };
  return text.replace(WINDOWS_PATH, rewrite).replace(posix, rewrite);
}

/**
 * A copy of `receipt` with every path string, in values and in keys, rewritten by the path
 * rule, and `paths.unmapped` (sorted, possibly empty, keeping what an earlier pass listed) listing
 * the paths that fit no root.
 * @param {object} receipt
 * @param {ReturnType<typeof pathContext>} context
 */
export function scrubReceiptPaths(receipt, context) {
  const posix = posixPath(context);
  const unmapped = new Set();
  const walk = (value) => {
    if (typeof value === 'string') return scrubText(value, context, posix, unmapped);
    if (Array.isArray(value)) return value.map(walk);
    if (value !== null && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [scrubText(key, context, posix, unmapped), walk(item)]));
    }
    return value;
  };
  const scrubbed = walk(receipt);
  const earlier = Array.isArray(scrubbed.paths?.unmapped) ? scrubbed.paths.unmapped : [];
  return { ...scrubbed, paths: { ...(scrubbed.paths ?? {}), unmapped: [...new Set([...earlier, ...unmapped])].sort() } };
}
