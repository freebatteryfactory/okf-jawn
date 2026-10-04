/** Vendor the installed Scalar browser bundle; documentation does not require a CDN. */

import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';

const require = createRequire(import.meta.url);
let directory = dirname(require.resolve('@scalar/api-reference'));
let pkg;
while (true) {
  try {
    const candidate = JSON.parse(await readFile(join(directory, 'package.json'), 'utf8'));
    if (candidate.name === '@scalar/api-reference') {
      pkg = candidate;
      break;
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const parent = dirname(directory);
  if (parent === directory) throw new Error('Cannot find installed Scalar package metadata');
  directory = parent;
}
if (typeof pkg.browser !== 'string')
  throw new Error('Selected Scalar package has no declared standalone browser entry');
const source = resolve(directory, pkg.browser);
const inside = relative(directory, source);
if (!inside || inside.startsWith('..') || isAbsolute(inside))
  throw new Error('Browser asset escapes package');
const out = 'dist/docs';
await mkdir(out, { recursive: true });
await copyFile(source, join(out, 'scalar.js'));
await copyFile('../api/openapi.json', join(out, 'openapi.json'));
await writeFile(
  join(out, 'init.js'),
  "Scalar.createApiReference('#app', { url: './openapi.json', proxyUrl: '', telemetry: false });\n",
);
await writeFile(
  join(out, 'index.html'),
  '<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>okf-jawn API</title></head><body><div id="app"></div><script src="./scalar.js"></script><script src="./init.js"></script></body></html>\n',
);
process.stdout.write(`Vendored Scalar ${pkg.version}; no external CDN loader.\n`);
