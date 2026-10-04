/** Produce a single HTML MCP App with bundled code and styles; no CDN dependency. */
import { build } from 'esbuild';
import { mkdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

const result = await build({ entryPoints: ['src/mcp-apps/main.tsx'], bundle: true, write: false,
  minify: true, format: 'iife', platform: 'browser', target: 'es2022', outfile: 'app.js',
  define: { 'process.env.NODE_ENV': '"production"' }, loader: { '.css': 'css' } });
const script = result.outputFiles.find(file => file.path.endsWith('.js'))?.text;
const style = result.outputFiles.find(file => file.path.endsWith('.css'))?.text ?? '';
if (!script) throw new Error('Bundler returned no JavaScript');
const hash = value => createHash('sha256').update(value).digest('base64');
const safeScript = script.replaceAll('</script', '<\\/script');
const safeStyle = style.replaceAll('</style', '<\\/style');
const policy = `default-src 'none'; script-src 'sha256-${hash(safeScript)}'; style-src 'sha256-${hash(safeStyle)}'; img-src data: blob:; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`;
const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="${policy}"><title>okf-jawn source view</title><style>${safeStyle}</style></head><body><div id="root"></div><script>${safeScript}</script></body></html>\n`;
await mkdir('dist-apps', { recursive: true });
for (const name of ['source', 'changes', 'timeline', 'present']) await writeFile(`dist-apps/${name}.html`, html);
process.stdout.write('Built source, changes, timeline, and present resources from the shared result-dispatching component.\n');
