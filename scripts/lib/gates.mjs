/** Gate sequences as data (lane, premerge, clean checkout) and the one runner that logs them. */
import { appendFileSync, writeFileSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { run } from './process.mjs';
import { exists } from './files.mjs';
import { bun } from './toolchain.mjs';
import { laneNamed, lanesParent } from './lanes.mjs';

const dev = (root, id, ...task) => ({ id, command: bun(), args: ['scripts/dev.mjs', ...task], cwd: root });
const uiScript = (root, id, script, ...extra) => ({ id, command: bun(), args: ['--bun', 'run', script, ...extra], cwd: join(root, 'ui') });
const cargo = (root, id, ...args) => ({ id, command: 'cargo', args, cwd: root });

/** The gate of one lane: its own crates and features, or its own UI specs, then the scope check. */
export function laneSteps(root, lane) {
  if (lane.kind === 'ui') {
    return [
      uiScript(root, 'biome', 'lint'),
      dev(root, 'routes', 'routes'),
      uiScript(root, 'typecheck', 'typecheck'),
      uiScript(root, 'test', 'test', ...lane.tests, ...lane.testExclude.flatMap(glob => ['--exclude', glob])),
      dev(root, 'scope', 'scope', lane.name),
    ];
  }
  const packages = lane.crates.flatMap(name => ['-p', name]);
  const features = lane.features.length ? ['--features', lane.features.join(',')] : [];
  return [
    cargo(root, 'fmt', 'fmt', '--check', ...packages),
    cargo(root, 'clippy', 'clippy', '--locked', ...packages, ...features, '--all-targets', '--', '-D', 'warnings'),
    cargo(root, 'test', 'test', '--locked', ...packages, ...features),
    cargo(root, 'source-policy', 'xtask', 'source-policy', '--root', root),
    dev(root, 'scope', 'scope', lane.name),
  ];
}

/** Run one step with its live output sent to `write`. The exit code is the result, never an exception. */
export async function executeStep(step, write) {
  try {
    return (await run(step.command, step.args, { cwd: step.cwd, allowFailure: true, tee: chunk => write(chunk) })).code;
  } catch (error) {
    write(`${error.message}\n`);
    return 1;
  }
}

/**
 * Run steps in order and log them to `logPath`. With `failFast` the first failure ends the run;
 * without it every step runs, so one failure cannot hide the rest.
 */
export async function runSteps(steps, { logPath, failFast, execute = executeStep, echo = chunk => process.stdout.write(chunk) }) {
  await mkdir(dirname(logPath), { recursive: true });
  writeFileSync(logPath, '');
  const write = chunk => { appendFileSync(logPath, chunk); echo(chunk); };
  write(`log: ${logPath}\n`);
  const results = [];
  for (const step of steps) {
    write(`\n=== ${step.id}: ${step.command} ${step.args.join(' ')}\n`);
    const code = await execute(step, write);
    write(`=== ${step.id} exit ${code}\n`);
    results.push({ id: step.id, code });
    if (code !== 0 && failFast) break;
  }
  return { results, failed: results.filter(result => result.code !== 0).map(result => result.id), write };
}

/** HEAD, suffixed `-dirty` when the tree has uncommitted changes or its status cannot be read, so a log never claims a commit it did not test. */
export async function revisionLabel(root) {
  const head = (await run('git', ['rev-parse', 'HEAD'], { cwd: root, capture: true })).stdout.trim();
  // A status that cannot be read is not evidence of a clean tree.
  const status = await run('git', ['status', '--porcelain'], { cwd: root, capture: true, allowFailure: true });
  return status.code !== 0 || status.stdout.trim() ? `${head}-dirty` : head;
}

export async function runLane(root, name, { execute, echo } = {}) {
  const lane = laneNamed(name);
  const label = await revisionLabel(root);
  const logPath = join(root, '.artifacts', 'lane', lane.name, `${label}.log`);
  const { failed, write } = await runSteps(laneSteps(root, lane), { logPath, failFast: true, execute, echo });
  const line = failed.length ? `FAIL ${lane.name} ${label} ${failed[0]}` : `PASS ${lane.name} ${label}`;
  write(`\n${line}\n`);
  return { passed: failed.length === 0, line, logPath };
}

/**
 * Everything CI runs after bootstrap, in CI's order, with every feature. CI executes these one
 * step at a time (`premerge --step <id>`); tests/foundation/ci.test.mjs keeps the two identical.
 */
export function premergeSteps(root) {
  return [
    dev(root, 'check-offline', 'check-offline'),
    dev(root, 'gen-check', 'gen-check'),
    cargo(root, 'fmt', 'fmt', '--all', '--check'),
    cargo(root, 'clippy', 'clippy', '--locked', '--workspace', '--all-features', '--all-targets', '--', '-D', 'warnings'),
    cargo(root, 'source-policy', 'xtask', 'source-policy', '--root', root),
    cargo(root, 'test', 'test', '--locked', '--workspace', '--all-features'),
    uiScript(root, 'ui-lint', 'lint'),
    uiScript(root, 'ui-build', 'build'),
    uiScript(root, 'ui-typecheck', 'typecheck'),
    uiScript(root, 'ui-test', 'test'),
    dev(root, 'check-receipts', 'check-receipts'),
  ];
}

export async function runPremerge(root, { only, execute, echo } = {}) {
  const steps = premergeSteps(root);
  if (only !== undefined) {
    const step = steps.find(entry => entry.id === only);
    if (!step) throw new Error(`Unknown premerge step: ${only}. Steps: ${steps.map(entry => entry.id).join(', ')}.`);
    await run(step.command, step.args, { cwd: step.cwd });
    return { passed: true, line: `PASS premerge ${only}`, logPath: null };
  }
  const label = await revisionLabel(root);
  const logPath = join(root, '.artifacts', 'premerge', `${label}.log`);
  const { failed, write } = await runSteps(steps, { logPath, failFast: false, execute, echo });
  const line = failed.length ? `FAIL premerge ${label} ${failed.join(',')}` : `PASS premerge ${label}`;
  write(`\n${line}\n`);
  return { passed: failed.length === 0, line, logPath };
}

/** The clean-checkout sequence as [log id, dev.mjs task]; generation is checked twice for drift. */
export const cleanCheckoutTasks = Object.freeze([['bootstrap', 'bootstrap'], ['gen-check-1', 'gen-check'], ['gen-check-2', 'gen-check'],
  ['foundation', 'foundation'], ['check', 'check'], ['test', 'test'], ['check-offline', 'check-offline'], ['audit', 'audit']]);

/**
 * Run the whole foundation in a temporary detached worktree of HEAD and write a receipt.
 * Every task runs even after a failure. The worktree is removed only when it is still clean;
 * otherwise it stays as evidence. Nothing here forces git.
 */
export async function cleanCheckout(root, { parent = lanesParent(root), execute = executeStep, echo = chunk => process.stdout.write(chunk), now = () => new Date() } = {}) {
  const git = (args, cwd = root) => run('git', args, { cwd, capture: true, allowFailure: true });
  const sha = (await run('git', ['rev-parse', 'HEAD'], { cwd: root, capture: true })).stdout.trim();
  const worktree = join(parent, `clean-checkout-${sha.slice(0, 12)}`);
  if (await exists(worktree)) throw new Error(`clean-checkout refused: ${worktree} already exists. Inspect it, then remove it with \`git worktree remove\`.`);
  await mkdir(parent, { recursive: true });
  const added = await git(['worktree', 'add', '--detach', worktree, sha]);
  if (added.code !== 0) throw new Error(`git worktree add failed (git exit ${added.code}): ${added.stderr.trim()}`);
  const out = join(root, '.artifacts', 'qualification', 'clean-checkout');
  const exit_codes = {};
  for (const [id, task] of cleanCheckoutTasks) {
    const step = { id, command: bun(), args: ['scripts/dev.mjs', task], cwd: worktree };
    const { results } = await runSteps([step], { logPath: join(out, `${id}.log`), failFast: true, execute, echo });
    exit_codes[id] = results[0].code;
  }
  const status = await git(['status', '--porcelain'], worktree);
  const git_status_empty = status.code === 0 && status.stdout.trim() === '';
  const receipt = { git_sha: sha, inputs: ['.'], produced_at: now().toISOString(), exit_codes, git_status_empty };
  const receiptPath = join(out, 'receipt.json');
  await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  let removed = false;
  if (git_status_empty) {
    const removal = await git(['worktree', 'remove', worktree]);
    removed = removal.code === 0;
    if (!removed) echo(`clean-checkout: could not remove ${worktree} (${removal.stderr.trim()}); remove it with \`git worktree remove\` once nothing holds it open.\n`);
  } else {
    echo(`clean-checkout: ${worktree} is not clean after the run and was left in place:\n${status.stdout}${status.stderr}`);
  }
  return { passed: git_status_empty && Object.values(exit_codes).every(value => value === 0), receipt, receiptPath, worktree, removed };
}

/**
 * The foundation tests by cost. `fast` files create no temporary repository and spawn no child
 * process, so `check-offline --fast` (the pre-commit hook) stays under a few seconds; every
 * other file is `slow` and runs in the full `check-offline`. tests/foundation/gates.test.mjs
 * fails when a `*.test.mjs` file is in neither list or in both, or when a fast file spawns.
 */
export const foundationTests = Object.freeze({
  fast: Object.freeze(['ci', 'files', 'generation', 'lockfile', 'policy', 'ports', 'receipt-envelope', 'toolchain'].map(name => `${name}.test.mjs`)),
  slow: Object.freeze(['gates', 'harness', 'hooks', 'init', 'lanes', 'no-external-engine', 'process', 'receipts', 'records', 'vendor'].map(name => `${name}.test.mjs`)),
});
