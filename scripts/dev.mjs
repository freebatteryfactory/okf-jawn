#!/usr/bin/env bun
/** Agent-agnostic task entrypoint. Every task fails honestly when prerequisites are missing. */
import { readFile, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gitLocalEnvironment, run, version } from './lib/process.mjs';
import { files, exists } from './lib/files.mjs';
import { generate, requireLockfiles } from './lib/generation.mjs';
import { initialize, installHooks } from './lib/init.mjs';
import { tree } from './lib/tree.mjs';
import { bun, pins } from './lib/toolchain.mjs';
import { checkScope, createLanes, resetLanes, syncLaneTable } from './lib/lanes.mjs';
import { checkReceipts } from './lib/receipts.mjs';
import { cleanCheckout, premergeSteps, runLane, runPremerge } from './lib/gates.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ui = join(root, 'ui');
const [task = 'help', ...args] = process.argv.slice(2);
// Git exports these to hooks. Every task addresses the checkout that contains this file, and
// the offline tests create disposable repositories; an inherited GIT_DIR or GIT_INDEX_FILE
// would aim their git commands at this repository.
for (const name of gitLocalEnvironment) delete process.env[name];
/** The value following `--name`, or undefined. */
const option = name => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
/** Arguments that are neither a `--flag` nor the value of one. */
const positional = args.filter((value, index) => !value.startsWith('--') && !args[index - 1]?.startsWith('--'));

async function doctor() {
  const selected = await pins(root);
  const report = { kind: 'environment', selected,
    runtime: { bun: process.versions.bun ?? null, executable: process.execPath },
    bun: await version('bun'), rust: await version('rustc'), cargo: await version('cargo'), git: await version('git'),
    lockfiles: { cargo: await exists(join(root, 'Cargo.lock')), bun: await exists(join(root, 'bun.lock')) } };
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  return report;
}

async function prerequisites() {
  const environment = await doctor();
  const { selected } = environment;
  const failures = [];
  if (typeof environment.rust !== 'string' || !environment.rust.startsWith(`rustc ${selected.rust} `)) failures.push(`Install/select Rust ${selected.rust} through rustup.`);
  if (environment.runtime.bun !== selected.bun) failures.push(`Run this task with Bun ${selected.bun}; the executing runtime is ${environment.runtime.bun ?? 'not Bun'}.`);
  if (environment.bun !== selected.bun) failures.push(`The bun on PATH must be ${selected.bun}.`);
  if (failures.length) throw new Error(failures.join('\n'));
}

async function lock() {
  await prerequisites();
  // Minimal update: workspace members' own entries follow their manifests; every package already
  // in Cargo.lock keeps its version unless a manifest requirement no longer admits it.
  await run('cargo', ['update', '--workspace'], { cwd: root });
  await run(bun(), ['install', '--lockfile-only'], { cwd: root });
  process.stdout.write('Updated Cargo.lock and bun.lock for manifest changes only; already locked versions were kept. Review `git diff Cargo.lock bun.lock` and commit; nothing was installed.\n');
}

async function uiScript(name) {
  await run(bun(), ['--bun', 'run', name], { cwd: ui });
}

// Non-blocking by design: it reports when the pinned Hey API runtime meets the authored
// exactOptionalPropertyTypes rule, which is the signal to delete ui/tsconfig.generated.json's exception.
async function generatedStrictProbe() {
  try {
    await uiScript('typecheck:generated-strict');
    process.stdout.write('PROBE: generated client runtime now passes exactOptionalPropertyTypes; remove the exception in ui/tsconfig.generated.json.\n');
  } catch {
    process.stdout.write('PROBE (non-blocking): generated client runtime still fails exactOptionalPropertyTypes (hey-api/openapi-ts#3157); exception retained.\n');
  }
}

/**
 * Write ui/src/routeTree.gen.ts without bundling. The TanStack Router Vite plugin generates
 * the tree from its configResolved hook, so resolving the Vite config is enough. The plugin
 * logs and swallows generator errors, so the file is removed first and required afterwards.
 */
async function routes() {
  const target = join(ui, 'src', 'routeTree.gen.ts');
  await rm(target, { force: true });
  await run(bun(), ['-e', "const { resolveConfig } = await import('vite'); await resolveConfig({}, 'build');"], { cwd: ui });
  if (!await exists(target)) throw new Error('Route tree was not generated: ui/src/routeTree.gen.ts is missing after resolving the Vite config.');
  process.stdout.write('Generated ui/src/routeTree.gen.ts.\n');
}

async function bootstrap() {
  await prerequisites();
  await requireLockfiles(root);
  await installHooks(root);
  await run('cargo', ['fetch', '--locked'], { cwd: root });
  await run(bun(), ['install', '--frozen-lockfile'], { cwd: root });
  await run('cargo', ['build', '--locked', '--package', 'xtask'], { cwd: root });
  await generate(root);
  // The TanStack Router build plugin writes src/routeTree.gen.ts, which type checking imports.
  await uiScript('build');
  await uiScript('typecheck');
  process.stdout.write('Locked installation, real generation, TypeScript type check, and UI consumer build completed. External host qualification remains separate.\n');
}

async function vendor() {
  const data = JSON.parse(await readFile(join(root, 'vendors.json'), 'utf8'));
  const query = args.join(' ').toLowerCase();
  const selected = data.vendors.filter(entry => !query || JSON.stringify(entry).toLowerCase().includes(query));
  if (!selected.length) throw new Error(`No vendor entry for ${query}`);
  process.stdout.write(`${JSON.stringify(selected, null, 2)}\n`);
}

