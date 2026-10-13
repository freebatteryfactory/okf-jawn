/**
 * An item's properties as a form, while its draft is edited.
 *
 * The form is built from the schema of the item's type: `list_types` answers each type's
 * `properties_schema` (JSON Schema) and `ui_schema`, and the item's `type_name` picks one. A
 * change is part of the item's draft: it goes into the session and is saved and snapshotted with
 * the text (`save_draft` carries the complete property map).
 *
 * The form is given every property but the application's own header (`okf_jawn`), which is never
 * a field: it is taken out of the properties the schema names before the form sees the schema. The
 * form draws a field for each property the schema names, at its top or under a rule about the same
 * object (`allOf`, `if`/`then`, `dependencies`), and for those of the alternative chosen where a
 * type offers a choice (`oneOf`, `anyOf`): the library draws a selector, named here "Which of
 * these applies", and the chosen alternative's fields. Choosing another makes the library take
 * the properties of the one before out of its data; the form names each property of the item
 * that a change takes out elsewhere than in its own field, until it is entered again. Limits:
 * what such a change removes inside a property (a choice within a group) changes that property
 * and is not named; and which alternative is chosen is the library's own state, so a form that
 * opens again shows the alternative its data fits best.
 *
 * A property the form draws no field for (an unknown key, an OKF extension, the header) is listed
 * read-only beside it and goes back into the map exactly as it was, in its place. Inside a
 * property the form does draw, keys its schema does not describe are kept by the form library
 * (it drops none unless told to).
 *
 * Opening the form is not an edit. A property the item does not hold is shown empty; a default its
 * schema names is not written into the draft unless the person enters it. Only the properties the
 * person changed in a field go from the form into the draft: a value the library puts into its
 * own data (one the schema fixes with `const`) stays out until the person changes that field.
 *
 * What is judged is what a save would carry: the draft's whole property map with the person's
 * input in it, without the header, against the schema as the type gives it. That is what the
 * service judges. While it does not fit, none of the input is put into the draft, so it is never
 * sent: the draft keeps its last properties, and a line above the form says the input is held
 * back. What was typed is kept with the item's editing session (`heldBack`), not only in this
 * form: a form that opens later (the editor was left and opened again, or moved to another pane)
 * shows it again, and while it is there the item is marked unsaved and the page asks before it is
 * left. It lasts as long as the page, like the session: a reload the person confirms ends it.
 *
 * A misfit at a field the form draws is shown at that field by the form library; every other one
 * (a required property with no field, a property a strict type does not allow, a rule about the
 * properties together) is listed above the form, and only there: the library is not given the
 * errors about the map as a whole, which it would draw under the form in the validator's words.
 * The library judges a form when it is changed, so a form is asked to judge what it opens on,
 * once. A map that did not fit before anything was entered is therefore said the same way (at
 * its fields, and above the form), and nothing is held back by it until the person enters
 * something.
 *
 * Without a schema there is no form, and the properties are shown as they are: when the
 * workspace has no definition of the item's type, when the definition names no properties, when
 * the types could not be read (shown in plain words, with its retry), or when the form library
 * cannot draw the definition (it throws; `FormBoundary` catches it). In each case the properties
 * still travel with every save.
 */
import Form from '@rjsf/core';
import {
  type ErrorTransformer,
  type MultiSchemaFieldTemplateProps,
  type RJSFSchema,
  retrieveSchema,
  type UiSchema,
} from '@rjsf/utils';
import { customizeValidator } from '@rjsf/validator-ajv8';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Component,
  isValidElement,
  memo,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { z } from 'zod';
import { listTypesOptions } from '../../api/generated/@tanstack/react-query.gen';
import { keyed } from '../../lib/keys';
import { editSession, holdProperties } from '../drafts/saving';
import type { DraftSession, HeldProperties } from '../drafts/sessions';
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

/** The top-level property a validation error's path is in; '' for the map as a whole. */
const topProperty = (path: string | undefined) =>
  (path ?? '').replace(/^\./, '').split('.').at(0) ?? '';

