/**
 * An item's properties as YAML, while its draft is edited: the advanced raw view beside the form
 * (SPEC section 3), and the two views together (`PropertiesViews`). Both are views of the same
 * thing, the draft session's property map; a change in either is part of the draft and is saved
 * and snapshotted with the text. One view is on screen at a time, and each opens on the draft's
 * properties as they then are, so a change made in one shows in the other.
 *
 * The text is every property but the application's own header (`okf_jawn`), which is the
 * service's and is never typed here: it goes back into the map exactly as it was, at its place.
 * Every other property is in the text, a key no type knows included, so nothing is dropped: what
 * the text holds is what the map becomes, in the order the text has it.
 *
 * Nothing that cannot be sent is put into the draft, so it is never sent:
 * - YAML that cannot be read (`lib/yaml.ts`), with each problem at its line;
 * - text that is not a mapping of names to values, or that names the header;
 * - properties that do not fit the item's type, judged over the whole map without the header, as
 *   the form judges them and as the service will (`misfits`); while the workspace's types are not
 *   read, nothing can be judged, and the text is held back with those words.
 * Until the text can be sent the draft keeps its last properties and the text stays as typed,
 * through a change of view too. A change made in the form after that replaces the text: the YAML
 * view then opens on the draft's properties again.
 *
 * Limit: what the form holds back (input that does not fit the type) is the form's own and is not
 * carried into the YAML view; that view shows what the draft holds.
 */
import { Tabs } from '@base-ui/react/tabs';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useId, useState } from 'react';
import { z } from 'zod';
import { listTypesOptions } from '../../api/generated/@tanstack/react-query.gen';
import { keyed } from '../../lib/keys';
import { YamlEditor, YamlFindings } from '../../lib/YamlEditor';
import { readYaml, writeYaml, type YamlRead } from '../../lib/yaml';
import { editSession } from '../drafts/saving';
import type { DraftSession } from '../drafts/sessions';
import {
  APP_HEADER_KEY,
  misfits,
  PropertiesForm,
  withoutHeader,
  withoutHeaderField,
} from './PropertiesForm';

type PropertyMap = Record<string, unknown>;

const zObject = z.record(z.string(), z.unknown());
const TYPES_NOT_READ =
  'The types of this workspace are not read yet, so these properties cannot be checked. They are held back until they can be.';

/** The YAML as typed, and what became of it. */
export interface TypedYaml {
  text: string;
  read: YamlRead;
  /** Why the text, though readable, is not put into the draft; empty when it is. */
  refused: string[];
  /** The property map the draft held once this was typed: while it still does, the text stands. */
  draft: PropertyMap;
}

