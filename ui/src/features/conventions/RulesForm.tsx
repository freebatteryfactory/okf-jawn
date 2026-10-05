/** Edit the Rust-defined naming schema through a form; persistence remains an explicit operation. */
import Form from '@rjsf/core';
import type { RJSFSchema } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import type { NamingRules } from '../../api/generated/types.gen';

const validator = customizeValidator<NamingRules>();

export interface RulesFormProps {
  schema: RJSFSchema;
  value: NamingRules;
  onPreview: (value: NamingRules) => void;
}

export function RulesForm({ schema, value, onPreview }: RulesFormProps) {
  return (
    <Form<NamingRules>
      schema={schema}
      formData={value}
      validator={validator}
      onSubmit={(event) => {
        if (event.formData) onPreview(event.formData);
      }}
    >
      <button type="submit">Preview naming changes</button>
    </Form>
  );
}