/**
 * The form library draws an error about the map as a whole under the form, in the validator's
 * words. Such a misfit is said above the form already, once and in plain words, so the library is
 * left the errors that are about a property: it draws those of its own fields.
 */
const ONLY_AT_PROPERTIES: ErrorTransformer<PropertyMap> = (errors) =>
  errors.filter((error) => topProperty(error.property) !== '');

/** The form's ids: a top-level property's field is `root_<property>`, and what is in it below. */
const ID_PREFIX = 'root';
const ID_SEPARATOR = '_';

/** A change reported at the control `id` was made in the field of the property `key`. */
const isFieldOf = (id: string, key: string) => {
  const field = `${ID_PREFIX}${ID_SEPARATOR}${key}`;
  return id === field || id.startsWith(`${field}${ID_SEPARATOR}`);
};

/**
 * The properties a change took out of the form's data though it was not made in their own field:
 * the library drops what belongs to an alternative when another is chosen. Emptying a property in
 * its own field is the person's own doing, and is not among them.
 */
export function takenOutBy(id: string, before: PropertyMap, after: PropertyMap): string[] {
  return Object.keys(before).filter(
    (key) => before[key] !== undefined && after[key] === undefined && !isFieldOf(id, key),
  );
}

const NAMES = new Intl.ListFormat('en', { style: 'long', type: 'conjunction' });
const zIdentified = z.object({ id: z.string() });

/**
 * How a choice among a type's alternatives is laid out. The selector the library draws has no
 * name of its own (its theme draws none for it), so it is given a label here.
 */
function ChoiceTemplate({ selector, optionSchemaField }: MultiSchemaFieldTemplateProps) {
  const identified = isValidElement(selector) ? zIdentified.safeParse(selector.props) : null;
  return (
    <div className="choice">
      {identified?.success ? (
        <div className="form-group">
          <label htmlFor={identified.data.id}>Which of these applies</label>
          {selector}
        </div>
      ) : (
        selector
      )}
      {optionSchemaField}
    </div>
  );
}

const TEMPLATES = { MultiSchemaFieldTemplate: ChoiceTemplate };

/** What the form library is given to call with errors that are already shown. */
const NOTHING_MORE = () => {};

/** The application's own header among an item's properties: the service's, never a type's. */
const APP_HEADER_KEY = 'okf_jawn';

/** A property map without the application's header: what the service judges against a type. */
export function withoutHeader(properties: PropertyMap): PropertyMap {
  const { [APP_HEADER_KEY]: _header, ...judged } = properties;
  return judged;
}

/** The schema keywords whose value is a list of schemas about the same object. */
const SAME_OBJECT_LISTS = ['allOf', 'anyOf', 'oneOf'] as const;
/** Those whose value is one schema about the same object (`if` only asks, and draws nothing). */
const SAME_OBJECT_ONE = ['then', 'else'] as const;
/** Those whose value is a map of schemas that may be about the same object, or be referred to. */
const SAME_OBJECT_MAPS = ['dependencies', 'dependentSchemas', 'definitions', '$defs'] as const;

/**
 * A type's schema as the form is given it: the same rules, with the application's header taken
 * out of the properties it names, at its top and in every rule about the same object, so the
 * header is never a field whatever the schema names. The properties are judged without the header
 * (the service judges them so), and for those a schema says the same with or without this entry;
 * a `required` that names the header is left as it is, and is said above the form.
 */
export function withoutHeaderField<T>(schema: T): T;
export function withoutHeaderField(schema: unknown): unknown {
  const read = zObject.safeParse(schema);
  if (!read.success) return schema;
  const next: PropertyMap = { ...read.data };
  const named = zObject.safeParse(next.properties);
  if (named.success) next.properties = withoutHeader(named.data);
  for (const keyword of SAME_OBJECT_LISTS) {
    const list = z.array(z.unknown()).safeParse(next[keyword]);
    if (list.success) next[keyword] = list.data.map((entry) => withoutHeaderField(entry));
  }
  for (const keyword of SAME_OBJECT_ONE) {
    if (keyword in next) next[keyword] = withoutHeaderField(next[keyword]);
  }
  for (const keyword of SAME_OBJECT_MAPS) {
    const map = zObject.safeParse(next[keyword]);
    if (map.success) {
      next[keyword] = Object.fromEntries(
        Object.entries(map.data).map(([name, entry]) => [name, withoutHeaderField(entry)]),
      );
    }
  }
  return next;
}

