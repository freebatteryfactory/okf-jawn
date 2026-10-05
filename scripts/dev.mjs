#!/usr/bin/env bun
/** Agent-agnostic task entrypoint. Every task fails honestly when prerequisites are missing. */
import { readFile, mkdir, readdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gitLocalEnvironment, run, version } from './lib/process.mjs';
import { files, exists } from './lib/files.mjs';
import { generate, requireLockfiles } from './lib/generation.mjs';
import { initialize } from './lib/init.mjs';
import { tree } from './lib/tree.mjs';
import { bun, pins } from './lib/toolchain.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ui = join(root, 'ui');
const [task = 'help', ...args] = process.argv.slice(2);
// Git exports these to hooks. Every task addresses the checkout that contains this file, and
// the offline tests create disposable repositories; an inherited GIT_DIR or GIT_INDEX_FILE
// would aim their git commands at this repository.
for (const name of gitLocalEnvironment) delete process.env[name];

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

async function bootstrap() {
  await prerequisites();
  await requireLockfiles(root);
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

async function lanes() {
  const status = await run('git', ['status', '--porcelain'], { cwd: root, capture: true });
  if (status.stdout.trim()) throw new Error('Commit the foundation before creating worktrees; no dirty-state fan-out.');
  const base = (await run('git', ['rev-parse', '--verify', 'HEAD'], { cwd: root, capture: true })).stdout.trim();
  const parent = resolve(root, '..', 'okf-jawn-lanes'); await mkdir(parent, { recursive: true });
  for (const lane of ['storage', 'ingest', 'core-cli', 'server', 'mcp-execution', 'workspace-ui', 'views']) {
    await run('git', ['worktree', 'add', '-b', `build/${lane}`, join(parent, lane), base], { cwd: root });
  }
}

/**
 * Remove empty construction lanes created by `lanes`.
 *
 * Refuses if any build/* branch has commits beyond its merge-base with HEAD, or if any
 * lane worktree is dirty. Otherwise removes the worktrees and deletes the local branches.
 */
async function lanesReset() {
  const parent = resolve(root, '..', 'okf-jawn-lanes');
  const lanes = ['storage', 'ingest', 'core-cli', 'server', 'mcp-execution', 'workspace-ui', 'views'];
  for (const lane of lanes) {
    const branch = `build/${lane}`;
    const worktree = join(parent, lane);
    const exists = await run('git', ['rev-parse', '--verify', branch], {
      cwd: root,
      capture: true,
      allowFailure: true,
    });
    if (exists.code !== 0) continue;
    if (await existsPath(worktree)) {
      const dirty = await run('git', ['status', '--porcelain'], {
        cwd: worktree,
        capture: true,
        allowFailure: true,
      });
      if (dirty.code === 0 && dirty.stdout.trim()) {
        throw new Error(`lanes-reset refused: worktree ${worktree} is dirty`);
      }
    }
    const base = (await run('git', ['merge-base', branch, 'HEAD'], { cwd: root, capture: true })).stdout.trim();
    const ahead = await run('git', ['rev-list', '--count', `${base}..${branch}`], {
      cwd: root,
      capture: true,
    });
    if (Number(ahead.stdout.trim()) > 0) {
      throw new Error(
        `lanes-reset refused: ${branch} has ${ahead.stdout.trim()} commit(s) beyond its base; reset would discard lane work`,
      );
    }
  }
  for (const lane of lanes) {
    const branch = `build/${lane}`;
    const worktree = join(parent, lane);
    if (await existsPath(worktree)) {
      await run('git', ['worktree', 'remove', '--force', worktree], { cwd: root });
    }
    const exists = await run('git', ['rev-parse', '--verify', branch], {
      cwd: root,
      capture: true,
      allowFailure: true,
    });
    if (exists.code === 0) {
      await run('git', ['branch', '-D', branch], { cwd: root });
    }
  }
  process.stdout.write('lanes-reset: removed empty build/* worktrees and branches.\n');
}

async function existsPath(path) {
  return exists(path);
}

async function offlineChecks() {
  const tests = (await files(join(root, 'tests/foundation'))).filter(name => name.endsWith('.test.mjs')).map(name => `./tests/foundation/${name}`);
  if (!tests.length) throw new Error('No foundation tests discovered');
  await run(bun(), ['test', ...tests], { cwd: root });
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

/**
 * Validate qualification receipts under qualification/receipts/.
 *
 * Expected minimum JSON shape per receipt:
 *   { "git_sha": "<full sha>", "inputs": ["path/relative/to/repo", ...] }
 *
 * Fails when git_sha is not an ancestor of HEAD, or when any listed input path
 * changed after that SHA (git diff --name-only <sha> HEAD -- <inputs...>).
 */
async function checkReceipts() {
  const receiptsDir = join(root, 'qualification', 'receipts');
  await mkdir(receiptsDir, { recursive: true });
  const entries = (await readdir(receiptsDir)).filter((name) => name.endsWith('.json')).sort();
  if (!entries.length) {
    process.stdout.write('check-receipts: no *.json receipts under qualification/receipts/ (ok while empty).\n');
    return;
  }
  const failures = [];
  for (const name of entries) {
    const path = join(receiptsDir, name);
    let receipt;
    try {
      receipt = JSON.parse(await readFile(path, 'utf8'));
    } catch (error) {
      failures.push(`${name}: not valid JSON (${error.message})`);
      continue;
    }
    const sha = receipt.git_sha ?? receipt.commit_sha;
    if (typeof sha !== 'string' || !/^[0-9a-f]{40}$/i.test(sha)) {
      failures.push(`${name}: missing git_sha (expected 40-hex); shape is { git_sha, inputs: string[] }`);
      continue;
    }
    if (!Array.isArray(receipt.inputs) || !receipt.inputs.every((item) => typeof item === 'string')) {
      failures.push(`${name}: inputs must be a string[] of repo-relative paths`);
      continue;
    }
    const ancestor = await run('git', ['merge-base', '--is-ancestor', sha, 'HEAD'], {
      cwd: root,
      capture: true,
      allowFailure: true,
    });
    if (ancestor.code !== 0) {
      failures.push(`${name}: git_sha ${sha} is not an ancestor of HEAD`);
      continue;
    }
    if (receipt.inputs.length === 0) continue;
    const changed = await run(
      'git',
      ['diff', '--name-only', sha, 'HEAD', '--', ...receipt.inputs],
      { cwd: root, capture: true },
    );
    const names = changed.stdout
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    if (names.length) {
      failures.push(
        `${name}: inputs changed after ${sha}:\n  ${names.join('\n  ')}`,
      );
    }
  }
  if (failures.length) {
    throw new Error(`check-receipts failed:\n${failures.join('\n')}`);
  }
  process.stdout.write(`check-receipts: ${entries.length} receipt(s) valid against HEAD.\n`);
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

async function main() {
  switch (task) {
    case 'doctor': await doctor(); break;
    case 'init': process.stdout.write(`${JSON.stringify(await initialize(root), null, 2)}\n`); break;
    case 'check-offline': await offlineChecks(); break;
    case 'lock': await lock(); break;
    case 'bootstrap': await bootstrap(); break;
    case 'gen': await generate(root); break;
    case 'gen-check': await generate(root, true); break;
    case 'vendor': await vendor(); break;
    case 'tree': process.stdout.write(await tree(root)); break;
    case 'lanes': await lanes(); break;
    case 'lanes-reset': await lanesReset(); break;
    case 'qualify': await qualify(); break;
    case 'check-receipts': await checkReceipts(); break;
    case 'audit': await audit(); break;
    case 'check':
      await run('cargo', ['fmt', '--all', '--check'], { cwd: root });
      await run('cargo', ['clippy', '--locked', '--workspace', '--all-targets', '--', '-D', 'warnings'], { cwd: root });
      await run('cargo', ['xtask', 'source-policy', '--root', root], { cwd: root });
      await uiScript('lint');
      await uiScript('typecheck');
      await generate(root, true); break;
    case 'test':
      await run('cargo', ['test', '--locked', '--workspace'], { cwd: root });
      await uiScript('test'); break;
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
