/** Read selected tool versions from their single declarations; disagreement is an error, not a fallback. */
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function pins(root) {
  const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  const declared = /^bun@(\d+\.\d+\.\d+)$/.exec(manifest.packageManager ?? '');
  if (!declared) throw new Error('package.json packageManager must select one exact bun@x.y.z');
  const file = (await readFile(join(root, '.bun-version'), 'utf8')).trim();
  if (file !== declared[1]) throw new Error(`.bun-version ${file} disagrees with packageManager bun@${declared[1]}`);
  const toolchain = /^channel\s*=\s*"([^"]+)"/m.exec(await readFile(join(root, 'rust-toolchain.toml'), 'utf8'));
  if (!toolchain) throw new Error('rust-toolchain.toml must select an exact channel');
  return { bun: declared[1], rust: toolchain[1] };
}

/** The Bun executable running this task, so child steps use the same runtime that passed the version check. */
export function bun() {
  return process.versions.bun ? process.execPath : 'bun';
}
