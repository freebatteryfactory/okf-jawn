/** Deterministic file sets for regeneration; symlinks are never traversed. */
import { createHash } from 'node:crypto';
import { readdir, readFile, lstat, mkdir, copyFile, rm, rename, mkdtemp } from 'node:fs/promises';
import { join, relative, dirname, resolve, sep } from 'node:path';

export async function files(root) {
  const result = [];
  async function walk(directory) {
    for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0)) {
      if (entry.name === '.git' || entry.name === 'target' || entry.name === 'node_modules' || entry.name === '.artifacts' || entry.name === 'dist' || entry.name === 'dist-apps') continue;
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Refusing symlink during deterministic traversal: ${path}`);
      if (entry.isDirectory()) await walk(path);
      else if (entry.isFile()) result.push(relative(root, path).split(sep).join('/'));
    }
  }
  await walk(root); return result.sort();
}

export async function manifest(root) {
  const entries = {};
  for (const path of await files(root)) entries[path] = createHash('sha256').update(await readFile(join(root, path))).digest('hex');
  return entries;
}

export function differences(left, right) {
  return [...new Set([...Object.keys(left), ...Object.keys(right)])].sort().filter(path => left[path] !== right[path]);
}

export async function exists(path) { try { await lstat(path); return true; } catch (error) { if (error.code === 'ENOENT') return false; throw error; } }

export async function replaceGenerated(source, destination, repo) {
  const base = resolve(repo); const output = resolve(destination);
  if (!output.startsWith(base + sep) || !['api', join('ui', 'src', 'api', 'generated'), join('generated', 'cli')].includes(relative(base, output))) {
    throw new Error(`Refusing to replace non-generated destination: ${destination}`);
  }
  let ancestor = base;
  for (const part of relative(base, output).split(sep)) {
    ancestor = join(ancestor, part);
    if (await exists(ancestor) && (await lstat(ancestor)).isSymbolicLink()) throw new Error('Generated path ancestor must not be a symlink');
  }
  if (await exists(output)) {
    const current = await lstat(output);
    if (current.isSymbolicLink()) throw new Error('Generated directory must not be a symlink');
  }
  await mkdir(dirname(output), { recursive: true });
  const staging = await mkdtemp(join(dirname(output), '.okf-codegen-stage-'));
  const prepared = join(staging, 'new');
  const backup = join(staging, 'previous');
  let moved = false;
  try {
    await mkdir(prepared);
    for (const name of await files(source)) {
      await mkdir(dirname(join(prepared, name)), { recursive: true });
      await copyFile(join(source, name), join(prepared, name));
    }
    if (await exists(output)) { await rename(output, backup); moved = true; }
    try { await rename(prepared, output); }
    catch (error) { if (moved) await rename(backup, output); throw error; }
  } finally { await rm(staging, { recursive: true, force: true }); }
}
