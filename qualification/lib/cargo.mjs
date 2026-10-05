/**
 * Cargo facts the qualification orchestrators need: where release binaries land
 * (honouring CARGO_TARGET_DIR and build.target-dir) and which versions Cargo.lock pins.
 * Dependency-free; the pure functions are covered by tests/foundation/harness.test.mjs.
 */

import { spawn } from 'node:child_process';
import { join } from 'node:path';

/** Run a command; resolve with its exit code and captured output (inherit: stream instead). */
export function exec(command, args, options = {}) {
  return new Promise((resolveExec, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: options.inherit ? 'inherit' : ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    child.stdout?.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr?.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => resolveExec({ code: code ?? 1, stdout, stderr }));
  });
}

/** Every [[package]] entry of Cargo.lock named `name`. */
export function lockedPackages(lockText, name) {
  const field = (block, key) => new RegExp(`^${key} = "([^"]+)"\\r?$`, 'm').exec(block)?.[1] ?? null;
  return lockText
    .split(/\r?\n\[\[package\]\]\r?\n/)
    .filter((block) => field(block, 'name') === name)
    .map((block) => ({ name, version: field(block, 'version'), checksum: field(block, 'checksum') }));
}

/** The single [[package]] entry named `name`; throws when Cargo.lock has none or several. */
export function lockedPackage(lockText, name) {
  const matches = lockedPackages(lockText, name);
  if (matches.length !== 1 || !matches[0].version) {
    throw new Error(`Cargo.lock must pin exactly one ${name} package with a version; found ${matches.length}`);
  }
  return matches[0];
}

/** Path of a release binary under a cargo target directory. */
export function releaseBinary(targetDir, name, platform = process.platform) {
  return join(targetDir, 'release', platform === 'win32' ? `${name}.exe` : name);
}

/** The workspace target directory as cargo itself resolves it. */
export async function cargoTargetDir(root) {
  const result = await exec('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked'], { cwd: root });
  if (result.code !== 0) throw new Error(`cargo metadata exited ${result.code}\n${result.stderr}`);
  const directory = JSON.parse(result.stdout).target_directory;
  if (typeof directory !== 'string' || directory.length === 0) {
    throw new Error('cargo metadata did not report target_directory');
  }
  return directory;
}

/** Build one workspace package in release mode from the lockfile; return its binary path. */
export async function buildRelease(root, pkg) {
  const result = await exec('cargo', ['build', '--locked', '--release', '-p', pkg], { cwd: root, inherit: true });
  if (result.code !== 0) throw new Error(`cargo build --locked --release -p ${pkg} exited ${result.code}`);
  return releaseBinary(await cargoTargetDir(root), pkg);
}
