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
    // JSON.parse yields any — no `as unknown as NamingRules`.
    const invalid = JSON.parse('{"schema_version":"not-an-integer","rules":[]}');
    const ajv = validator.validateFormData(invalid, schema);
    expect(ajv.errors.length).toBeGreaterThan(0);
    const first = ajv.errors[0];
    expect(first).toBeDefined();
    expect(first!.message).toMatch(/must be integer/i);
    expect(first!.stack).toMatch(/schema_version.*must be integer/i);

    const { container } = render(
      <RulesForm schema={schema} value={invalid} onPreview={onPreview} />,
    );
    // Clicking the button alone is not reliable in happy-dom; submit the form.
    const form = container.querySelector('form');
    expect(form).toBeTruthy();
    fireEvent.submit(form!);

    // RJSF ErrorList renders error.stack; field error list renders error.message.
    const errorPanel = container.querySelector('.panel.errors');
    expect(errorPanel).toBeTruthy();
    expect(errorPanel!.textContent).toContain(first!.stack!);
    expect(errorPanel!.textContent).toMatch(/must be integer/i);
    const fieldErrors = container.querySelector('#root_schema_version__error');
    expect(fieldErrors).toBeTruthy();
    expect(fieldErrors!.textContent).toContain(first!.message!);
    expect(onPreview).not.toHaveBeenCalled();
  });
});
