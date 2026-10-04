#!/usr/bin/env node
/** Agent-agnostic task entrypoint. Every task fails honestly when prerequisites are missing. */
import { readFile, mkdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run, version } from './lib/process.mjs';
import { files, exists } from './lib/files.mjs';
import { generate } from './lib/generation.mjs';
import { initialize } from './lib/init.mjs';
import { tree } from './lib/tree.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [task = 'help', ...args] = process.argv.slice(2);

async function doctor() {
  const report = { kind: 'environment', rust: await version('rustc'), cargo: await version('cargo'),
    node: process.version, pnpm: await version('pnpm'), git: await version('git'),
    lockfiles: { cargo: await exists(join(root, 'Cargo.lock')), pnpm: await exists(join(root, 'pnpm-lock.yaml')) } };
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  return report;
}

async function prerequisites() {
  const environment = await doctor();
  const failures = [];
  if (typeof environment.rust !== 'string' || !environment.rust.startsWith('rustc 1.99.0 ')) failures.push('Install/select Rust 1.99.0 through rustup.');
  if (typeof environment.pnpm !== 'string' || environment.pnpm !== '12.8.1') failures.push('Install/select pnpm 12.8.1.');
  if (environment.node !== 'v24.21.0') failures.push('Use Node 24.21.0 from .node-version.');
  if (failures.length) throw new Error(failures.join('\n'));
}

async function bootstrap() {
  await prerequisites();
  if (!await exists(join(root, 'Cargo.lock'))) await run('cargo', ['generate-lockfile'], { cwd: root });
  if (!await exists(join(root, 'pnpm-lock.yaml'))) await run('pnpm', ['install', '--lockfile-only'], { cwd: root });
  await run('cargo', ['fetch', '--locked'], { cwd: root });
  await run('pnpm', ['install', '--frozen-lockfile'], { cwd: root });
  await run('cargo', ['build', '--locked', '--package', 'xtask'], { cwd: root });
  await generate(root);
  await run('pnpm', ['--filter', 'okf-jawn-ui', 'build'], { cwd: root });
  process.stdout.write('Dependency resolution, real generation, and UI consumer build completed. External host qualification remains separate.\n');
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

async function offlineChecks() {
  const tests = (await files(join(root, 'tests/foundation'))).filter(name => name.endsWith('.test.mjs')).map(name => join('tests/foundation', name));
  if (!tests.length) throw new Error('No foundation tests discovered');
  await run(process.execPath, ['--test', ...tests], { cwd: root });
}

async function qualify() {
  const name = args[0];
  if (name === 'mcp-wire') {
    await run(process.execPath, ['tests/integration/mcp-wire.mjs'], { cwd: root });
  } else if (name === 'application') {
    await run(process.execPath, ['tests/integration/acceptance.mjs'], { cwd: root });
  } else {
    throw new Error('Available: qualify mcp-wire or qualify application against a real server. Converter, iii crash recovery, and visual host qualification require their completed integrations; see SPEC.md and the lane instructions. None is recorded as passed by this command.');
  }
}

async function main() {
  switch (task) {
    case 'doctor': await doctor(); break;
    case 'init': process.stdout.write(`${JSON.stringify(await initialize(root), null, 2)}\n`); break;
    case 'check-offline': await offlineChecks(); break;
    case 'bootstrap': await bootstrap(); break;
    case 'gen': await generate(root); break;
    case 'gen-check': await generate(root, true); break;
    case 'vendor': await vendor(); break;
    case 'tree': process.stdout.write(await tree(root)); break;
    case 'lanes': await lanes(); break;
    case 'qualify': await qualify(); break;
    case 'check':
      await run('cargo', ['fmt', '--all', '--check'], { cwd: root });
      await run('cargo', ['clippy', '--locked', '--workspace', '--all-targets', '--', '-D', 'warnings'], { cwd: root });
      await run('cargo', ['xtask', 'source-policy', '--root', root], { cwd: root });
      await run('pnpm', ['--filter', 'okf-jawn-ui', 'check'], { cwd: root });
      await generate(root, true); break;
    case 'test':
      await run('cargo', ['test', '--locked', '--workspace'], { cwd: root });
      await run('pnpm', ['--filter', 'okf-jawn-ui', 'test'], { cwd: root }); break;
    case 'foundation':
      await generate(root, true);
      await run('cargo', ['test', '--locked', '-p', 'okf-jawn-contract', '-p', 'okf-jawn-core', '-p', 'xtask'], { cwd: root });
      await run('pnpm', ['--filter', 'okf-jawn-ui', 'build'], { cwd: root });
      await offlineChecks(); break;
    case 'help': process.stdout.write('Tasks: init doctor bootstrap gen gen-check check-offline check test foundation vendor tree lanes qualify\n'); break;
    default: throw new Error(`Unknown task: ${task}. Use help.`);
  }
}

main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
