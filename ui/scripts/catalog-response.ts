/** The published catalog document: the contract's `CatalogResponse`, built from the real catalog. */
import { z } from 'zod';
import { catalog } from '../src/features/views/catalog';
import { type SpecComponent, storedSpecJsonSchema } from '../src/features/views/spec-schema';

/** Wire shape of `CatalogResponse`; the generated Zod schema is the conformance check. */
export interface CatalogDocument {
  schema_version: number;
  components: {
    name: string;
    description: string;
    properties_schema: Record<string, unknown>;
    actions: string[];
  }[];
  view_schema: unknown;
}

export const CATALOG_SCHEMA_VERSION = 1;

/** Every approved component with its description, in catalog order. */
export function specComponents(): (SpecComponent & { description: string })[] {
  return Object.entries(catalog.data.components).map(([name, entry]) => ({
    name,
    description: entry.description,
    props: entry.props,
  }));
}

/** JSON Schema of the stored json-render spec for this catalog. */
export function specJsonSchema(): Record<string, unknown> {
  return storedSpecJsonSchema(specComponents());
}

/** Build the catalog document; `viewSchema` is the generated schema of a persisted View document. */
export function catalogResponse(viewSchema: unknown): CatalogDocument {
  return {
    schema_version: CATALOG_SCHEMA_VERSION,
    components: specComponents().map(({ name, description, props }) => {
      const { $schema: _dialect, ...properties } = z.toJSONSchema(z.strictObject(props.shape), {
        target: 'draft-7',
      });
      return {
        name,
        description,
        properties_schema: properties,
        actions: [...catalog.actionNames],
      };
    }),
    view_schema: viewSchema,
  };
}
