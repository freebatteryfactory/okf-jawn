/**
 * Bundle and execute the catalog module with installed dependencies; no hand-written schema substitute.
 * Output names are fixed (no content hashes) so gen-check can prove two generations are byte-identical.
 * Also copies the pinned vega-lite JSON Schema into the presentation staging directory.
 */

import { copyFile, mkdtemp, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { env } from 'node:process';
import { fileURLToPath, pathToFileURL, URL } from 'node:url';
import { build } from 'vite';

const root = fileURLToPath(new URL('..', import.meta.url));
const output = env.OKF_CATALOG_OUT;
if (!output) throw new Error('OKF_CATALOG_OUT must name a generation staging directory');

const require = createRequire(join(root, 'package.json'));
const vegaLiteSchema = join(
  dirname(require.resolve('vega-lite/package.json')),
  'build',
  'vega-lite-schema.json',
);

const directory = await mkdtemp(join(tmpdir(), 'okf-catalog-'));
try {
  await build({
    configFile: false,
    logLevel: 'warn',
    root,
    resolve: { alias: { '@': fileURLToPath(new URL('../src', import.meta.url)) } },
    ssr: {
      // Catalog runs once under Bun; every dependency must be in the single fixed entry file.
      noExternal: true,
      target: 'node',
    },
    build: {
      outDir: directory,
      emptyOutDir: true,
      ssr: fileURLToPath(new URL('./catalog-entry.ts', import.meta.url)),
      target: 'esnext',
      minify: false,
      sourcemap: false,
      cssCodeSplit: false,
      rolldownOptions: {
        output: {
          format: 'es',
          entryFileNames: 'catalog.mjs',
          chunkFileNames: 'chunks/[name].mjs',
          assetFileNames: 'assets/[name][extname]',
        },
      },
    },
  });
  await import(pathToFileURL(join(directory, 'catalog.mjs')).href);
  await copyFile(vegaLiteSchema, join(output, 'vega-lite.schema.json'));
} finally {
  await rm(directory, { recursive: true, force: true });
}
