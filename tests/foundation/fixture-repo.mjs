/** Disposable Git repositories for tooling tests; never the real checkout. */
import { mkdtemp, mkdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
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
