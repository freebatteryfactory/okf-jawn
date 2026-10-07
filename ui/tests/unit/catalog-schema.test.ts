/** Committed catalog.schema.json rejects bad props and accepts the six-component fixture. */

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { RJSFSchema } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { describe, expect, it } from 'vitest';
import catalogSchema from '../../../api/presentation/catalog.schema.json';
import sixComponentSpec from '../fixtures/six-component-spec.json';

const validator = customizeValidator();
const schema = catalogSchema as RJSFSchema;

function validate(instance: unknown) {
  return validator.validateFormData(instance, schema);
}

describe('catalog.schema.json', () => {
  it('accepts the six-component fixture', () => {
    const result = validate(sixComponentSpec);
    expect(result.errors).toEqual([]);
  });

  it('rejects unknown Chart props', () => {
    const result = validate({
      root: 'root',
      elements: {
        root: {
          type: 'Chart',
          props: { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics', extra: true },
          children: [],
        },
      },
    });
    expect(result.errors.length).toBeGreaterThan(0);
  });

  it('rejects SourceList bindings when not an array', () => {
    const result = validate({
      root: 'root',
      elements: {
        root: {
          type: 'SourceList',
          props: { bindings: 'venue' },
          children: [],
        },
      },
    });
    expect(result.errors.length).toBeGreaterThan(0);
  });

  it('rejects over-long binding and title', () => {
    const longBinding = 'b'.repeat(121);
    const longTitle = 't'.repeat(161);
    const bindingResult = validate({
      root: 'root',
      elements: {
        root: {
          type: 'SourceExcerpt',
          props: { binding: longBinding },
          children: [],
        },
      },
    });
    expect(bindingResult.errors.length).toBeGreaterThan(0);
    const titleResult = validate({
      root: 'root',
      elements: {
        root: {
          type: 'Chart',
          props: { binding: 'metrics', chart: 'metrics_chart', title: longTitle },
          children: [],
        },
      },
    });
    expect(titleResult.errors.length).toBeGreaterThan(0);
  });

  it('rejects an unknown component type', () => {
    const result = validate({
      root: 'root',
      elements: {
        root: {
          type: 'NotInCatalog',
          props: {},
          children: [],
        },
      },
    });
    expect(result.errors.length).toBeGreaterThan(0);
  });

  it('is generated next to the pinned vega-lite schema', () => {
    const presentation = join(dirname(fileURLToPath(import.meta.url)), '../../../api/presentation');
    const raw = readFileSync(join(presentation, 'vega-lite.schema.json'), 'utf8');
    expect(JSON.parse(raw).$schema).toMatch(/draft-07|draft\/2020-12/i);
  });
});
