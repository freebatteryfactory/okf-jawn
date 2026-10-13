/**
 * The naming rules as a form built from the Rust-defined schema. Saving and applying stay
 * explicit operations of the caller; the form's own button asks for a preview.
 *
 * The form holds the rules exactly as it is given them. A key the schema does not name is not
 * shown as a field, and it is not dropped either: every change the form reports carries it,
 * unchanged, so what was typed in the YAML view survives an edit made here. Because the service
 * accepts no unknown key, such a key is reported as a problem, by its name, until it is removed.
 *
 * Every problem is shown at its field and in a list above the form. A preview is asked only for
 * rules without a problem (`onPreview`); `onChange` reports every change, valid or not.
 */
import Form from '@rjsf/core';
import type { IconButtonProps, RJSFSchema, RJSFValidationError } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { flushSync } from 'react-dom';
import type { NamingRules } from '../../api/generated/types.gen';

const validator = customizeValidator<NamingRules>();
/** The same checks for a value that may be anything: what the YAML view holds. */
const anyValidator = customizeValidator();

export interface RulesFormProps {
  schema: RJSFSchema;
  value: NamingRules;
  /** Ask for a preview of valid rules; without it the form has no button of its own. */
  onPreview?: (value: NamingRules) => void;
  /** Every change, exactly as the form holds it: it may not be valid rules. */
  onChange?: (value: unknown) => void;
  /** A preview is being made: the button says so and asks for nothing more meanwhile. */
  previewing?: boolean;
}

/**
 * A value the form can hold: any mapping. Whether it is valid rules is what the form then says,
 * field by field; the type only names what it holds when it is.
 */
export function formHolds(value: unknown): value is NamingRules {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** What is wrong with `value` as naming rules by `schema`, in the form's own words. */
export function rulesProblems(schema: RJSFSchema, value: unknown): string[] {
  const { errors } = anyValidator.validateFormData(value, schema, undefined, named);
  return errors.map((error) => error.stack);
}

/** An unknown key is named, so the person can find and remove it. */
function named(errors: RJSFValidationError[]): RJSFValidationError[] {
  return errors.map((error) => {
    const key: unknown = error.params?.additionalProperty;
    if (error.name !== 'additionalProperties' || typeof key !== 'string') return error;
    const message = `has a key these rules do not have: “${key}”. Remove it; the service accepts no unknown key`;
    return { ...error, message, stack: `${error.property ?? ''} ${message}`.trim() };
  });
}

/**
 * A button of a list of rules, with its action in words (the library's own shows an icon font).
 *
 * Pressing one changes the list under keyboard focus: "Remove this rule" takes its own rule, and
 * itself, off the page, and a move puts the rule elsewhere. So the change is made at once
 * (`flushSync`) and focus that fell to the page body with it is put back before anything is
 * painted: on the button when it is still there, otherwise on "Add a rule", which always is.
 */
function worded(text: string, always = false) {
  return function WordedButton(props: IconButtonProps<NamingRules>) {
    const { id, onClick, disabled } = props;
    return (
      // aria-disabled rather than disabled: the first rule's "up" is unavailable the moment it
      // gets there, with keyboard focus on it.
      <button
        type="button"
        id={id}
        aria-disabled={disabled === true}
        {...(always ? { 'data-always-there': '' } : {})}
        onClick={(event) => {
          if (disabled === true) return;
          const button = event.currentTarget;
          const form = button.closest('form');
          flushSync(() => onClick?.(event));
          const active = document.activeElement;
          if (active !== null && active !== document.body) return;
          if (button.isConnected) button.focus();
          else form?.querySelector<HTMLElement>('button[data-always-there]')?.focus();
        }}
      >
        {text}
      </button>
    );
  };
}

const BUTTONS = {
  AddButton: worded('Add a rule', true),
  RemoveButton: worded('Remove this rule'),
  MoveUpButton: worded('Move this rule up'),
  MoveDownButton: worded('Move this rule down'),
  CopyButton: worded('Copy this rule'),
};

export function RulesForm({ schema, value, onPreview, onChange, previewing }: RulesFormProps) {
  return (
    <Form<NamingRules>
      className="rules-form"
      schema={schema}
      formData={value}
      liveValidate
      showErrorList="top"
      noHtml5Validate
      validator={validator}
      transformErrors={named}
      templates={{ ButtonTemplates: BUTTONS }}
      onChange={(event) => onChange?.(event.formData)}
      onSubmit={(event) => {
        if (event.formData && previewing !== true) onPreview?.(event.formData);
      }}
    >
      {onPreview === undefined ? (
        // No button of its own: the caller's buttons act on what `onChange` reported.
        <span />
      ) : (
        // aria-disabled rather than disabled: the button keeps focus while the preview is made.
        <button type="submit" aria-disabled={previewing === true}>
          {previewing === true ? 'Previewing…' : 'Preview naming changes'}
        </button>
      )}
    </Form>
  );
}
