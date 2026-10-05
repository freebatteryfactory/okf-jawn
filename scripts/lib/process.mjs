/** Run argument-vector commands without shell interpolation or swallowed failures. */
import { spawn } from 'node:child_process';

/** Limit for one command unless the caller passes `timeout`. */
export const DEFAULT_TIMEOUT_MS = 600_000;
/** A cold build of the runtime feature graph (Docling, libgit2, SQLite) outlasts ten minutes. */
export const CARGO_TIMEOUT_MS = 2_700_000;
/**
 * Repository-location variables Git exports to hooks. A task that inherited them would aim its
 * own git commands, and those of the disposable repositories the offline tests create, at the
 * repository whose hook is running.
 */
export const gitLocalEnvironment = Object.freeze(['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_PREFIX',
  'GIT_COMMON_DIR', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_IMPLICIT_WORK_TREE']);

export function defaultTimeout(command) {
  return command === 'cargo' ? CARGO_TIMEOUT_MS : DEFAULT_TIMEOUT_MS;
}

/**
 * PATH override for a cargo child on Windows. Git for Windows and MSYS2 keep a coreutils
 * `link.exe` in `usr\bin`; ahead of the MSVC linker it makes every build script fail to link.
 * Returns `{}` when nothing has to change, so other platforms and clean PATHs are untouched.
 */
export function cargoEnvironment(env = process.env, platform = process.platform) {
  if (platform !== 'win32') return {};
  const key = Object.keys(env).find(name => name.toUpperCase() === 'PATH');
  if (!key) return {};
  const entries = String(env[key]).split(';');
  const kept = entries.filter(entry => !/[\\/]usr[\\/]bin[\\/]?$/i.test(entry.trim()));
  return kept.length === entries.length ? {} : { [key]: kept.join(';') };
}

export function childEnvironment(command, env = {}, base = process.env, platform = process.platform) {
  const merged = { ...base, ...env };
  return command === 'cargo' ? { ...merged, ...cargoEnvironment(merged, platform) } : merged;
}

/**
 * The command that ends a whole process tree, or `null` where the child alone is signalled.
 * On Windows SIGTERM ends only cargo.exe and leaves rustc and link.exe holding `target\` open.
 * POSIX callers that need a group kill own their process group (see qualification harnesses).
 */
export function treeKill(pid, platform = process.platform) {
  return platform === 'win32' ? ['taskkill', ['/PID', String(pid), '/T', '/F']] : null;
}

export async function run(command, args, options = {}) {
  const { cwd, env = {}, capture = false, tee, timeout = defaultTimeout(command), allowFailure = false } = options;
  const piped = capture || typeof tee === 'function';
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env: childEnvironment(command, env), shell: false,
      stdio: piped ? ['ignore', 'pipe', 'pipe'] : 'inherit' });
    let stdout = ''; let stderr = ''; let timer;
    if (piped) {
      child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
      child.stdout.on('data', chunk => { if (capture) stdout += chunk; if (tee) tee(chunk, 'stdout'); });
      child.stderr.on('data', chunk => { if (capture) stderr += chunk; if (tee) tee(chunk, 'stderr'); });
    }
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('close', (code, signal) => {
      clearTimeout(timer);
      if (code === 0 || allowFailure) resolve({ code: code ?? 1, stdout, stderr });
      else reject(new Error(`${command} ${args.join(' ')} exited ${code ?? signal}\n${stderr}`));
    });
    timer = setTimeout(() => {
      const kill = child.pid === undefined ? null : treeKill(child.pid);
      if (kill) spawn(kill[0], kill[1], { stdio: 'ignore', windowsHide: true }).once('error', () => child.kill('SIGTERM'));
      else child.kill('SIGTERM');
      reject(new Error(`${command} timed out after ${timeout} ms; no success recorded`));
    }, timeout);
  });
}

export async function version(command, args = ['--version']) {
  try { return (await run(command, args, { capture: true, timeout: 20_000 })).stdout.trim(); }
  catch (error) { return { unavailable: String(error.message) }; }
}