/** The keywords that offer a choice among schemas about the same object. */
const CHOICES = ['oneOf', 'anyOf'] as const;

/**
 * The properties the form draws a field for, with the data it holds: those the schema names at its
 * top, and those its rules bring in (`allOf`, `if`/`then`, `dependencies`), as the form library
 * itself resolves them. Where the schema offers a choice (`oneOf`, `anyOf`) the library draws the
 * fields of the alternative chosen; which one that is, is the library's to know (it keeps the
 * person's choice), so a property any alternative names counts as the form's to draw.
 */
export function drawnProperties(schema: PropertyMap, data: PropertyMap): string[] {
  const resolve = (part: PropertyMap) => {
    const resolved = retrieveSchema<PropertyMap>(validator, part, schema, data);
    const named = zNaming.safeParse(resolved);
    return { resolved, names: named.success ? Object.keys(named.data.properties) : [] };
  };
  const top = resolve(schema);
  const names = new Set(top.names);
  for (const keyword of CHOICES) {
    const alternatives = z.array(zObject).safeParse(top.resolved[keyword]);
    if (!alternatives.success) continue;
    for (const alternative of alternatives.data) {
      for (const name of resolve(alternative).names) names.add(name);
    }
  }
  return [...names];
}

/** One way the properties do not fit the type: the property it is about ('' for the whole map). */
export interface Misfit {
  property: string;
  /** In plain words, a sentence. */
  words: string;
}

const zMissing = z.object({ missingProperty: z.string() });
const zAdditional = z.object({ additionalProperty: z.string() });

/**
 * Every way `properties` (without the application's header) do not fit the type's schema. What a
 * missing property and one the type does not allow mean is said in plain words; any other rule is
 * said as the validator says it, with the property it is about. That an `if` did not hold is not a
 * misfit of its own: the rule under it that failed is one, and is said.
 */
export function misfits(schema: PropertyMap, properties: PropertyMap): Misfit[] {
  const errors = validator.validateFormData(properties, schema).errors;
  return errors
    .filter((error) => error.name !== 'if')
    .map((error) => {
      const missing = zMissing.safeParse(error.params);
      if (error.name === 'required' && missing.success) {
        const property = missing.data.missingProperty;
        return { property, words: `“${property}” is required, and this item does not have it.` };
      }
      const additional = zAdditional.safeParse(error.params);
      if (error.name === 'additionalProperties' && additional.success) {
        const property = additional.data.additionalProperty;
        return { property, words: `“${property}” is not a property this type allows.` };
      }
      const property = topProperty(error.property);
      const said = error.message ?? error.stack;
      return {
        property,
        words:
          property === ''
            ? `The properties together do not follow a rule of this type (it says: ${said}).`
            : `“${property}” ${said}.`,
      };
    });
}

/**
 * The complete property map after the form changed: every property not among `held` is exactly
 * as it was, in its place; each one among them takes the form's value, or leaves the map when the
 * form emptied it. One the map does not have goes back where it was in `order` (the map as it was
 * when the form opened), before the first property that followed it there; one that was never in
 * the map is added at its end.
 */
