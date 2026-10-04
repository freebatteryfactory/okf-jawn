/** Render the actual source tree, including reserved empty directories, without traversing symlinks. */
import { readdir } from 'node:fs/promises';
import { basename, join } from 'node:path';
const excluded = new Set(['.git', 'target', 'node_modules', '.artifacts', 'dist', 'dist-apps', '.env']);
export async function tree(root) {
  const lines = [`${basename(root)}/`];
  async function visit(directory, prefix) {
    const entries = (await readdir(directory, { withFileTypes: true })).filter(entry => !excluded.has(entry.name))
      .sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
    for (const [index, entry] of entries.entries()) {
      const last = index === entries.length - 1;
      lines.push(`${prefix}${last ? '└── ' : '├── '}${entry.name}${entry.isDirectory() ? '/' : ''}`);
      if (entry.isDirectory()) await visit(join(directory, entry.name), prefix + (last ? '    ' : '│   '));
    }
  }
  await visit(root, ''); return lines.join('\n') + '\n';
}
