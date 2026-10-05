/** Emit the catalog's published files from json-render's library APIs and the single spec shape. */
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { env } from 'node:process';
import { catalog } from '../src/features/views/catalog';
import { catalogResponse, specJsonSchema } from './catalog-response';

// Read through node:process so the bundler cannot fold these at build time.
const output = env.OKF_CATALOG_OUT;
if (!output) throw new Error('OKF_CATALOG_OUT must name a generation staging directory');
const viewSchemaPath = env.OKF_VIEW_SCHEMA;
if (!viewSchemaPath)
  throw new Error('OKF_VIEW_SCHEMA must name the generated forms/view.schema.json');
const viewSchema: unknown = JSON.parse(readFileSync(viewSchemaPath, 'utf8'));

function writeJson(name: string, value: unknown): void {
  writeFileSync(join(output as string, name), `${JSON.stringify(value, null, 2)}\n`);
}

mkdirSync(output, { recursive: true });
writeJson('catalog.schema.json', specJsonSchema());
writeJson('catalog.json', catalogResponse(viewSchema));
writeFileSync(
  join(output, 'catalog-prompt.txt'),
  `${catalog.prompt({
    customRules: [
      'Source components receive binding names, never model-written quotations or data.',
      'Use only the resolved sources provided by the application. Missing bindings must remain unresolved.',
      'Presentation does not grant permission to modify, approve, or verify content.',
    ],
  })}\n`,
);
writeJson('catalog-components.json', {
  components: catalog.componentNames,
  actions: catalog.actionNames,
});
