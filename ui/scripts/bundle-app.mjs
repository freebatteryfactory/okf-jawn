/**
 * Produce a single HTML MCP App with bundled code and styles; no CDN dependency.
 *
 * The App goes through Vite with the workspace's React compiler and Tailwind plugins, because the
 * stylesheet is Tailwind source that only the Tailwind compiler turns into real CSS. Any emitted
 * file other than one script and one stylesheet would be unreachable from the inline document and
 * fails the build.
 */

import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath, URL } from 'node:url';
import babel from '@rolldown/plugin-babel';
import tailwindcss from '@tailwindcss/vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import { build } from 'vite';

const output = await build({
  configFile: false,
  logLevel: 'warn',
  mode: 'production',
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('../src', import.meta.url)) } },
  build: {
    write: false,
    target: 'es2022',
    minify: true,
    sourcemap: false,
    cssCodeSplit: false,
    modulePreload: false,
    assetsInlineLimit: Number.POSITIVE_INFINITY,
    rolldownOptions: {
      input: fileURLToPath(new URL('../src/mcp-apps/main.tsx', import.meta.url)),
      output: { format: 'iife', codeSplitting: false },
    },
  },
});
if (!output || Array.isArray(output) || !('output' in output))
  throw new Error('Bundler returned an unexpected result shape');
const chunks = output.output.filter((file) => file.type === 'chunk');
const assets = output.output.filter((file) => file.type === 'asset');
const styles = assets.filter((file) => file.fileName.endsWith('.css'));
const unreachable = assets.filter((file) => !file.fileName.endsWith('.css'));
if (chunks.length !== 1) throw new Error(`Expected one script, got ${chunks.length}`);
if (styles.length > 1) throw new Error(`Expected at most one stylesheet, got ${styles.length}`);
if (unreachable.length > 0)
  throw new Error(
    `Unreachable emitted files: ${unreachable.map((file) => file.fileName).join(', ')}`,
  );
const script = chunks[0].code;
const style = styles.length === 1 ? String(styles[0].source) : '';
const hash = (value) => createHash('sha256').update(value).digest('base64');
const safeScript = script.replaceAll('</script', '<\\/script');
const safeStyle = style.replaceAll('</style', '<\\/style');
const policy = `default-src 'none'; script-src 'sha256-${hash(safeScript)}'; style-src 'sha256-${hash(safeStyle)}'; img-src data: blob:; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`;
const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="${policy}"><title>okf-jawn source view</title><style>${safeStyle}</style></head><body><div id="root"></div><script>${safeScript}</script></body></html>\n`;
await mkdir('dist-apps', { recursive: true });
for (const name of ['source', 'changes', 'timeline', 'present'])
  await writeFile(`dist-apps/${name}.html`, html);
process.stdout.write(
  'Built source, changes, timeline, and present resources from the shared result-dispatching component.\n',
);
