/**
 * An item's properties as a form, while its draft is edited.
 *
 * The form is built from the schema of the item's type: `list_types` answers each type's
 * `properties_schema` (JSON Schema) and `ui_schema`, and the item's `type_name` picks one. A
 * change is part of the item's draft: it goes into the session and is saved and snapshotted with
 * the text (`save_draft` carries the complete property map).
 *
 * The form holds only the properties its schema names. Every other property (an unknown key, an
 * OKF extension, the application's own header) is not given to the form at all: it is listed
 * read-only beside it and goes back into the map exactly as it was. Inside a property the form
 * does hold, keys its schema does not describe are kept by the form library (it drops none
 * unless told to).
 *
 * Opening the form is not an edit. A property the item does not hold is shown empty; a default its
 * schema names is not written into the draft unless the person enters it. Only the properties the
 * person changed in a field go from the form into the draft: a value the library puts into its
 * own data (one the schema fixes with `const`) stays out until the person changes that field.
 *
 * Invalid input is never put into the draft, so it is never sent: the form shows the error at the
 * field, keeps what was typed, and the draft keeps its last valid properties until the form is
 * valid again.
 *
 * Without a schema there is no form, and the properties are shown as they are: when the
 * workspace has no definition of the item's type, when the definition names no properties, when
 * the types could not be read (shown in plain words, with its retry), or when the form library
 * cannot draw the definition (it throws; `FormBoundary` catches it). In each case the properties
 * still travel with every save.
 */
import Form from '@rjsf/core';
import type { RJSFSchema, UiSchema } from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Component, type ReactNode, useRef, useState } from 'react';
import { z } from 'zod';
import { listTypesOptions } from '../../api/generated/@tanstack/react-query.gen';
import { editSession } from '../drafts/saving';
import type { DraftSession } from '../drafts/sessions';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { PropertyList } from './Properties';

export interface PropertiesFormProps {
  session: DraftSession;
  /** The item's user type name: the type whose schema describes its properties. */
  typeName: string;
}

type PropertyMap = Record<string, unknown>;

const validator = customizeValidator<PropertyMap>();
const zObject = z.record(z.string(), z.unknown());
/** A schema that names properties: an object's. */
const zNaming = z.object({ properties: zObject });
/**
 * The form fills in nothing by itself: a property the item does not hold is shown empty, whatever
 * default its schema names, and an absent list or group is not made an empty one.
 */
const NO_DEFAULTS = {
  emptyObjectFields: 'skipDefaults',
  arrayMinItems: { populate: 'never' },
} as const;
/** No submit button: a change is saved with the draft, not submitted. */
const NO_SUBMIT: UiSchema<PropertyMap> = { 'ui:submitButtonOptions': { norender: true } };

/** The properties a schema names, in its order; none when it is not an object schema. */
export function namedProperties(schema: PropertyMap): string[] {
  const named = zNaming.safeParse(schema);
  return named.success ? Object.keys(named.data.properties) : [];
}

/**
 * The complete property map after the form changed: every property the form does not hold is
 * exactly as it was, in its place; each one it holds takes the form's value, or leaves the map
 * when the form emptied it.
 */
export function mergeProperties(
  all: PropertyMap,
  held: readonly string[],
  form: PropertyMap,
): PropertyMap {
  const next: PropertyMap = {};
  for (const [key, value] of Object.entries(all)) {
    if (!held.includes(key)) next[key] = value;
    else if (form[key] !== undefined) next[key] = form[key];
  }
  for (const key of held) {
    if (!(key in next) && form[key] !== undefined) next[key] = form[key];
  }
  return next;
}

/**
 * Whatever the form library throws while it draws a type's schema or UI schema stops here. The
 * schema is the service's data, so the page must not fall to it: the caller shows the properties
 * as they are instead. A new type gets a new boundary (its `key`).
 */
class FormBoundary extends Component<
  { fallback: ReactNode; children: ReactNode },
  { failed: boolean }
