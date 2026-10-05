/** RulesForm mounts with the shared NamingRules generic on validator and Form. */

import type { RJSFSchema } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import namingSchema from '../../../api/forms/naming-rules.schema.json';
import type { NamingRules } from '../../src/api/generated/types.gen';
import { RulesForm } from '../../src/features/conventions/RulesForm';

const schema = namingSchema as RJSFSchema;

const validValue: NamingRules = {
  schema_version: 1,
  rules: [
    {
      glob: '**/*.md',
      case: 'kebab',
      separator: '-',
      prefix: '',
      date_order: 'ymd',
      date_format: '%Y-%m-%d',
      strip_version_suffix: true,
      collision: 'reject',
    },
  ],
};

describe('RulesForm', () => {
  it('mounts with valid NamingRules data', () => {
    const onPreview = vi.fn();
    render(<RulesForm schema={schema} value={validValue} onPreview={onPreview} />);
    expect(screen.getByRole('button', { name: /preview naming changes/i })).toBeTruthy();
  });

  it('produces AJV errors for invalid NamingRules data', () => {
    const validator = customizeValidator<NamingRules>();
    const invalid = {
      schema_version: 'not-an-integer',
      rules: validValue.rules,
    } as unknown as NamingRules;
    const result = validator.validateFormData(invalid, schema);
    expect(result.errors.length).toBeGreaterThan(0);
  });
});
