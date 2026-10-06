/** Disposable Git repositories for tooling tests; never the real checkout. */
import { cp, mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { gitLocalEnvironment, run } from '../../scripts/lib/process.mjs';
import { foldCriteria } from '../../scripts/lib/receipt-envelope.mjs';

// A second guard beside dev.mjs: no fixture command may inherit a hook's repository.
for (const name of gitLocalEnvironment) delete process.env[name];

const identity = ['-c', 'user.name=okf-jawn test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false'];

export async function git(cwd, ...args) {
  return (await run('git', [...identity, ...args], { cwd, capture: true })).stdout.trim();
}

export async function commit(root, files, message) {
  for (const [path, content] of Object.entries(files)) {
    await mkdir(dirname(join(root, path)), { recursive: true });
    await writeFile(join(root, path), content);
  }
  await git(root, 'add', '--all');
  await git(root, 'commit', '--quiet', '-m', message);
  return git(root, 'rev-parse', 'HEAD');
}

export async function fixtureRepo(t, files = { 'README.md': 'fixture\n' }) {
  const base = await scratch(t);
  const root = join(base, 'repo');
  await mkdir(root);
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await commit(root, files, 'initial');
  return { base, root };
}

/** A scratch directory removed when `t` ends. Cleanup is best effort; a locked file there is not a test failure. */
async function scratch(t) {
  const base = await realpath(await mkdtemp(join(tmpdir(), 'okf-repo-')));
  t.after(() => rm(base, { recursive: true, force: true, maxRetries: 3 }).catch(() => {}));
  return base;
}

/** A copy of the repository at `source` in a scratch directory private to `t`, as `{ base, root }`; what the copy does never reaches `source`. */
export async function copyRepo(t, source) {
  const base = await scratch(t);
  const root = join(base, 'repo');
  await cp(source, root, { recursive: true });
  return { base, root };
}

/**
 * Prepared repositories for one test file: each distinct set of files is initialised and committed
 * once, then the returned `fixtureRepo` copies the result for every test, so a test may commit,
 * check out and write in its repository and no test sees another's. Call `dispose` once, after the
 * file's last test (an `afterAll`); the prepared originals are only ever read, never handed to a test.
 */
export function sharedRepos() {
  const bases = new Set();
  const prepared = new Map();
  const prepare = async files => {
    const base = await realpath(await mkdtemp(join(tmpdir(), 'okf-prepared-')));
    bases.add(base);
    const root = join(base, 'repo');
    await mkdir(root);
    await git(root, 'init', '--quiet', '--initial-branch=main');
    await commit(root, files, 'initial');
    return root;
  };
  return {
    fixtureRepo: async (t, files = { 'README.md': 'fixture\n' }) => {
      const key = JSON.stringify(files);
      if (!prepared.has(key)) prepared.set(key, prepare(files));
      return copyRepo(t, await prepared.get(key));
    },
    dispose: () => Promise.all([...bases].map(base => rm(base, { recursive: true, force: true, maxRetries: 3 }).catch(() => {}))),
  };
}

/** The receipt-backed Phase 0 gates of a fixture record: gate id by harness name. */
export const fixtureGates = Object.freeze({ docling: 'docling-library-qualification', 'mcp-apps': 'mcp-apps-protocol-qualification' });

/** The criterion ids each fixture harness pins in its criteria.json. */
export const fixturePinned = Object.freeze(['corpus/a.pdf/content', 'corpus/a.pdf/provenance']);

/**
 * A verification.json in the two-space form record.mjs keeps, with one Phase 0 gate of kind
 * receipt per harness. `statuses` types a status by harness (`incomplete` otherwise),
 * `qualified` types `phase_0_qualified`, and `extra` appends other Phase 0 gates. `accepted`
 * gives a harness's gate its `accepted_failures`, and `construction` adds construction gates
 * (the gates an accepted failure can be tracked by).
 */
export function fixtureRecord({ statuses = {}, qualified = false, extra = [], accepted = {}, construction } = {}) {
  const phase_0 = Object.entries(fixtureGates).map(([harness, id]) => ({ id, kind: 'receipt', harness,
    receipt: `qualification/receipts/${harness}.json`, criteria: `qualification/${harness}/criteria.json`,
    status: statuses[harness] ?? 'incomplete', ...(Object.hasOwn(accepted, harness) ? { accepted_failures: accepted[harness] } : {}), covers: 'fixture' }));
  const gates = { phase_0: [...phase_0, ...extra], ...(construction ? { construction } : {}) };
  return `${JSON.stringify({ phase_0_qualified: qualified, current: { gates } }, null, 2)}\n`;
}

/** The construction gate the fixture acceptances are tracked by. */
export const fixtureTracker = Object.freeze({ id: 'converter-font-run-spacing', status: 'blocked_upstream', owner: 'integration-owner' });

/** A sound accepted failure of the fixture Docling gate: the owner accepts that `criterion` fails. */
export function fixtureAcceptance(criterion = 'corpus/a.pdf/provenance', overrides = {}) {
  return { criterion, decision: 'Accepted for now and reported upstream.', decided_on: '2026-10-06', decided_by: 'owner', tracked_by: fixtureTracker.id, ...overrides };
}

/** The tracked criteria.json of a fixture harness. */
export function fixtureCriteria(harness, required = fixturePinned) {
  return `${JSON.stringify({ gate: fixtureGates[harness], required }, null, 2)}\n`;
}

/**
 * A receipt of a fixture harness citing `sha`: the shared header and the envelope. Its result is
 * the fold of its criteria, as a harness computes it; `overrides` types any field instead, which
 * is how a test builds a receipt that must be rejected.
 */
export function fixtureReceipt(harness, sha, overrides = {}) {
  const criteria = overrides.criteria ?? fixturePinned.map(id => ({ id, required: true, result: 'pass' }));
  const harness_error = overrides.harness_error ?? null;
  const not_judged = Array.isArray(criteria) ? criteria.filter(entry => ['not_judged', 'not_applicable'].includes(entry?.result)).map(entry => entry.id) : [];
  return `${JSON.stringify({ git_sha: sha, inputs: ['harness/run.mjs'], produced_at: '2026-10-05T18:00:00Z', gate: fixtureGates[harness],
    result: foldCriteria(criteria, harness_error), harness_error, criteria, not_judged, ...overrides }, null, 2)}\n`;
}

/** The files every fixture with receipt gates starts from: the record, one harness input and both criteria files. */
export function fixtureRecordFiles(options) {
  return { 'verification.json': fixtureRecord(options), 'harness/run.mjs': '// v1\n',
    ...Object.fromEntries(Object.keys(fixtureGates).map(harness => [`qualification/${harness}/criteria.json`, fixtureCriteria(harness)])) };
}

/** Run a command with `input` on its stdin without blocking the event loop (spawnSync would stop every test running beside it); resolves like spawnSync with `status`, `signal`, `stdout` and `stderr`. */
export function runWithInput(command, args, { cwd, input, env }) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env, shell: false, stdio: ['pipe', 'pipe', 'pipe'] });
    let stdout = ''; let stderr = '';
    child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.once('error', reject);
    child.once('close', (status, signal) => resolve({ status: status ?? 1, signal, stdout, stderr }));
    child.stdin.on('error', () => {});
    child.stdin.end(input);
  });
}

