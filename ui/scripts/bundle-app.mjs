/**
 * Produce a single HTML MCP App with bundled code and styles; no CDN dependency.
 *
 * The App goes through Vite with the workspace's React compiler and Tailwind plugins, because the
 * stylesheet is Tailwind source that only the Tailwind compiler turns into real CSS. Any emitted
 * file other than one script and one stylesheet would be unreachable from the inline document and
 * fails the build.
 *
 * URI and mimeType must match the committed api/mcp-apps.json declaration.
 */

import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath, URL } from 'node:url';
import { RESOURCE_MIME_TYPE } from '@modelcontextprotocol/ext-apps';
import babel from '@rolldown/plugin-babel';
import tailwindcss from '@tailwindcss/vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import { build } from 'vite';

const declarationPath = fileURLToPath(new URL('../../api/mcp-apps.json', import.meta.url));
const declaration = JSON.parse(await readFile(declarationPath, 'utf8'));
const declared = declaration?.resources?.[0];
if (!declared || typeof declared.uri !== 'string' || typeof declared.mimeType !== 'string') {
  throw new Error('api/mcp-apps.json must declare exactly one resource with uri and mimeType');
}
if (declaration.resources.length !== 1) {
  throw new Error(`api/mcp-apps.json must list exactly one resource, got ${declaration.resources.length}`);
}
const expectedUri = 'ui://okf-jawn/app.html';
if (declared.uri !== expectedUri) {
  throw new Error(`api/mcp-apps.json uri ${declared.uri} !== ${expectedUri}`);
}
if (declared.mimeType !== RESOURCE_MIME_TYPE) {
  throw new Error(
    `api/mcp-apps.json mimeType ${declared.mimeType} !== SDK ${RESOURCE_MIME_TYPE}`,
  );
}

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
const file = 'dist-apps/app.html';
await writeFile(file, html);
const bytes = Buffer.from(html, 'utf8');
const resources = [
  {
    name: 'app',
    uri: declared.uri,
    mimeType: declared.mimeType,
    byteLength: bytes.byteLength,
    sha256: createHash('sha256').update(bytes).digest('hex'),
    csp: policy,
  },
];
await writeFile('dist-apps/manifest.json', `${JSON.stringify({ resources }, null, 2)}\n`);
process.stdout.write('Built shared ui://okf-jawn/app.html MCP App resource.\n');
