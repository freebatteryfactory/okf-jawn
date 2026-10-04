/** Initialize a local repository without a remote, credentials, commit, or overwritten settings. */
import { copyFile } from 'node:fs/promises';
import { constants } from 'node:fs';
import { join } from 'node:path';
import { exists } from './files.mjs';
import { run } from './process.mjs';

export async function initialize(root) {
  if (!await exists(join(root, '.git'))) await run('git', ['init', '--initial-branch=main', root], { cwd: root });
  const sample = join(root, 'deploy', '.env.example');
  if (await exists(sample) && !await exists(join(root, '.env'))) {
    await copyFile(sample, join(root, '.env'), constants.COPYFILE_EXCL);
  }
  return { repository: root, remoteCreated: false, committed: false };
}
