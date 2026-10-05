/** Emit the actual catalog's schemas and model guidance using json-render's library APIs. */
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { env } from 'node:process';
import { z } from 'zod';
import { catalog } from '../src/features/views/catalog';

// Read through node:process so the bundler cannot fold OKF_CATALOG_OUT at build time.
const output = env.OKF_CATALOG_OUT;
if (!output) throw new Error('OKF_CATALOG_OUT must name a generation staging directory');

/** Per-component props from Zod; never emit unconstrained catalog.jsonSchema() props. */
function catalogSchema(): Record<string, unknown> {
  const elementBranches = catalog.componentNames.map((name) => {
    const entry = catalog.data.components[name];
    if (!entry?.props) throw new Error(`Catalog component ${name} has no props schema`);
    const props = z.toJSONSchema(entry.props, { target: 'draft-7' }) as Record<string, unknown>;
    // Nested object schemas must not carry a sibling $schema; keep draft-7 constraints only.
    const { $schema: _dropped, ...propsSchema } = props;
    return {
      type: 'object',
      properties: {
        type: { const: name },
        props: { ...propsSchema, additionalProperties: false },
        children: { type: 'array', items: { type: 'string' } },
        slots: {
          type: 'object',
          additionalProperties: { type: 'array', items: { type: 'string' } },
        },
        visible: {},
        repeat: {},
      },
      required: ['type', 'props', 'children'],
      additionalProperties: false,
    };
  });
  return {
    $schema: 'http://json-schema.org/draft-07/schema#',
    type: 'object',
    properties: {
      root: { type: 'string' },
      elements: {
        type: 'object',
        additionalProperties: { oneOf: elementBranches },
      },
      state: { type: 'object', additionalProperties: {} },
    },
    required: ['root', 'elements'],
    additionalProperties: false,
  };
}

mkdirSync(output, { recursive: true });
writeFileSync(join(output, 'catalog.schema.json'), `${JSON.stringify(catalogSchema(), null, 2)}\n`);
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
