/**
 * Bundle and execute the catalog module with installed dependencies; no hand-written schema substitute.
 * Output names are fixed (no content hashes) so gen-check can prove two generations are byte-identical.
 */

import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL, URL } from 'node:url';
import { build } from 'vite';

const root = fileURLToPath(new URL('..', import.meta.url));
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
} finally {
  await rm(directory, { recursive: true, force: true });
}
