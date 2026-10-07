/** The generated catalog document conforms to the contract's CatalogResponse. */

import type { RJSFSchema } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { describe, expect, it } from 'vitest';
import viewSchema from '../../../api/forms/view.schema.json';
import { catalogResponse, specJsonSchema } from '../../scripts/catalog-response';
import { zCatalogResponse } from '../../src/api/generated/zod.gen';
import { catalog } from '../../src/features/views/catalog';
import sixComponentSpec from '../fixtures/six-component-spec.json';

const validator = customizeValidator();

describe('catalog.json', () => {
  it('parses as CatalogResponse without losing or inventing a field', () => {
    const document = catalogResponse(viewSchema);
    expect(zCatalogResponse.parse(document)).toEqual(document);
    expect(Object.keys(document).sort()).toEqual(['components', 'schema_version', 'view_schema']);
  });

  it('describes every approved component with closed props and its actions', () => {
    const document = catalogResponse(viewSchema);
    expect(document.components.map((component) => component.name)).toEqual(catalog.componentNames);
    for (const component of document.components) {
      expect(Object.keys(component).sort()).toEqual([
        'actions',
        'description',
        'name',
        'properties_schema',
      ]);
      expect(component.description.length).toBeGreaterThan(0);
      expect(component.properties_schema).toMatchObject({
        type: 'object',
        additionalProperties: false,
      });
      expect(component.properties_schema).not.toHaveProperty('$schema');
      expect(component.actions).toEqual(catalog.actionNames);
    }
  });

  it('carries the persisted View document schema unchanged', () => {
    expect(catalogResponse(viewSchema).view_schema).toEqual(viewSchema);
  });
});

describe('stored spec schema derived from the single definition', () => {
  const schema = specJsonSchema() as RJSFSchema;

  it('accepts the six-component fixture', () => {
    expect(validator.validateFormData(sixComponentSpec, schema).errors).toEqual([]);
  });

  it('rejects props the component does not declare', () => {
    const result = validator.validateFormData(
      {
        root: 'root',
        elements: {
          root: {
            type: 'Chart',
            props: { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics', extra: true },
            children: [],
          },
        },
      },
      schema,
    );
    expect(result.errors.length).toBeGreaterThan(0);
  });

  it('rejects a component that is not in the catalog', () => {
    const result = validator.validateFormData(
      { root: 'root', elements: { root: { type: 'NotInCatalog', props: {}, children: [] } } },
      schema,
    );
    expect(result.errors.length).toBeGreaterThan(0);
  });

  it('rejects an element without children, which the stored shape requires', () => {
    const result = validator.validateFormData(
      { root: 'root', elements: { root: { type: 'Columns', props: {} } } },
      schema,
    );
    expect(result.errors.length).toBeGreaterThan(0);
  });
});