export function mergeProperties(
  all: PropertyMap,
  held: readonly string[],
  form: PropertyMap,
  order: readonly string[] = [],
): PropertyMap {
  const next: [string, unknown][] = [];
  for (const [key, value] of Object.entries(all)) {
    if (!held.includes(key)) next.push([key, value]);
    else if (form[key] !== undefined) next.push([key, form[key]]);
  }
  for (const key of held) {
    if (key in all || form[key] === undefined) continue;
    const was = order.indexOf(key);
    const followed = was < 0 ? -1 : next.findIndex(([other]) => order.indexOf(other) > was);
    if (followed < 0) next.push([key, form[key]]);
    else next.splice(followed, 0, [key, form[key]]);
  }
  return Object.fromEntries(next);
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
  // The schema and the UI schema as the form is given them are made once for a definition, not
  // at each drawing: the form below is drawn again, and its properties judged again, only when
  // what it is given changes, and a keystroke in the text changes none of it.
  const typeSchema = definition?.properties_schema;
  const schema = useMemo(() => {
    const read = zObject.safeParse(typeSchema);
    return read.success ? withoutHeaderField(read.data) : null;
  }, [typeSchema]);
  const typeUiSchema = definition?.ui_schema;
  const uiSchema = useMemo(() => {
    const read = zObject.safeParse(typeUiSchema);
    return { ...(read.success ? plainText(read.data) : {}), ...NO_SUBMIT };
  }, [typeUiSchema]);
  const count = Object.keys(session.properties).length;
  const asTheyAre = <PropertyList properties={session.properties} />;

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
        <>
          {typesFailure.shown ? null : <p role="status">Reading this item’s type…</p>}
          {asTheyAre}
        </>
      ) : definition === undefined ? (
        <>
          <p role="note" className="notice">
            This workspace has no definition of the type{' '}
            <span className="type-name">{typeName}</span>, so there is no form for these properties.
            They are kept as they are.
          </p>
          {asTheyAre}
        </>
      ) : schema === null ? (
        <>
          <p role="note" className="notice">
            The type <span className="type-name">{typeName}</span> names no properties to edit. The
            ones below are kept as they are.
          </p>
          {asTheyAre}
        </>
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
              {asTheyAre}
            </>
          }
        >
          <TypedForm
            key={`${session.workspaceId}/${session.itemId}`}
            workspaceId={session.workspaceId}
            itemId={session.itemId}
            properties={session.properties}
            heldBack={session.heldBack}
            typeName={typeName}
            schema={schema}
            uiSchema={uiSchema}
          />
        </FormBoundary>
      )}
    </details>
  );
}

/**
 * The form of one item's properties. It is given the session's property map and its held-back
 * input, not the session: a session changes at every keystroke in the text, and those two do
 * not. So the form is drawn, and the properties judged against the type, when the properties,
 * the held-back input or the type change, and not when the text does (`memo`).
 */