/** A mapping of names to values: what an item's properties are. */
function isMapping(value: unknown): value is PropertyMap {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/**
 * The complete property map for what the YAML holds: its properties in its order, and the
 * application's header back at the place it had in `all`.
 */
export function withHeaderBack(all: PropertyMap, typed: PropertyMap): PropertyMap {
  const entries = Object.entries(typed);
  if (APP_HEADER_KEY in all) {
    const place = Math.min(Object.keys(all).indexOf(APP_HEADER_KEY), entries.length);
    entries.splice(place, 0, [APP_HEADER_KEY, all[APP_HEADER_KEY]]);
  }
  return Object.fromEntries(entries);
}

/**
 * Why readable YAML is not put into the draft; empty when it can be. `schema` is the schema of
 * the item's type without the header's field, null when the workspace defines none for it, and
 * undefined while the types are not read.
 */
export function refusals(schema: PropertyMap | null | undefined, value: unknown): string[] {
  if (!isMapping(value)) {
    return ['The properties are a mapping of names to values, such as “owner: Ada”.'];
  }
  if (APP_HEADER_KEY in value) {
    return [`“${APP_HEADER_KEY}” is kept by okf-jawn and is not set here. Remove it.`];
  }
  if (schema === undefined) return [TYPES_NOT_READ];
  return schema === null ? [] : misfits(schema, value).map((misfit) => misfit.words);
}

interface PropertiesYamlProps {
  session: DraftSession;
  typeName: string;
  /** The text as typed, while it still stands; null while the view shows the draft's properties. */
  typed: TypedYaml | null;
  onTyped: (typed: TypedYaml) => void;
}

function PropertiesYaml({ session, typeName, typed, onTyped }: PropertiesYamlProps) {
  const client = useQueryClient();
  const findingsId = useId();
  const types = useQuery(listTypesOptions({ body: { workspace_id: session.workspaceId } }));
  const definition = types.data?.items.find((type) => type.name === typeName);
  const shape = zObject.safeParse(definition?.properties_schema);
  const schema =
    types.data === undefined ? undefined : shape.success ? withoutHeaderField(shape.data) : null;
  const count = Object.keys(session.properties).length;
  return (
    <details className="properties" open>
      <summary>Properties ({count})</summary>
      <YamlEditor
        label="Properties as YAML"
        initialText={typed?.text ?? writeYaml(withoutHeader(session.properties))}
        describedBy={findingsId}
        onChange={(text) => {
          const read = readYaml(text);
          const refused = read.ok ? refusals(schema, read.value) : [];
          if (!read.ok || refused.length > 0 || !isMapping(read.value)) {
            onTyped({ text, read, refused, draft: session.properties });
            return;
          }
          const wanted = withHeaderBack(session.properties, read.value);
          onTyped({ text, read, refused, draft: wanted });
          editSession(client, session.workspaceId, session.itemId, { properties: wanted });
        }}
      />
      <div id={findingsId}>
        <YamlFindings id={`${findingsId}-read`} read={typed?.read ?? null} />
        {typed !== null && typed.refused.length > 0 && (
          <div role="alert" className="failure-notice">
            <p>
              <strong>
                These properties are held back, and are not saved until this is put right.
              </strong>
            </p>
            <ul>
              {keyed(typed.refused, (words) => words).map(({ key, value }) => (
                <li key={key}>{value}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
      <p className="meta">
        Every property is here except the one okf-jawn keeps for itself. A name no type knows is
        kept as it is typed.
      </p>
    </details>
  );
}

export interface PropertiesViewsProps {
  session: DraftSession;
  /** The item's user type name: the type whose schema describes its properties. */
  typeName: string;
}

/** The properties of the draft being edited, as a form or as YAML. */
export function PropertiesViews({ session, typeName }: PropertiesViewsProps) {
  const [view, setView] = useState<'form' | 'yaml'>('form');
  const [typed, setTyped] = useState<TypedYaml | null>(null);
  // The text stands while the draft's properties are what they were when it was typed; a change
  // made in the form since then replaces it.
  const standing = typed !== null && typed.draft === session.properties ? typed : null;
  const heldBack = standing !== null && (!standing.read.ok || standing.refused.length > 0);
  return (
    <Tabs.Root
      value={view}
      onValueChange={(next) => {
        if (next === 'form' || next === 'yaml') setView(next);
      }}
    >
      <Tabs.List aria-label="How the properties are shown" className="tab-list">
        <Tabs.Tab value="form" className="tab">
          Form
        </Tabs.Tab>
        <Tabs.Tab value="yaml" className="tab">
          YAML
        </Tabs.Tab>
      </Tabs.List>
      <Tabs.Panel value="form">
        {heldBack && (
          <p role="note" className="notice">
            The YAML view holds text that is not saved. Below are the properties the draft holds. A
            change made here replaces that text.
          </p>
        )}
        <PropertiesForm session={session} typeName={typeName} />
      </Tabs.Panel>
      <Tabs.Panel value="yaml">
        <PropertiesYaml session={session} typeName={typeName} typed={standing} onTyped={setTyped} />
      </Tabs.Panel>
    </Tabs.Root>
  );
}