let shell;
/** The path of a POSIX sh: `sh` from PATH when runnable, else (win32) the sh.exe that ships with the git on PATH. Throws, naming what was looked for; never skips. The answer is found once per process. */
export function posixShell() {
  shell ??= findPosixShell();
  return shell;
}

function findPosixShell() {
  if (spawnSync('sh', ['-c', 'exit 0'], { stdio: 'ignore' }).status === 0) return 'sh';
  const looked = ['sh (on PATH, not runnable)'];
  if (process.platform === 'win32') {
    const exec = spawnSync('git', ['--exec-path'], { encoding: 'utf8' });
    const execPath = exec.status === 0 ? exec.stdout.trim() : '';
    looked.push(`git --exec-path => ${execPath || `(failed: ${exec.error?.message ?? exec.stderr})`}`);
    // <git root>/mingw64/libexec/git-core -> <git root>
    const gitRoot = execPath ? resolve(execPath, '..', '..', '..') : '';
    for (const candidate of gitRoot ? [join(gitRoot, 'bin', 'sh.exe'), join(gitRoot, 'usr', 'bin', 'sh.exe')] : []) {
      looked.push(candidate);
      if (existsSync(candidate)) return candidate;
    }
  }
  throw new Error(`no POSIX sh found; looked for: ${looked.join('; ')}`);
}

/**
 * Run `body(entry)` for every entry at once and wait for ALL of them to settle before returning or
 * failing, so no case is still creating files in a scratch directory when its test's cleanup runs.
 * When any fail, one error names every failing case (an entry's label is its first element when it
 * is an array, else the entry itself) and keeps the causes in `errors`.
 */
export async function eachCase(entries, body) {
  const label = entry => String(Array.isArray(entry) ? entry[0] : entry);
  const settled = await Promise.allSettled(entries.map(entry => Promise.resolve().then(() => body(entry))));
  const failed = settled.flatMap((outcome, index) => (outcome.status === 'rejected' ? [{ name: label(entries[index]), error: outcome.reason }] : []));
  if (failed.length === 0) return settled.map(outcome => outcome.value);
  const error = new Error(`${failed.length} of ${entries.length} cases failed:\n${failed.map(({ name, error: cause }) => `- ${name}: ${String(cause?.message ?? cause).split('\n')[0]}`).join('\n')}`);
  error.errors = failed.map(({ error: cause }) => cause);
  throw error;
}
