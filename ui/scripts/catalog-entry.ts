/** Emit the actual catalog's schemas and model guidance using json-render's library APIs. */
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { env } from 'node:process';
import { catalog } from '../src/features/views/catalog';

// Read through node:process so the bundler cannot fold OKF_CATALOG_OUT at build time.
const output = env.OKF_CATALOG_OUT;
if (!output) throw new Error('OKF_CATALOG_OUT must name a generation staging directory');
mkdirSync(output, { recursive: true });
writeFileSync(
  join(output, 'catalog.schema.json'),
  `${JSON.stringify(catalog.jsonSchema(), null, 2)}\n`,
);
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
writeFileSync(
  join(output, 'catalog-components.json'),
  `${JSON.stringify({ components: catalog.componentNames, actions: catalog.actionNames }, null, 2)}\n`,
);
