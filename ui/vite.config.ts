/** Build the human workspace without starting the application backend. */
import { readdirSync, readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import babel from '@rolldown/plugin-babel';
import tailwindcss from '@tailwindcss/vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import { defineConfig, type Plugin } from 'vite';

/**
 * PDF.js's own files, from the installed distribution, as files of the build that `'self'` serves
 * (UI M4a, ruling A2; `src/features/viewers/pdf/pdfjs.ts` names them to PDF.js): its worker, its
 * standard fonts, its CMaps and the script fallbacks of its image decoders, with their licences,
 * under `pdfjs-<version>/` in the build output, and at the same paths on the dev server. The page
 * policy has `worker-src 'self'`, `connect-src 'self'` and `script-src 'self'` and nothing else, so
 * no CDN and no `blob:` worker: the worker is started from its own file of this origin, and fetches
 * the fonts and CMaps from here.
 *
 * - The worker is the legacy build's (`legacy/build/pdf.worker.min.mjs`), as the viewer's library
 *   is: the desktop launcher shows the workspace in the system WebKit (SPEC section 2), which can
 *   be older than the modern build needs. It is copied byte for byte under a `.js` name: a module
 *   worker needs a JavaScript media type, and `.js` is the extension every static server, the
 *   test servers included, gives one.
 * - The version in the path is the installed package's; the viewer builds the same path from the
 *   library's own `version`, so a worker is never paired with another version of the library.
 * - The `.wasm` decoders are not copied: WebAssembly needs `'wasm-unsafe-eval'`, which the policy
 *   does not grant, so the viewer turns it off and PDF.js uses these script fallbacks instead.
 */
function pdfjsFiles(): Plugin {
  const root = fileURLToPath(new URL('./node_modules/pdfjs-dist/', import.meta.url));
  const { version } = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as {
    version: string;
  };
  const base = `pdfjs-${version}/`;
  /** Each file's path in the build, and where the distribution has it. */
  const files = new Map<string, string>([
    [`${base}pdf.worker.min.js`, join(root, 'legacy/build/pdf.worker.min.mjs')],
    [`${base}LICENSE`, join(root, 'LICENSE')],
  ]);
  for (const folder of ['standard_fonts', 'cmaps']) {
    for (const name of readdirSync(join(root, folder))) {
      files.set(`${base}${folder}/${name}`, join(root, folder, name));
    }
  }
  for (const name of readdirSync(join(root, 'wasm'))) {
    if (name.endsWith('_nowasm_fallback.js') || name.startsWith('LICENSE')) {
      files.set(`${base}wasm/${name}`, join(root, 'wasm', name));
    }
  }
  return {
    name: 'okf-pdfjs-files',
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const path = (request.url ?? '').split('?', 1)[0]?.replace(/^\//, '') ?? '';
        const source = files.get(path);
        if (source === undefined) {
          next();
          return;
        }
        readFile(source).then((body) => {
          response.setHeader(
            'Content-Type',
            path.endsWith('.js') ? 'text/javascript' : 'application/octet-stream',
          );
          response.end(body);
        }, next);
      });
    },
    async generateBundle() {
      for (const [fileName, source] of files) {
        this.emitFile({ type: 'asset', fileName, source: await readFile(source) });
      }
    },
  };
}

export default defineConfig(({ command }) => {
  // React picks its development or production build from NODE_ENV, which Vite does not override
  // for `vite build`. A build never ships a development React; the dev server stays development.
  if (command === 'build') process.env.NODE_ENV = 'production';
  return {
    plugins: [
      tanstackRouter({ target: 'react', autoCodeSplitting: true }),
      react(),
      babel({ presets: [reactCompilerPreset()] }),
      tailwindcss(),
      pdfjsFiles(),
    ],
    resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
    server: { host: '127.0.0.1', proxy: { '/api': 'http://127.0.0.1:7711' } },
    build: { target: 'es2022', sourcemap: true },
  };
});
