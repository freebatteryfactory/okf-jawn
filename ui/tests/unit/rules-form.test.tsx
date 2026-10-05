/** RulesForm mounts with the shared NamingRules generic on validator and Form. */

import type { RJSFSchema } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import namingSchema from '../../../api/forms/naming-rules.schema.json';
import type { NamingRules } from '../../src/api/generated/types.gen';
import { RulesForm } from '../../src/features/conventions/RulesForm';

const schema = namingSchema as RJSFSchema;
const validator = customizeValidator<NamingRules>();

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
      extension_policy: 'preserve',
      alias_preservation: true,
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

  it('displays AJV errors for invalid NamingRules data', () => {
    const onPreview = vi.fn();
    const invalid = JSON.parse(
      '{"schema_version":"not-an-integer","rules":[]}',
    ) as Record<string, unknown>;
    const ajv = validator.validateFormData(invalid, schema);
    expect(ajv.errors.length).toBeGreaterThan(0);
    expect(ajv.errors.some((error) => /schema_version|type|integer/i.test(JSON.stringify(error)))).toBe(
      true,
    );

    const { container } = render(
      <RulesForm schema={schema} value={invalid} onPreview={onPreview} />,
    );
    fireEvent.click(screen.getByRole('button', { name: /preview naming changes/i }));
    const body = container.textContent ?? '';
    expect(body).toMatch(/schema_version|integer|type/i);
    expect(ajv.errors.map((error) => error.message ?? '').join(' ')).toMatch(
      /integer|type|must|should/i,
    );
  });
});
