/** Gate sequences as data (lane, premerge, clean checkout) and the one runner that logs them. */
import { appendFileSync, writeFileSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { run } from './process.mjs';
import { bun } from './toolchain.mjs';
import { laneNamed } from './lanes.mjs';

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
    cargo(root, 'fmt', 'fmt', '--all', '--check'),
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

/** HEAD, suffixed `-dirty` when the tree has uncommitted changes, so a log never claims a commit it did not test. */
export async function revisionLabel(root) {
  const head = (await run('git', ['rev-parse', 'HEAD'], { cwd: root, capture: true })).stdout.trim();
  const status = (await run('git', ['status', '--porcelain'], { cwd: root, capture: true })).stdout.trim();
  return status ? `${head}-dirty` : head;
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