const TypedForm = memo(function TypedForm({
  workspaceId,
  itemId,
  properties,
  heldBack: heldBefore,
  typeName,
  schema,
  uiSchema,
}: {
  workspaceId: string;
  itemId: string;
  /** The draft's complete property map. */
  properties: PropertyMap;
  /** What a form before this one held back, if anything. */
  heldBack: HeldProperties | null;
  typeName: string;
  schema: PropertyMap;
  uiSchema: UiSchema<PropertyMap>;
}) {
  const client = useQueryClient();
  // Input the draft has not taken is kept with the session, so a form that opens finds what the
  // one before it held back (the editor was left, or moved to another pane).
  const [held] = useState(() => heldBefore);
  // The property map as it was when the form opened: where a property emptied and entered again
  // goes back to.
  const [opened] = useState(() => held?.opened ?? Object.keys(properties));
  // The form's own data: every property but the application's header. It also holds what is typed
  // and not valid, which the draft never gets.
  const [data, setData] = useState<PropertyMap>(() => held?.form ?? withoutHeader(properties));
  // The properties the person has changed in a field since the form opened: these, and no other,
  // go from the form into the draft.
  const [entered, setEntered] = useState<readonly string[]>(() => held?.entered ?? []);
  // The properties a choice took out of the form's data (see `takenOutBy`), named above the form
  // until they are entered again.
  const [takenOut, setTakenOut] = useState<readonly string[]>(() => held?.takenOut ?? []);
  // The form library says what is wrong at a field once that form has been changed. So a form is
  // asked to judge what it opens on, once: stored properties that do not fit at a field, and
  // input held back before, have their reason at the field from the start.
  const formRef = useRef<Form<PropertyMap> | null>(null);
  useEffect(() => {
    formRef.current?.validateForm();
  }, []);
  const rules: RJSFSchema = schema;
  const drawn = drawnProperties(schema, data);
  // What a save would carry if the form's input went into the draft now: it is this, without the
  // application's header, that the service judges against the type, so it is this that is judged.
  const wanted = mergeProperties(properties, entered, data, opened);
  const heldBack = changedKeys(properties, wanted).length > 0;
  // What the form shows at a field of its own needs no second place.
  const atFields = misfits(schema, data).filter((misfit) => drawn.includes(misfit.property));
  const unplaced = misfits(schema, withoutHeader(wanted)).filter(
    (misfit) =>
      !atFields.some((at) => at.property === misfit.property && at.words === misfit.words),
  );
  const others = Object.fromEntries(
    Object.entries(properties).filter(([key]) => !drawn.includes(key)),
  );
  // Always there, so what it comes to say is announced: the misfits no field shows, and that input
  // is held back.
  const said = (
    <div role="status" className="form-said">
      {unplaced.length > 0 && (
        <div className="notice">
          <p>
            <strong>These properties do not fit the type {typeName}:</strong>
          </p>
          <ul>
            {keyed(unplaced, (misfit) => misfit.words).map(({ key, value: misfit }) => (
              <li key={key}>{misfit.words}</li>
            ))}
          </ul>
        </div>
      )}
      {takenOut.length > 0 && (
        <p className="notice">
          What you chose takes {NAMES.format(takenOut.map((name) => `“${name}”`))} out of this
          item’s properties.
        </p>
      )}
      {heldBack && (
        <p className="notice">
          What you changed in the properties is held back, and is not saved until the properties fit
          the type.
        </p>
      )}
    </div>
  );
  if (drawn.length === 0) {
    return (
      <>
        <p role="note" className="notice">
          The type <span className="type-name">{typeName}</span> names no properties to edit. The
          ones below are kept as they are.
        </p>
        {said}
        <PropertyList properties={properties} />
      </>
    );
  }
  return (
    <>
      {said}
      <Form<PropertyMap>
        ref={formRef}
        className="properties-form"
        idPrefix={ID_PREFIX}
        idSeparator={ID_SEPARATOR}
        templates={TEMPLATES}
        // Asked to judge what it opens on (above), the library reports what it found here; it is
        // on the page already, at the fields and above the form.
        onError={NOTHING_MORE}
        schema={rules}
        uiSchema={uiSchema}
        formData={data}
        validator={validator}
        liveValidate="onChange"
        transformErrors={ONLY_AT_PROPERTIES}
        showErrorList={false}
        noHtml5Validate
        experimental_defaultFormStateBehavior={NO_DEFAULTS}
        onChange={(event, id) => {
          const next = event.formData ?? {};
          setData(next);
          // The library also reports what it made of the data itself (a value its schema fixes,
          // when it mounts); that names no field. Only what a person entered in a field is an edit.
          if (id === undefined) return;
          const now = [
            ...new Set([
              ...entered,
              ...changedKeys(data, next).filter((key) => key !== APP_HEADER_KEY),
            ]),
          ];
          setEntered(now);
          // What this change took out elsewhere than in its own field, with what was taken out
          // before and is still out.
          const out = [...new Set([...takenOut, ...takenOutBy(id, data, next)])].filter(
            (key) => next[key] === undefined,
          );
          setTakenOut(out);
          const candidate = mergeProperties(properties, now, next, opened);
          // Judged as the service judges it: the whole map, without the application's header.
          if (!validator.isValid(rules, withoutHeader(candidate), rules)) {
            // None of it goes into the draft. It is kept with the session, not only in this
            // form, unless the input is back to what the draft has.
            holdProperties(
              workspaceId,
              itemId,
              changedKeys(properties, candidate).length > 0
                ? { form: next, entered: now, opened: [...opened], takenOut: out }
                : null,
            );
            return;
          }
          editSession(client, workspaceId, itemId, { properties: candidate });
        }}
      />
      {Object.keys(others).length > 0 && (
        <>
          <p className="meta">Other properties, kept exactly as they are:</p>
          <PropertyList properties={others} />
        </>
      )}
    </>
  );
});
