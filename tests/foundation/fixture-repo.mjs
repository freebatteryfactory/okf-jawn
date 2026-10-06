/** Disposable Git repositories for tooling tests; never the real checkout. */
import { mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { gitLocalEnvironment, run } from '../../scripts/lib/process.mjs';

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
