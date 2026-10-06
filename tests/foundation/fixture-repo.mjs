/** Disposable Git repositories for tooling tests; never the real checkout. */
import { mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
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
  const base = await realpath(await mkdtemp(join(tmpdir(), 'okf-repo-')));
  // Cleanup of a scratch directory is best effort; a locked file there is not a test failure.
  t.after(() => rm(base, { recursive: true, force: true, maxRetries: 3 }).catch(() => {}));
  const root = join(base, 'repo');
  await mkdir(root);
  await git(root, 'init', '--quiet', '--initial-branch=main');
  await commit(root, files, 'initial');
  return { base, root };
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

/** The path of a POSIX sh: `sh` from PATH when runnable, else (win32) the sh.exe that ships with the git on PATH. Throws, naming what was looked for; never skips. */
export function posixShell() {
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