async function offlineChecks() {
  const tests = (await files(join(root, 'tests/foundation'))).filter(name => name.endsWith('.test.mjs')).map(name => `./tests/foundation/${name}`);
  if (!tests.length) throw new Error('No foundation tests discovered');
  // Fixture repositories run many git processes; the bun default of 5 s is too short on a loaded Windows machine.
  await run(bun(), ['test', '--timeout', '60000', ...tests], { cwd: root });
}

async function audit() {
  // Exact tooling exception: braces@3.0.3 via shadcn → @shadcn/registry → fast-glob → micromatch.
  // No newer braces release exists. No CVE-class fix available. Removal: shadcn/micromatch
  // move past braces 3.0.3, or braces publishes a patched release.
  const bracesException = 'GHSA-vfj7-8cjw-p6xm';
  await run('cargo', ['audit'], { cwd: root });
  // deny.toml is the cargo-deny policy; require the tool on PATH (CI installs it).
  await run('cargo', ['deny', 'check'], { cwd: root });
  await run(bun(), ['audit', '--prod'], { cwd: root });
  await run(bun(), ['audit', '--audit-level=high', '--ignore', bracesException], { cwd: root });
  // Full audit is informational for lower-severity tooling; never fail the task on it alone.
  const full = await run(bun(), ['audit'], { cwd: root, capture: true, allowFailure: true });
  process.stdout.write(full.stdout);
  if (full.stderr) process.stderr.write(full.stderr);
  process.stdout.write(`Full bun audit exit=${full.code} (informational; lower-severity tooling findings do not fail this task).\n`);
  process.stdout.write(`Ignored high tooling advisory ${bracesException} (shadcn braces path; see vendors.json / verification).\n`);
}

async function qualify() {
  const name = args[0];
  if (name === 'mcp-wire') {
    await run(bun(), ['tests/integration/mcp-wire.mjs'], { cwd: root });
  } else if (name === 'application') {
    await run(bun(), ['tests/integration/acceptance.mjs'], { cwd: root });
  } else if (name === 'docling') {
    await run(bun(), ['qualification/docling/run.mjs'], { cwd: root, timeout: 1_800_000 });
  } else if (name === 'iii') {
    await run(bun(), ['qualification/iii/run.mjs'], { cwd: root, timeout: 1_800_000 });
  } else if (name === 'mcp-apps') {
    await run(bun(), ['qualification/mcp-apps/run.mjs'], { cwd: root, timeout: 1_800_000 });
  } else {
    throw new Error('Available: qualify mcp-wire | application | docling | iii | mcp-apps. None is recorded as passed by this command until its receipt is written.');
  }
}

/** Run named premerge steps in order, stopping at the first failure. */
async function runNamed(ids) {
  const steps = premergeSteps(root);
  for (const id of ids) {
    const step = steps.find(entry => entry.id === id);
    if (!step) throw new Error(`No premerge step named ${id}.`);
    await run(step.command, step.args, { cwd: step.cwd });
  }
}

async function main() {
  switch (task) {
    case 'doctor': await doctor(); break;
    case 'init': process.stdout.write(`${JSON.stringify(await initialize(root), null, 2)}\n`); break;
    case 'check-offline': await offlineChecks(); break;
    case 'lock': await lock(); break;
    case 'bootstrap': await bootstrap(); break;
    case 'gen': await generate(root); break;
    case 'gen-check': await generate(root, true); break;
    case 'routes': await routes(); break;
    case 'vendor': await vendor(); break;
    case 'tree': process.stdout.write(await tree(root)); break;
    case 'lanes': process.stdout.write(`${(await createLanes(root, { names: args })).join('\n')}\n`); break;
    case 'lanes-table': process.stdout.write(await syncLaneTable(root) ? 'AGENTS.md lane table regenerated.\n' : 'AGENTS.md lane table is current.\n'); break;
    case 'scope': {
      const result = await checkScope(root, { lane: positional[0], base: option('--base') });
      process.stdout.write(`scope: ${result.changed.length} changed path(s) since ${result.base}, all inside ${result.name}.\n`);
      break;
    }
    case 'lane': {
      const result = await runLane(root, positional[0]);
      if (!result.passed) process.exitCode = 1;
      break;
    }
    case 'premerge': {
      const result = await runPremerge(root, { only: option('--step') });
      if (!result.passed) process.exitCode = 1;
      break;
    }
    case 'clean-checkout': {
      const result = await cleanCheckout(root);
      process.stdout.write(`${result.passed ? 'CLEAN_CHECKOUT_PASS' : 'CLEAN_CHECKOUT_FAIL'} ${result.receipt.git_sha}\nreceipt: ${result.receiptPath}\n`);
      if (!result.passed) process.exitCode = 1;
      break;
    }
    case 'lanes-reset': process.stdout.write(`${await resetLanes(root)}\n`); break;
    case 'qualify': await qualify(); break;
    case 'check-receipts': process.stdout.write(`${await checkReceipts(root)}\n`); break;
    case 'audit': await audit(); break;
    case 'check': await runNamed(['fmt', 'clippy', 'source-policy', 'ui-lint', 'ui-typecheck', 'gen-check']); break;
    case 'test': await runNamed(['test', 'ui-test']); break;
    case 'foundation':
      await generate(root, true);
      await run('cargo', ['test', '--locked', '-p', 'okf-jawn-contract', '-p', 'okf-jawn-core', '-p', 'xtask'], { cwd: root });
      await uiScript('build');
      await generatedStrictProbe();
      await uiScript('typecheck');
      await offlineChecks(); break;
    case 'help': process.stdout.write('Tasks: init doctor lock bootstrap gen gen-check check-offline check test foundation vendor tree lanes lanes-reset qualify check-receipts audit\n'); break;
    default: throw new Error(`Unknown task: ${task}. Use help.`);
  }
}

main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
