/** Bundle and execute the catalog module with installed dependencies; no hand-written schema substitute. */
import { build } from 'esbuild';
import { pathToFileURL } from 'node:url';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const directory = await mkdtemp(join(tmpdir(), 'okf-catalog-'));
try {
  const output = join(directory, 'catalog.mjs');
  await build({ entryPoints: ['scripts/catalog-entry.ts'], outfile: output, bundle: true, platform: 'node', format: 'esm', target: 'node24' });
  await import(pathToFileURL(output).href);
} finally { await rm(directory, { recursive: true, force: true }); }