> {
  override state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  override render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

/** The form library's options that render a description or a help text as Markdown. */
const MARKDOWN_OPTIONS: ReadonlySet<string> = new Set([
  'enableMarkdownInDescription',
  'enableMarkdownInHelp',
]);

/**
 * A type's UI schema without the options that turn on Markdown, at every depth and in either
 * spelling (`ui:enableMarkdownInDescription`, or the bare name under `ui:options` and
 * `ui:globalOptions`). A description and a help text are then plain text: no image is fetched and
 * no link is drawn because a schema said so. (The reader refuses images in documents the same way.)
 * A property that is itself named like one of the two options loses its UI hints with them.
 */
export function plainText<T>(uiSchema: T): T;
export function plainText(uiSchema: unknown): unknown {
  if (Array.isArray(uiSchema)) return uiSchema.map((entry) => plainText(entry));
  if (uiSchema === null || typeof uiSchema !== 'object') return uiSchema;
  return Object.fromEntries(
    Object.entries(uiSchema)
      .filter(([key]) => !MARKDOWN_OPTIONS.has(key.replace(/^ui:/, '')))
      .map(([key, value]) => [key, plainText(value)]),
  );
}

/** The properties whose value differs between two states of the form's data. */
export function changedKeys(before: PropertyMap, after: PropertyMap): string[] {
  return [...new Set([...Object.keys(before), ...Object.keys(after)])].filter(
    (key) => JSON.stringify(before[key]) !== JSON.stringify(after[key]),
  );
}

export function PropertiesForm({ session, typeName }: PropertiesFormProps) {
  const summaryRef = useRef<HTMLElement | null>(null);
  const types = useQuery(listTypesOptions({ body: { workspace_id: session.workspaceId } }));
  const typesFailure = useRetry({
    request: session.workspaceId,
    failed: types.isError,
    error: types.error,
    run: () => types.refetch(),
    loaded: () => summaryRef.current,
  });
  const definition = types.data?.items.find((type) => type.name === typeName);
  const schema = zObject.safeParse(definition?.properties_schema);
  const uiSchema = zObject.safeParse(definition?.ui_schema);
  const held = schema.success ? namedProperties(schema.data) : [];
  const others = Object.fromEntries(
    Object.entries(session.properties).filter(([key]) => !held.includes(key)),
  );
  const count = Object.keys(session.properties).length;

  return (
    <details className="properties" open>
      <summary ref={summaryRef}>Properties ({count})</summary>
      {/* Beside the form when the types were read before: a read that fails when asked again
          never takes the form, and what was typed in it, away. */}
      {typesFailure.shown && (
        <FailureNotice
          context="Reading this item’s type"
          error={typesFailure.error}
          retry={typesFailure}
          workspaceId={session.workspaceId}
        />
      )}
      {types.data === undefined ? (
        typesFailure.shown ? null : (
          <p role="status">Reading this item’s type…</p>
        )
      ) : definition === undefined ? (
        <p role="note" className="notice">
          This workspace has no definition of the type <span className="type-name">{typeName}</span>
          , so there is no form for these properties. They are kept as they are.
        </p>
      ) : !schema.success || held.length === 0 ? (
        <p role="note" className="notice">
          The type <span className="type-name">{typeName}</span> names no properties to edit. The
          ones below are kept as they are.
        </p>
      ) : (
        <FormBoundary
          key={typeName}
          fallback={
            <>
              <p role="note" className="notice">
                The form of the type <span className="type-name">{typeName}</span> cannot be shown
                here: its definition asks for something this workspace cannot draw. The properties
                are kept as they are and are saved with the text.
              </p>
              <PropertyList
                properties={Object.fromEntries(
                  Object.entries(session.properties).filter(([key]) => held.includes(key)),
                )}
              />
            </>
          }
        >
          <TypedForm
            session={session}
            schema={schema.data}
            uiSchema={uiSchema.success ? plainText(uiSchema.data) : {}}
            held={held}
          />
        </FormBoundary>
      )}
      {Object.keys(others).length > 0 && (
        <>
          {held.length > 0 && <p className="meta">Other properties, kept exactly as they are:</p>}
          <PropertyList properties={others} />
        </>
      )}
    </details>
  );
}

function TypedForm({
  session,
  schema,
  uiSchema,
  held,
}: {
  session: DraftSession;
  schema: PropertyMap;
  uiSchema: PropertyMap;
  held: readonly string[];
}) {
  const client = useQueryClient();
  // The form's own data: it also holds what is typed and not valid, which the draft never gets.
  const [data, setData] = useState<PropertyMap>(() =>
    Object.fromEntries(Object.entries(session.properties).filter(([key]) => held.includes(key))),
  );
  // The properties the person has changed in a field since the form opened: these, and no other,
  // go from the form into the draft.
  const entered = useRef(new Set<string>());
  const rules: RJSFSchema = schema;
  return (
    <Form<PropertyMap>
      className="properties-form"
      schema={rules}
      uiSchema={{ ...uiSchema, ...NO_SUBMIT }}
      formData={data}
      validator={validator}
      liveValidate="onChange"
      showErrorList={false}
      noHtml5Validate
      experimental_defaultFormStateBehavior={NO_DEFAULTS}
      onChange={(event, id) => {
        const next = event.formData ?? {};
        setData(next);
        // The library also reports what it made of the data itself (a value its schema fixes,
        // when it mounts); that names no field. Only what a person entered in a field is an edit.
        if (id === undefined) return;
        for (const key of changedKeys(data, next)) entered.current.add(key);
        if (!validator.isValid(rules, next, rules)) return;
        editSession(client, session.workspaceId, session.itemId, {
          properties: mergeProperties(session.properties, [...entered.current], next),
        });
      }}
    />
  );
}
