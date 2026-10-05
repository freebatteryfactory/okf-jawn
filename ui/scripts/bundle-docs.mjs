/** Vendor the installed swagger-ui-dist assets against the canonical OpenAPI YAML; no CDN. */

import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';

const require = createRequire(import.meta.url);
let directory = dirname(require.resolve('swagger-ui-dist/swagger-ui-bundle.js'));
let pkg;
while (true) {
  try {
    const candidate = JSON.parse(await readFile(join(directory, 'package.json'), 'utf8'));
    if (candidate.name === 'swagger-ui-dist') {
      pkg = candidate;
      break;
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const parent = dirname(directory);
  if (parent === directory)
    throw new Error('Cannot find installed swagger-ui-dist package metadata');
  directory = parent;
}
const assets = ['swagger-ui-bundle.js', 'swagger-ui-standalone-preset.js', 'swagger-ui.css'];
for (const name of assets) {
  const source = resolve(directory, name);
  const inside = relative(directory, source);
  if (!inside || inside.startsWith('..') || isAbsolute(inside))
    throw new Error(`Docs asset escapes package: ${name}`);
}
const out = 'dist/docs';
await mkdir(out, { recursive: true });
for (const name of assets) await copyFile(join(directory, name), join(out, name));
await copyFile('../api/openapi.yaml', join(out, 'openapi.yaml'));
await writeFile(
  join(out, 'init.js'),
  `window.ui = SwaggerUIBundle({
  url: './openapi.yaml',
  dom_id: '#swagger-ui',
  presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
  layout: 'StandaloneLayout',
  validatorUrl: null
});
`,
);
await writeFile(
  join(out, 'index.html'),
  `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>okf-jawn API</title>
  <link rel="stylesheet" href="./swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="./swagger-ui-bundle.js"></script>
  <script src="./swagger-ui-standalone-preset.js"></script>
  <script src="./init.js"></script>
</body>
</html>
`,
);
process.stdout.write(`Vendored swagger-ui-dist ${pkg.version}; no external CDN loader.\n`);
