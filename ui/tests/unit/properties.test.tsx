/**
 * An item's properties while its draft is edited: a form built from the schema of its type, part
 * of the draft, that never sends invalid input and never touches a property it does not hold.
 */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  drawnProperties,
  mergeProperties,
  misfits,
  takenOutBy,
  withoutHeaderField,
} from '../../src/features/documents/PropertiesForm';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { draftsPage, fail, type Handler, mockApi, ok } from './support/api';
import { textBox, typeInto } from './support/editor';
import {
  draftOf,
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  otherSummary,
  renderAt,
  summary,
  WS,
  writableWorkspace,
} from './support/workspace';

const SAVE = '/api/items/save-draft';
const TYPES = '/api/types/list-types';
const ITEM = '/api/items/get-item';
const LATER = '2026-01-02T08:15:00.000Z';

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** The type of the note: three properties, one required, one bounded. */
const meetingNote = {
  name: 'meeting-note',
  schema_version: 1,
  properties_schema: {
    type: 'object',
    required: ['owner'],
    properties: {
      owner: { type: 'string', title: 'Owner', minLength: 1 },
      attendees: { type: 'integer', title: 'Attendees', minimum: 1 },
      place: {
        type: 'object',
        title: 'Place',
        properties: { room: { type: 'string', title: 'Room' } },
      },
    },
  },
  ui_schema: {},
};

/** Another type of the workspace, with a property of its own. */
const plan = {
  name: 'plan',
  schema_version: 1,
  properties_schema: {
    type: 'object',
    properties: { deadline: { type: 'string', title: 'Deadline' } },
  },
  ui_schema: {},
};

/** What the note holds: properties its type names, and others it does not. */
const stored = {
  type: 'meeting-note',
  owner: 'Ada',
  attendees: 4,
  place: { room: 'Blue', floor: 2 },
  'x-okf-review': { by: 'grace', scores: [1, 2, 3] },
  aliases: ['standup', 'weekly'],
  okf_jawn: { id: NOTE },
};

function typed(overrides: Record<string, Handler> = {}) {
  return mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      [ITEM]: (body) =>
        ok(
          body.item_id === NOTE
            ? { ...itemDocument(noteSummary), properties: stored }
            : itemDocument(otherSummary),
        ),
      // Another type comes first: the form is the one of the item's own type.
      [TYPES]: () => ok({ items: [plan, meetingNote] }),
      [SAVE]: (body) => ok(draftOf(String(body.item_id), String(body.base_revision), LATER)),
      ...overrides,
    }),
  );
}

const pass = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

async function eventually(check: () => void) {
  for (let turn = 0; turn < 200; turn += 1) {
    try {
      check();
      return;
    } catch {
      await pass(0);
    }
  }
  check();
}

/** Open the note and press "Edit" once it has been read. */
async function pressEdit() {
  renderAt(`/w/${WS}/i/${NOTE}`);
  const edit = await screen.findByRole('button', { name: 'Edit' });
  await screen.findByText('Text of Meeting notes');
  fireEvent.click(edit);
}

/** A field of the form by its label; a required one carries a star after it. */
const field = (label: string) => screen.getByLabelText(new RegExp(`^${label}\\*?$`));

/** Open the note's editor with its form on screen; the clock is then the test's. */
async function editNote() {
  await pressEdit();
  await waitFor(() => field('Owner'));
  vi.useFakeTimers();
}

const set = (label: string, value: string) => fireEvent.change(field(label), { target: { value } });

/**
 * The conditional rules of a JSON Schema, by keyword. They are built from entries because an
 * object written with a `then` key reads as a promise to the linter; a schema is never awaited.
 */
const conditional = (rules: { when?: object; rule?: object; otherwise?: object }) =>
  Object.fromEntries(
    [
      ['if', rules.when],
      ['then', rules.rule],
      ['else', rules.otherwise],
    ].filter(([, rule]) => rule !== undefined),
  );

/** Wait, on the test's clock, for a field a change brought in. */
const waitForField = (label: string) => eventually(() => expect(field(label)).toBeDefined());

describe('merging what the form holds back into the property map', () => {
  it('keeps every other property exactly, in its place', () => {
    const all = { a: 1, x: { deep: [1, { y: null }] }, b: 'two', z: false };
    const merged = mergeProperties(all, ['a', 'b', 'c'], { a: 5, b: 'two', c: 'new' });
    expect(merged).toEqual({ a: 5, x: { deep: [1, { y: null }] }, b: 'two', z: false, c: 'new' });
    expect(Object.keys(merged)).toEqual(['a', 'x', 'b', 'z', 'c']);
    expect(merged.x).toBe(all.x);
  });

  it('drops a held property the form emptied, and no other', () => {
    expect(mergeProperties({ a: 1, x: 2 }, ['a'], {})).toEqual({ x: 2 });
    expect(mergeProperties({ a: 1, x: 2 }, ['a'], { a: undefined })).toEqual({ x: 2 });
  });

  it('never takes a property the form does not hold from the form', () => {
    expect(mergeProperties({ a: 1, x: 2 }, ['a'], { a: 1, x: 'forged', q: 'added' })).toEqual({
      a: 1,
      x: 2,
    });
  });

  it('puts a property that left the map back where it was when the form opened', () => {
    const order = ['a', 'x', 'b', 'z'];
    const keys = (all: Record<string, unknown>, held: string[], form: Record<string, unknown>) =>
      Object.keys(mergeProperties(all, held, form, order));
    expect(keys({ x: 2, b: 3, z: 4 }, ['a'], { a: 1 })).toEqual(['a', 'x', 'b', 'z']);
    expect(keys({ a: 1, x: 2, z: 4 }, ['b'], { b: 3 })).toEqual(['a', 'x', 'b', 'z']);
    expect(keys({ a: 1, x: 2, b: 3 }, ['z'], { z: 4 })).toEqual(['a', 'x', 'b', 'z']);
    // One that was never in the map goes to its end, and stays there.
    expect(keys({ a: 1, x: 2, b: 3, z: 4 }, ['n'], { n: 5 })).toEqual(['a', 'x', 'b', 'z', 'n']);
    expect(keys({ a: 1, n: 5, z: 4 }, ['x'], { x: 2 })).toEqual(['a', 'n', 'x', 'z']);
  });
});

describe('what the form is given of a type’s schema', () => {
  /** Rules that reach beyond the properties named at the top. */
  const ruled = {
    type: 'object',
    properties: { kind: { type: 'string' } },
    allOf: [{ properties: { venue: { type: 'string' } } }],
    ...conditional({
      when: { properties: { kind: { const: 'remote' } }, required: ['kind'] },
      rule: { properties: { link: { type: 'string' } } },
    }),
    dependencies: { kind: { properties: { floor: { type: 'integer' } } } },
  };

  it('reads the properties the form draws: those named at the top, and those a rule brings in', () => {
    expect(drawnProperties(meetingNote.properties_schema, {})).toEqual([
      'owner',
      'attendees',
      'place',
    ]);
    expect(drawnProperties({ type: 'string' }, {})).toEqual([]);
    expect(drawnProperties(ruled, {}).sort()).toEqual(['kind', 'venue']);
    expect(drawnProperties(ruled, { kind: 'local' }).sort()).toEqual(['floor', 'kind', 'venue']);
    expect(drawnProperties(ruled, { kind: 'remote' }).sort()).toEqual([
      'floor',
      'kind',
      'link',
      'venue',
    ]);
  });

  it('takes the application’s header out of the properties a schema names, wherever it names them', () => {
    const schema = {
      type: 'object',
      required: ['okf_jawn', 'owner'],
      properties: { owner: { type: 'string' }, okf_jawn: { type: 'object' } },
      allOf: [{ properties: { okf_jawn: {}, venue: { type: 'string' } } }],
      oneOf: [{ properties: { okf_jawn: {} } }, { anyOf: [{ properties: { okf_jawn: {} } }] }],
      ...conditional({
        rule: { properties: { okf_jawn: {} } },
        otherwise: { properties: { okf_jawn: {} } },
      }),
      dependencies: { owner: { properties: { okf_jawn: {}, when: {} } }, venue: ['owner'] },
      $defs: { part: { properties: { okf_jawn: {} } } },
    };
    const before = JSON.stringify(schema);
    const given = withoutHeaderField(schema);
    expect(JSON.stringify(given)).not.toMatch(/"okf_jawn":/);
    // Nothing else is taken, and a rule that demands the header still does.
    expect(given.required).toEqual(['okf_jawn', 'owner']);
    expect(given.dependencies.venue).toEqual(['owner']);
    expect(Object.keys(given.properties)).toEqual(['owner']);
    expect(given.allOf.at(0)?.properties).toEqual({ venue: { type: 'string' } });
    expect(JSON.stringify(schema)).toBe(before);
    expect(drawnProperties(given, {}).sort()).toEqual(['owner', 'venue']);
  });

  it('says in plain words what does not fit, and which property it is about', () => {
    const strict = {
      type: 'object',
      required: ['owner', 'reviewed'],
      additionalProperties: false,
      properties: { owner: { type: 'string' }, reviewed: { type: 'boolean' } },
    };
    expect(misfits(strict, { owner: 'Ada', reviewed: true })).toEqual([]);
    expect(misfits(strict, { owner: 'Ada', extra: 1 })).toEqual([
      { property: 'reviewed', words: '“reviewed” is required, and this item does not have it.' },
      { property: 'extra', words: '“extra” is not a property this type allows.' },
    ]);
    expect(misfits(strict, { owner: 'Ada', reviewed: 'yes' })).toEqual([
      { property: 'reviewed', words: '“reviewed” must be boolean.' },
    ]);
    const together = { type: 'object', minProperties: 2 };
    expect(misfits(together, { owner: 'Ada' })).toEqual([
      {
        property: '',
        words:
          'The properties together do not follow a rule of this type (it says: must NOT have fewer than 2 properties).',
      },
    ]);
  });
});

describe('the properties form', () => {
  it('is built from the schema of the item’s type, with the other properties beside it', async () => {
    const api = typed();
    await editNote();
    expect(field('Owner')).toHaveProperty('value', 'Ada');
    expect(field('Attendees')).toHaveProperty('value', '4');
    expect(field('Room')).toHaveProperty('value', 'Blue');
    const properties = screen.getByText('Properties (7)').closest('details');
    if (properties === null) throw new Error('the properties are not in their disclosure');
    expect(properties.textContent).toContain('Other properties, kept exactly as they are:');
    // The ones the type does not name are shown, not editable.
    for (const key of ['type', 'x-okf-review', 'aliases', 'okf_jawn']) {
      expect(within(properties).getByText(new RegExp(`^${key}`))).toBeDefined();
    }
    expect(
      within(properties).queryByRole('textbox', { name: /x-okf-review|aliases|type/ }),
    ).toBeNull();
    expect(api.callsTo(TYPES)).toEqual([{ workspace_id: WS }]);
    expect(api.violations).toEqual([]);
  });

  it('has no button to submit it: a change is saved with the draft', async () => {
    typed();
    await editNote();
    const form = document.querySelector('.properties-form');
    if (!(form instanceof HTMLElement)) throw new Error('the form is not on the page');
    expect(within(form).queryAllByRole('button')).toEqual([]);
    expect(form.querySelector('[type="submit"]')).toBeNull();
  });

  it('saves a change with the draft, and every other property unchanged', async () => {
    const api = typed();
    await editNote();
    set('Owner', 'Grace');
    expect(document.querySelector('.save-state')?.textContent).toBe('Unsaved changes.');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    const sent = api.callsTo(SAVE).at(0);
    expect(sent?.properties).toEqual({ ...stored, owner: 'Grace' });
    expect(Object.keys(Object(sent?.properties))).toEqual(Object.keys(stored));
    // The text goes with it, as it is.
    expect(sent?.body).toBe('# Meeting notes');
    expect(sent?.base_revision).toBe(HEAD);
    expect(api.violations).toEqual([]);
  });

  it('keeps a key its schema does not describe inside a property it holds', async () => {
    const api = typed();
    await editNote();
    set('Room', 'Green');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(Object(api.callsTo(SAVE).at(0)?.properties).place).toEqual({ room: 'Green', floor: 2 });
  });

  it('never sends invalid input, and shows the error at its field', async () => {
    const api = typed();
    await editNote();
    set('Attendees', '0');
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    const attendees = field('Attendees');
    // What was typed stays in the field, and the error is described at it.
    expect(attendees).toHaveProperty('value', '0');
    const described = (attendees.getAttribute('aria-describedby') ?? '').split(' ');
    const errors = described.map((id) => document.getElementById(id)?.textContent ?? '').join(' ');
    expect(errors).toMatch(/must be >= 1/);
    // Nothing is called saved, or unchanged, while input is held back.
    expect(document.querySelector('.save-state')?.textContent).toBe(
      'Not saved: what you changed in the properties is held back.',
    );
    // A valid change elsewhere is held back with it: the map is sent whole or not at all.
    set('Owner', 'Grace');
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    set('Attendees', '6');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)?.properties).toEqual({
      ...stored,
      owner: 'Grace',
      attendees: 6,
    });
  });

  it('says at its field what does not fit in the stored properties, before anything is typed', async () => {
    // The item lacks the required owner, and holds fewer attendees than its type allows.
    const api = typed({
      [ITEM]: () =>
        ok({
          ...itemDocument(noteSummary),
          properties: { type: 'meeting-note', attendees: 0, okf_jawn: { id: NOTE } },
        }),
    });
    await editNote();
    const errorsAt = (label: string) =>
      (field(label).getAttribute('aria-describedby') ?? '')
        .split(' ')
        .map((id) => document.getElementById(id)?.textContent ?? '')
        .join(' ');
    await eventually(() => expect(errorsAt('Attendees')).toMatch(/must be >= 1/));
    expect(errorsAt('Owner')).toMatch(/required/);
    // Saying so is not an edit: nothing is held back, marked or sent.
    expect(document.querySelector('.form-said')?.textContent).toBe('');
    expect(document.querySelector('.save-state')?.textContent).toBe('No changes to save.');
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    await pass(AUTOSAVE_DELAY_MS * 3);
    expect(api.callsTo(SAVE)).toHaveLength(0);
  });

  // The service judges the whole map without the application's header; so does the form.
  describe('a type whose rules reach beyond the properties it names', () => {
    const has = { type: 'meeting-note', owner: 'Ada', reviewed: true, okf_jawn: { id: NOTE } };
    const owner = { type: 'string', title: 'Owner' };
    const service = (properties_schema: Record<string, unknown>, properties: object = has) =>
      typed({
        [TYPES]: () => ok({ items: [{ ...meetingNote, properties_schema }] }),
        [ITEM]: () => ok({ ...itemDocument(noteSummary), properties }),
      });
    /** What the form says above its fields. */
    const saidAbove = () => document.querySelector('.form-said')?.textContent ?? '';
    const HELD_BACK =
      'What you changed in the properties is held back, and is not saved until the properties fit the type. It can be repaired in the YAML view, which shows the properties as you left them.';
    const sent = (api: ReturnType<typeof typed>, at: number) =>
      Object(api.callsTo(SAVE).at(at)?.properties);

    it('saves a valid change when a required property has no field and the item has it', async () => {
      const api = service({
        type: 'object',
        required: ['owner', 'reviewed'],
        properties: { owner },
      });
      await editNote();
      expect(saidAbove()).toBe('');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api, 0)).toEqual({ ...has, owner: 'Grace' });
      expect(saidAbove()).toBe('');
    });

    it('says above the form what no field can say, and that valid input is held back by it', async () => {
      const lacks = { type: 'meeting-note', owner: 'Ada', okf_jawn: { id: NOTE } };
      const api = service(
        { type: 'object', required: ['owner', 'reviewed'], properties: { owner } },
        lacks,
      );
      await editNote();
      // Said from the start, and nothing is held back yet: nothing was entered.

      expect(saidAbove()).toContain('These properties do not fit the type meeting-note:');
      expect(saidAbove()).toContain('“reviewed” is required, and this item does not have it.');
      expect(saidAbove()).not.toContain(HELD_BACK);
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(saidAbove()).toContain(HELD_BACK);
      expect(field('Owner')).toHaveProperty('value', 'Grace');
      expect(document.querySelector('.save-state')?.textContent).toBe(
        'Not saved: what you changed in the properties is held back.',
      );
      // Put back as it was, nothing is held back any more.
      set('Owner', 'Ada');
      expect(saidAbove()).not.toContain(HELD_BACK);
      expect(document.querySelector('.save-state')?.textContent).toBe('No changes to save.');
    });

    it('takes what is typed in a field a rule under allOf draws', async () => {
      const api = service({
        type: 'object',
        properties: { owner },
        allOf: [{ properties: { venue: { type: 'string', title: 'Venue' } } }],
      });
      await editNote();
      set('Venue', 'Blue room');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api, 0)).toEqual({ ...has, venue: 'Blue room' });
      expect(Object.keys(sent(api, 0))).toEqual([...Object.keys(has), 'venue']);
    });

    it('takes what is typed in a field an if/then draws, once the rule it brings is met', async () => {
      const api = service({
        type: 'object',
        properties: { owner, kind: { type: 'string', title: 'Kind' } },
        ...conditional({
          when: { properties: { kind: { const: 'remote' } }, required: ['kind'] },
          rule: { properties: { link: { type: 'string', title: 'Link' } }, required: ['link'] },
        }),
      });
      await editNote();
      expect(screen.queryByLabelText(/^Link\*?$/)).toBeNull();
      set('Kind', 'remote');
      // The rule now asks for a link: nothing is sent without it, and its field says so.
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(saidAbove()).toBe(HELD_BACK);
      const link = field('Link');
      const described = (link.getAttribute('aria-describedby') ?? '').split(' ');
      expect(
        described.map((id) => document.getElementById(id)?.textContent ?? '').join(' '),
      ).toMatch(/required/);
      set('Link', 'https://meet.example/1');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api, 0)).toEqual({ ...has, kind: 'remote', link: 'https://meet.example/1' });
      expect(saidAbove()).toBe('');
    });

    it('takes what is typed in a field a dependency draws', async () => {
      const api = service({
        type: 'object',
        properties: { owner, card: { type: 'string', title: 'Card' } },
        dependencies: { card: { properties: { billing: { type: 'string', title: 'Billing' } } } },
      });
      await editNote();
      set('Card', '4000');
      await waitForField('Billing');
      set('Billing', 'Main street');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE).length).toBeGreaterThan(0));
      expect(sent(api, -1)).toEqual({ ...has, card: '4000', billing: 'Main street' });
    });

    it('never makes the application’s header a field, whatever the schema names', async () => {
      const header = {
        type: 'object',
        title: 'Header',
        properties: { id: { type: 'string', title: 'Header id' } },
      };
      const api = service({
        type: 'object',
        properties: { owner, okf_jawn: header },
        allOf: [{ properties: { okf_jawn: header } }],
      });
      await editNote();
      expect(screen.queryByLabelText(/Header id/)).toBeNull();
      expect(document.querySelector('.properties-form')?.textContent).not.toContain('Header');
      // It is listed with the others, kept as it is, and goes back exactly.
      const properties = screen.getByText('Properties (4)').closest('details');
      expect(properties?.textContent).toContain('okf_jawn');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api, 0)).toEqual({ ...has, owner: 'Grace' });
      expect(sent(api, 0).okf_jawn).toEqual({ id: NOTE });
    });

    it('says a schema that demands the header can be met by no item, and sends nothing', async () => {
      const api = service({ type: 'object', required: ['okf_jawn'], properties: { owner } });
      await editNote();
      expect(saidAbove()).toContain('“okf_jawn” is required, and this item does not have it.');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(saidAbove()).toContain(HELD_BACK);
    });

    it('judges what a save would carry, not what the form library filled in for itself', async () => {
      // The type requires a property whose value it fixes. The library shows that value in its
      // field; the item does not hold it, and the form writes nothing the person did not enter.
      const api = service({
        type: 'object',
        required: ['format'],
        properties: { owner, format: { type: 'string', title: 'Format', const: 'minutes' } },
      });
      await editNote();
      expect(saidAbove()).toContain('“format” is required, and this item does not have it.');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(saidAbove()).toContain(HELD_BACK);
    });

    it('lets a type that allows no other property save an item that holds only the header beside its own', async () => {
      const api = service(
        { type: 'object', additionalProperties: false, properties: { owner } },
        { owner: 'Ada', okf_jawn: { id: NOTE } },
      );
      await editNote();
      expect(saidAbove()).toBe('');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api, 0)).toEqual({ owner: 'Grace', okf_jawn: { id: NOTE } });
    });

    it('names each property a strict type does not allow, as the service would refuse it', async () => {
      const api = service({ type: 'object', additionalProperties: false, properties: { owner } });
      await editNote();
      expect(saidAbove()).toContain('“type” is not a property this type allows.');
      expect(saidAbove()).toContain('“reviewed” is not a property this type allows.');
      expect(saidAbove()).not.toContain('okf_jawn');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(saidAbove()).toContain(HELD_BACK);
      // Said once. The form library validates at a change: it does not then say the same misfit a
      // second time, under the form and in the validator's words.
      expect(document.querySelector('.properties-form')?.textContent).not.toMatch(/must NOT/);
      expect(
        document.querySelector('.properties')?.textContent?.match(/not a property this type/g),
      ).toHaveLength(2);
    });

    it('keeps every key in its place through each kind of edit', async () => {
      const api = service(
        {
          type: 'object',
          properties: {
            owner,
            attendees: { type: 'integer', title: 'Attendees' },
            venue: { type: 'string', title: 'Venue' },
          },
        },
        stored,
      );
      await editNote();
      const order = Object.keys(stored);
      const saved = async (count: number) => {
        await pass(AUTOSAVE_DELAY_MS);
        await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(count));
        return sent(api, count - 1);
      };
      // A value changed stays where it is.
      set('Attendees', '9');
      expect(Object.keys(await saved(1))).toEqual(order);
      // A property emptied leaves the map; every other one keeps its place.
      set('Owner', '');
      const emptied = await saved(2);
      expect(Object.keys(emptied)).toEqual(order.filter((key) => key !== 'owner'));
      // Entered again, it goes back where it was, not to the end.
      set('Owner', 'Grace');
      const again = await saved(3);
      expect(Object.keys(again)).toEqual(order);
      // One the item never had is added at the end.
      set('Venue', 'Blue room');
      const added = await saved(4);
      expect(Object.keys(added)).toEqual([...order, 'venue']);
      expect(added).toEqual({ ...stored, attendees: 9, owner: 'Grace', venue: 'Blue room' });
      // What the form does not hold is the same throughout.
      expect(added['x-okf-review']).toEqual(stored['x-okf-review']);
    });
  });

  // A type may offer a choice: the library draws a selector and the fields of the alternative
  // chosen, and takes the properties of the one before out of its data when another is chosen.
  describe('a type whose schema offers a choice (oneOf)', () => {
    const meeting = {
      type: 'meeting-note',
      owner: 'Ada',
      room: 'Blue',
      floor: 2,
      'x-okf-review': { by: 'grace' },
      okf_jawn: { id: NOTE },
    };
    const choosing = {
      type: 'object',
      properties: { owner: { type: 'string', title: 'Owner' } },
      oneOf: [
        {
          title: 'In a room',
          properties: {
            room: { type: 'string', title: 'Room' },
            floor: { type: 'integer', title: 'Floor' },
          },
          required: ['room'],
        },
        {
          title: 'Remote',
          properties: { link: { type: 'string', title: 'Link', minLength: 8 } },
          required: ['link'],
        },
      ],
    };
    const service = () =>
      typed({
        [TYPES]: () => ok({ items: [{ ...meetingNote, properties_schema: choosing }] }),
        [ITEM]: () => ok({ ...itemDocument(noteSummary), properties: meeting }),
      });
    const saidAbove = () => document.querySelector('.form-said')?.textContent ?? '';
    const TAKEN_OUT = 'What you chose takes “room” and “floor” out of this item’s properties.';
    const choice = () => screen.getByRole('combobox', { name: 'Which of these applies' });
    const choose = (title: string) => {
      const option = within(choice()).getByRole('option', { name: title });
      fireEvent.change(choice(), { target: { value: option.getAttribute('value') } });
    };
    const sent = (api: ReturnType<typeof typed>) => Object(api.callsTo(SAVE).at(-1)?.properties);

    it('draws the choice with a name and the fields of the alternative the item fits, and lists none of them as other properties', async () => {
      service();
      await editNote();
      expect(
        within(choice()).getByRole<HTMLOptionElement>('option', { name: 'In a room' }).selected,
      ).toBe(true);
      expect(field('Room')).toHaveProperty('value', 'Blue');
      expect(field('Floor')).toHaveProperty('value', '2');
      expect(screen.queryByLabelText(/^Link\*?$/)).toBeNull();
      // The alternative's properties have fields: they are not "kept exactly as they are".
      const others = document.querySelector('.properties')?.textContent?.split('Other properties');
      expect(others).toHaveLength(2);
      expect(others?.at(1)).toContain('x-okf-review');
      expect(others?.at(1)).not.toMatch(/room|floor|link/);
      expect(saidAbove()).toBe('');
    });

    it('takes what is typed in a field of the chosen alternative into the draft', async () => {
      const api = service();
      await editNote();
      set('Room', 'Green');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api)).toEqual({ ...meeting, room: 'Green' });
      expect(Object.keys(sent(api))).toEqual(Object.keys(meeting));
      expect(saidAbove()).toBe('');
    });

    it('says which properties another choice takes out, and saves without them once the new one fits', async () => {
      const api = service();
      await editNote();
      choose('Remote');
      await waitForField('Link');
      // Said at once, by name: the fields are gone, and so will the properties be.
      expect(saidAbove()).toContain(TAKEN_OUT);
      expect(screen.queryByLabelText(/^Room\*?$/)).toBeNull();
      // The new alternative lacks its required link: nothing is saved yet, and the item says so.
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(saidAbove()).toContain(
        'What you changed in the properties is held back, and is not saved until the properties fit the type.',
      );
      expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
      set('Link', 'https://meet.example/weekly');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api)).toEqual({
        type: 'meeting-note',
        owner: 'Ada',
        'x-okf-review': { by: 'grace' },
        okf_jawn: { id: NOTE },
        link: 'https://meet.example/weekly',
      });
      // It is still said after the save: the two properties are out of the item now.
      expect(saidAbove()).toBe(TAKEN_OUT);
    });

    it('stops naming a property once it is entered again', async () => {
      const api = service();
      await editNote();
      choose('Remote');
      await waitForField('Link');
      expect(saidAbove()).toContain(TAKEN_OUT);
      // Back to the first alternative: its fields are there again, empty. What they held is gone.
      choose('In a room');
      await waitForField('Room');
      expect(field('Room')).toHaveProperty('value', '');
      expect(saidAbove()).toContain(TAKEN_OUT);
      set('Room', 'Green');
      expect(saidAbove()).toContain('What you chose takes “floor” out of this item’s properties.');
      expect(saidAbove()).not.toContain('“room”');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api)).toEqual({
        type: 'meeting-note',
        owner: 'Ada',
        room: 'Green',
        'x-okf-review': { by: 'grace' },
        okf_jawn: { id: NOTE },
      });
    });

    it('does not call a property the person emptied in its own field taken out by a choice', async () => {
      const api = service();
      await editNote();
      set('Floor', '');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api)).not.toHaveProperty('floor');
      expect(saidAbove()).toBe('');
    });

    describe('when the editor is left and opened again with the choice held back', () => {
      const selected = (title: string) =>
        within(choice()).getByRole<HTMLOptionElement>('option', { name: title }).selected;
      async function leaveAndReturn() {
        fireEvent.click(screen.getByRole('button', { name: 'Read the committed text' }));
        await eventually(() => expect(screen.queryByLabelText(/^Owner\*?$/)).toBeNull());
        fireEvent.click(screen.getByRole('button', { name: 'Edit your draft' }));
        await waitForField('Owner');
      }

      it('shows the choice again with what was typed in it, and still names what it took out', async () => {
        const api = service();
        await editNote();
        choose('Remote');
        await waitForField('Link');
        // Too short for the type: held back, with the choice.
        set('Link', 'short');
        await leaveAndReturn();
        expect(selected('Remote')).toBe(true);
        expect(field('Link')).toHaveProperty('value', 'short');
        expect(saidAbove()).toContain(TAKEN_OUT);
        expect(api.callsTo(SAVE)).toHaveLength(0);
      });

      // A limit: which alternative is chosen is the form library's own state. A form that opens
      // shows the alternative its data fits best, so a choice with nothing typed in it yet is not
      // shown again. What the choice took out is still out, and still named.
      it('shows the alternative the data fits when nothing was typed in the one chosen', async () => {
        service();
        await editNote();
        choose('Remote');
        await waitForField('Link');
        await leaveAndReturn();
        expect(selected('In a room')).toBe(true);
        expect(field('Room')).toHaveProperty('value', '');
        expect(saidAbove()).toContain(TAKEN_OUT);
      });
    });
  });

  // The same choice written as `anyOf`: the library draws it the same way, and the page treats it
  // the same way.
  describe('a type whose schema offers a choice (anyOf)', () => {
    const meeting = {
      type: 'meeting-note',
      owner: 'Ada',
      room: 'Blue',
      floor: 2,
      'x-okf-review': { by: 'grace' },
      okf_jawn: { id: NOTE },
    };
    const choosing = {
      type: 'object',
      properties: { owner: { type: 'string', title: 'Owner' } },
      anyOf: [
        {
          title: 'In a room',
          properties: {
            room: { type: 'string', title: 'Room' },
            floor: { type: 'integer', title: 'Floor' },
          },
          required: ['room'],
        },
        {
          title: 'Remote',
          properties: { link: { type: 'string', title: 'Link', minLength: 8 } },
          required: ['link'],
        },
      ],
    };
    const service = () =>
      typed({
        [TYPES]: () => ok({ items: [{ ...meetingNote, properties_schema: choosing }] }),
        [ITEM]: () => ok({ ...itemDocument(noteSummary), properties: meeting }),
      });
    const saidAbove = () => document.querySelector('.form-said')?.textContent ?? '';
    const choice = () => screen.getByRole('combobox', { name: 'Which of these applies' });
    const sent = (api: ReturnType<typeof typed>) => Object(api.callsTo(SAVE).at(-1)?.properties);

    it('draws the choice and the fields of the alternative the item fits, and lists none of them as other properties', async () => {
      service();
      await editNote();
      expect(
        within(choice()).getByRole<HTMLOptionElement>('option', { name: 'In a room' }).selected,
      ).toBe(true);
      expect(field('Room')).toHaveProperty('value', 'Blue');
      expect(field('Floor')).toHaveProperty('value', '2');
      const others = document.querySelector('.properties')?.textContent?.split('Other properties');
      expect(others).toHaveLength(2);
      expect(others?.at(1)).toContain('x-okf-review');
      expect(others?.at(1)).not.toMatch(/room|floor|link/);
    });

    it('takes what is typed in a field of the chosen alternative into the draft', async () => {
      const api = service();
      await editNote();
      set('Room', 'Green');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api)).toEqual({ ...meeting, room: 'Green' });
    });

    it('says which properties another choice takes out, and loses nothing without saying so', async () => {
      const api = service();
      await editNote();
      const option = within(choice()).getByRole('option', { name: 'Remote' });
      fireEvent.change(choice(), { target: { value: option.getAttribute('value') } });
      await waitForField('Link');
      expect(saidAbove()).toContain(
        'What you chose takes “room” and “floor” out of this item’s properties.',
      );
      // The new alternative lacks its required link: nothing is saved, and the item says so.
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
      set('Link', 'https://meet.example/weekly');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(sent(api)).toEqual({
        type: 'meeting-note',
        owner: 'Ada',
        'x-okf-review': { by: 'grace' },
        okf_jawn: { id: NOTE },
        link: 'https://meet.example/weekly',
      });
    });
  });

  describe('what a change takes out of the form’s data', () => {
    it('names what left at another control, and not what was emptied in its own field', () => {
      const before = { owner: 'Ada', room: 'Blue', floor: 2, place: { room: 'x' } };
      expect(takenOutBy('root__oneof_select', before, { owner: 'Ada' })).toEqual([
        'room',
        'floor',
        'place',
      ]);
      expect(takenOutBy('root_room', before, { owner: 'Ada', floor: 2, place: {} })).toEqual([]);
      // A field inside a property is that property's own.
      expect(
        takenOutBy('root_place_room', before, { owner: 'Ada', room: 'Blue', floor: 2 }),
      ).toEqual([]);
      // A property whose name begins like another's is not that other's field.
      expect(
        takenOutBy('root_roommate', before, { owner: 'Ada', floor: 2, place: { room: 'x' } }),
      ).toEqual(['room']);
      // What changed and stayed, or was never there, was not taken out.
      expect(
        takenOutBy('root__oneof_select', before, { ...before, owner: 'Grace', link: 'x' }),
      ).toEqual([]);
    });
  });

  // Opening the editor is not an edit, whatever the type's schema would fill in.
  describe('a type whose schema names defaults', () => {
    const withDefaults = {
      ...meetingNote,
      properties_schema: {
        type: 'object',
        required: ['owner'],
        properties: {
          owner: { type: 'string', title: 'Owner', minLength: 1 },
          priority: { type: 'string', title: 'Priority', default: 'normal' },
          tags: { type: 'array', title: 'Tags', items: { type: 'string' } },
          place: {
            type: 'object',
            title: 'Place',
            properties: { room: { type: 'string', title: 'Room', default: 'Hall' } },
          },
        },
      },
    };
    const held = { type: 'meeting-note', owner: 'Ada', okf_jawn: { id: NOTE } };
    const service = () =>
      typed({
        [TYPES]: () => ok({ items: [withDefaults] }),
        [ITEM]: () => ok({ ...itemDocument(noteSummary), properties: held }),
      });

    it('pressing "Edit" sends nothing and marks nothing', async () => {
      const api = service();
      await editNote();
      // A property the item does not hold is shown empty, not as its default.
      expect(field('Priority')).toHaveProperty('value', '');
      expect(field('Room')).toHaveProperty('value', '');
      expect(document.querySelector('.save-state')?.textContent).toBe('No changes to save.');
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(document.querySelector('.save-state')?.textContent).toBe('No changes to save.');
      expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
      expect(screen.getByRole('region', { name: 'Drafts (0)' })).toBeDefined();
    });

    it('a value the schema fixes is not written by the form itself, with or without an edit', async () => {
      const fixed = {
        ...meetingNote,
        properties_schema: {
          type: 'object',
          properties: {
            owner: { type: 'string', title: 'Owner' },
            format: { type: 'string', title: 'Format', const: 'minutes' },
          },
        },
      };
      const api = typed({
        [TYPES]: () => ok({ items: [fixed] }),
        [ITEM]: () => ok({ ...itemDocument(noteSummary), properties: held }),
      });
      await editNote();
      await pass(AUTOSAVE_DELAY_MS * 3);
      expect(api.callsTo(SAVE)).toHaveLength(0);
      expect(document.querySelector('.save-state')?.textContent).toBe('No changes to save.');
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(api.callsTo(SAVE).at(0)?.properties).toEqual({ ...held, owner: 'Grace' });
    });

    it('a change of one property sends that change, and no default beside it', async () => {
      const api = service();
      await editNote();
      set('Owner', 'Grace');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(api.callsTo(SAVE).at(0)?.properties).toEqual({ ...held, owner: 'Grace' });
      // A default the person enters is theirs, and is sent.
      set('Priority', 'normal');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(2));
      expect(api.callsTo(SAVE).at(1)?.properties).toEqual({
        ...held,
        owner: 'Grace',
        priority: 'normal',
      });
    });
  });

  // A type's schema and UI schema are the service's data: the page never falls to them.
  describe('a type whose definition the form library cannot draw', () => {
    const undrawable = {
      ...meetingNote,
      ui_schema: { owner: { 'ui:widget': 'no-such-widget' } },
    };

    it('keeps the Explorer, shows the properties as they are, and saves them with the text', async () => {
      const api = typed({ [TYPES]: () => ok({ items: [undrawable] }) });
      await pressEdit();
      expect(await screen.findByText(/cannot be shown here: its definition asks/)).toBeDefined();
      // The page is still the Explorer, with the editor in it.
      expect(screen.getByRole('tree', { name: 'Folders' })).toBeDefined();
      const box = textBox('Text of Meeting notes');
      const properties = screen.getByText('Properties (7)').closest('details');
      expect(properties?.textContent).toContain('owner');
      expect(properties?.textContent).toContain('Ada');
      expect(properties?.textContent).toContain('x-okf-review');
      vi.useFakeTimers();
      typeInto(box, ' more');
      await pass(AUTOSAVE_DELAY_MS);
      await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
      expect(api.callsTo(SAVE).at(0)?.properties).toEqual(stored);
    });

    it('opens an item of that type that already has a draft', async () => {
      typed({
        [TYPES]: () => ok({ items: [undrawable] }),
        [ITEM]: () =>
          ok({
            ...itemDocument(noteSummary),
            properties: stored,
            draft: {
              body: '# Meeting notes\n\nMy draft',
              properties: stored,
              draft: draftOf(NOTE, HEAD, LATER),
            },
          }),
      });
      renderAt(`/w/${WS}/i/${NOTE}`);
      const box = await screen.findByRole('textbox', { name: 'Text of Meeting notes' });
      expect(box.textContent).toContain('My draft');
      expect(await screen.findByText(/cannot be shown here: its definition asks/)).toBeDefined();
      expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
    });
  });

  it('shows a type’s description and help as text: no image, no link, whatever its UI schema asks', async () => {
    const described = {
      ...meetingNote,
      properties_schema: {
        type: 'object',
        properties: {
          owner: {
            type: 'string',
            title: 'Owner',
            description:
              'see <img src=x onerror=alert(1)> [link](javascript:alert(3)) ![pix](https://elsewhere.example/p.png) [out](https://elsewhere.example/)',
          },
          place: {
            type: 'object',
            title: 'Place',
            properties: {
              room: {
                type: 'string',
                title: 'Room',
                description: '![in](https://elsewhere.example/r.png)',
              },
            },
          },
        },
      },
      ui_schema: {
        owner: {
          'ui:enableMarkdownInDescription': true,
          'ui:help': '[help](https://elsewhere.example/h)',
          'ui:options': { enableMarkdownInHelp: true },
        },
        place: { room: { 'ui:enableMarkdownInDescription': true } },
        'ui:globalOptions': { enableMarkdownInDescription: true, enableMarkdownInHelp: true },
      },
    };
    typed({ [TYPES]: () => ok({ items: [described] }) });
    await editNote();
    const form = document.querySelector('.properties-form');
    expect(form?.querySelectorAll('img, a, script')).toHaveLength(0);
    // The words are there, as the text they are.
    expect(form?.textContent).toContain('![pix](https://elsewhere.example/p.png)');
    expect(form?.textContent).toContain('[help](https://elsewhere.example/h)');
    expect(form?.textContent).toContain('![in](https://elsewhere.example/r.png)');
  });

  // A read that fails when it is asked again never takes away what is on screen.
  describe('a read asked again in the background that fails', () => {
    it.each([
      ['get_item', ITEM, 'Reading this item’s properties: The service is temporarily unavailable.'],
      ['list_types', TYPES, 'Reading this item’s type: The service is temporarily unavailable.'],
    ] as const)(
      '%s: the form stays, with what was typed and with focus, and the failure is said beside it',
      async (_read, path, words) => {
        let down: string | null = null;
        typed({
          [ITEM]: () =>
            down === ITEM
              ? fail(503, 'unavailable', 'down')
              : ok({ ...itemDocument(noteSummary), properties: stored }),
          [TYPES]: () =>
            down === TYPES ? fail(503, 'unavailable', 'down') : ok({ items: [meetingNote] }),
        });
        const { queryClient } = renderAt(`/w/${WS}/i/${NOTE}`);
        const edit = await screen.findByRole('button', { name: 'Edit' });
        await screen.findByText('Text of Meeting notes');
        fireEvent.click(edit);
        await waitFor(() => field('Owner'));
        // Typed and not valid yet: it is only in the form.
        set('Attendees', '0');
        act(() => field('Attendees').focus());
        down = path;
        await act(() => queryClient.invalidateQueries());
        expect(await screen.findByText(words)).toBeDefined();
        expect(field('Attendees')).toHaveProperty('value', '0');
        expect(document.activeElement).toBe(field('Attendees'));
        expect(screen.getByRole('textbox', { name: 'Text of Meeting notes' })).toBeDefined();
        // The notice has the retry; when it answers, the form is still the same one.
        down = null;
        const attendees = field('Attendees');
        fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
        await waitFor(() => expect(screen.queryByText(words)).toBeNull());
        expect(field('Attendees')).toBe(attendees);
        expect(attendees).toHaveProperty('value', '0');
      },
    );
  });

  it('does not send a required property emptied', async () => {
    const api = typed();
    await editNote();
    set('Owner', '');
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    expect(
      screen.getByText(/must have required property 'Owner'|is a required property/),
    ).toBeDefined();
  });

  it('saves the text with the whole map when the item’s type has no definition', async () => {
    const api = typed({ [TYPES]: () => ok({ items: [] }) });
    await pressEdit();
    expect(await screen.findByText(/This workspace has no definition of the type/)).toBeDefined();
    expect(screen.queryByRole('textbox', { name: /^Owner/ })).toBeNull();
    vi.useFakeTimers();
    typeInto(textBox('Text of Meeting notes'), ' more');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)?.properties).toEqual(stored);
  });

  it('reads "not available in this build" where types are not built, and keeps the properties', async () => {
    const { [TYPES]: _unbuilt, ...handlers } = explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      [ITEM]: () => ok({ ...itemDocument(noteSummary), properties: stored }),
    });
    mockApi(handlers);
    await pressEdit();
    expect(
      await screen.findByText('Reading this item’s type: Not available in this build.'),
    ).toBeDefined();
    expect(screen.getByText(/^owner/)).toBeDefined();
    expect(screen.getByRole('textbox', { name: 'Text of Meeting notes' })).toBeDefined();
  });
});

// What the form holds back is in no save. It is kept with the item's session, so it is there
// again when the form is; and while it is there the item is unsaved.
describe('input the form holds back', () => {
  const COMMIT = '/api/history/commit-items';
  const DRAFTS = '/api/items/list-drafts';
  const MADE = 'c'.repeat(40);
  const HELD_BACK =
    'What you changed in the properties is held back, and is not saved until the properties fit the type.';
  const saidAbove = () => document.querySelector('.form-said')?.textContent ?? '';
  const saveState = () => document.querySelector('.save-state')?.textContent ?? '';
  const panel = () => screen.getByRole('region', { name: /^Drafts/ });
  const row = () =>
    within(panel()).getByRole('button', { name: 'Meeting notes' }).closest('li')?.textContent ?? '';
  const box = () =>
    within(panel()).getByRole('checkbox', { name: 'Include Meeting notes in the Snapshot' });
  const leaving = () => {
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  };
  const errorsAt = (label: string) =>
    (field(label).getAttribute('aria-describedby') ?? '')
      .split(' ')
      .map((id) => document.getElementById(id)?.textContent ?? '')
      .join(' ');

  /** A strict type, and a note that holds a key it does not allow: every change is held back. */
  const strict = () =>
    typed({
      [TYPES]: () =>
        ok({
          items: [
            {
              ...meetingNote,
              properties_schema: {
                type: 'object',
                additionalProperties: false,
                properties: {
                  type: { type: 'string', title: 'Type' },
                  owner: { type: 'string', title: 'Owner' },
                },
              },
            },
          ],
        }),
      [ITEM]: (body) =>
        ok(
          body.item_id === NOTE
            ? {
                ...itemDocument(noteSummary),
                properties: {
                  type: 'meeting-note',
                  owner: 'Ada',
                  'x-okf-review': { by: 'grace' },
                  okf_jawn: { id: NOTE },
                },
              }
            : itemDocument(otherSummary),
        ),
    });

  it('is shown again after the committed text was read, and the item is unsaved meanwhile', async () => {
    const api = strict();
    await editNote();
    set('Owner', 'Grace');
    await pass(AUTOSAVE_DELAY_MS * 3);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    expect(saveState()).toBe('Not saved: what you changed in the properties is held back.');
    // The tab, the tree and the Drafts panel all say the item is unsaved.
    expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
    expect(
      within(screen.getByRole('tree', { name: 'Folders' })).getByRole('treeitem', {
        name: 'Meeting notes, type meeting-note, unsaved',
      }),
    ).toBeDefined();
    expect(panel().querySelector('h2')?.textContent).toBe('Drafts (1)');
    expect(row()).toContain('unsaved: what you changed in its properties is held back');

    fireEvent.click(screen.getByRole('button', { name: 'Read the committed text' }));
    await eventually(() =>
      expect(
        screen.getByText(
          'You changed this item’s properties, and that is held back and not saved. Below is the committed text.',
        ),
      ).toBeDefined(),
    );
    expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
    fireEvent.click(screen.getByRole('button', { name: 'Edit your draft' }));
    await waitForField('Owner');
    expect(field('Owner')).toHaveProperty('value', 'Grace');
    expect(saidAbove()).toContain(HELD_BACK);
    expect(saidAbove()).toContain('“x-okf-review” is not a property this type allows.');
    expect(api.callsTo(SAVE)).toHaveLength(0);
  });

  it('comes back with the error at its field after another item was opened, and is saved once it fits', async () => {
    const api = typed();
    await editNote();
    set('Attendees', '0');
    set('Owner', 'Grace');
    const tree = screen.getByRole('tree', { name: 'Folders' });
    fireEvent.click(within(tree).getByRole('treeitem', { name: /^Plan/ }));
    await eventually(() => expect(screen.queryByLabelText(/^Owner\*?$/)).toBeNull());
    fireEvent.click(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' }));
    await waitForField('Owner');
    expect(field('Owner')).toHaveProperty('value', 'Grace');
    expect(field('Attendees')).toHaveProperty('value', '0');
    expect(saidAbove()).toContain(HELD_BACK);
    // Why it is held back is said at the field again, though nothing was typed in this form.
    expect(errorsAt('Attendees')).toMatch(/must be >= 1/);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    set('Attendees', '6');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    // Everything entered before the editor was left goes with it, in its place.
    expect(api.callsTo(SAVE).at(0)?.properties).toEqual({
      ...stored,
      owner: 'Grace',
      attendees: 6,
    });
    expect(Object.keys(Object(api.callsTo(SAVE).at(0)?.properties))).toEqual(Object.keys(stored));
    await eventually(() => expect(saveState()).toMatch(/^Saved as a draft at/));
    expect(saidAbove()).not.toContain(HELD_BACK);
    expect(screen.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeDefined();
  });

  it('moves with the editor to another pane ("Edit here")', async () => {
    strict();
    await editNote();
    set('Owner', 'Grace');
    fireEvent.click(screen.getByRole('button', { name: 'Split right' }));
    const here = await eventuallyButton('Edit here');
    act(() => here.focus());
    fireEvent.click(here);
    await eventually(() => {
      const second = screen.getByRole('region', { name: 'Documents, pane 2 of 2, focused' });
      expect(within(second).getByLabelText(/^Owner\*?$/)).toHaveProperty('value', 'Grace');
    });
    expect(screen.getAllByLabelText(/^Owner\*?$/)).toHaveLength(1);
    expect(saidAbove()).toContain(HELD_BACK);
  });

  describe('the question before a draft is discarded', () => {
    const question = () => screen.getByRole('group', { name: 'Discard this draft?' });
    const HELD = 'what you changed in the properties that is not saved';

    it('names what is held back in the properties, among what it removes', async () => {
      strict();
      await editNote();
      set('Owner', 'Grace');
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      expect(question().textContent).toContain(
        `Your text here, and ${HELD}, will be removed. The committed text stays as it is.`,
      );
    });

    it('does not mention the properties when none of their input is held back', async () => {
      strict();
      await editNote();
      typeInto(textBox('Text of Meeting notes'), ' more');
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      expect(question().textContent).toContain(
        'Your text here will be removed. The committed text stays as it is.',
      );
      expect(question().textContent).not.toContain(HELD);
    });
  });

  it('asks before the page is left, until the input is put back', async () => {
    strict();
    await editNote();
    expect(leaving()).toBe(false);
    set('Owner', 'Grace');
    expect(leaving()).toBe(true);
    // Put back as it was: nothing is held back, nothing is unsaved, and the page leaves freely.
    set('Owner', 'Ada');
    expect(leaving()).toBe(false);
    expect(saveState()).toBe('No changes to save.');
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeDefined();
    expect(panel().querySelector('h2')?.textContent).toBe('Drafts (0)');
  });

  it('with nothing of the item saved, is not offered for a Snapshot', async () => {
    const api = strict();
    await editNote();
    set('Owner', 'Grace');
    expect(row()).toContain('so there is nothing to snapshot');
    expect(box()).toHaveProperty('checked', false);
    expect(box().getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(box());
    expect(box()).toHaveProperty('checked', false);
    fireEvent.change(within(panel()).getByRole('textbox', { name: 'Snapshot name' }), {
      target: { value: 'Tidy' },
    });
    fireEvent.click(within(panel()).getByRole('button', { name: 'Snapshot 0 drafts' }));
    await pass(AUTOSAVE_DELAY_MS * 3);
    expect(api.callsTo(COMMIT)).toHaveLength(0);
    expect(api.callsTo(SAVE)).toHaveLength(0);
  });

  it('is said before and after a Snapshot of the saved draft, and stays in the form', async () => {
    let taken = false;
    const draft = {
      body: '# Meeting notes\n\nMy draft text',
      properties: stored,
      draft: draftOf(NOTE, HEAD, LATER),
    };
    const api = typed({
      [ITEM]: (body) =>
        ok(
          body.item_id !== NOTE
            ? itemDocument(otherSummary)
            : taken
              ? { ...itemDocument(noteSummary), body: draft.body, properties: stored }
              : { ...itemDocument(noteSummary), properties: stored, draft },
        ),
      [DRAFTS]: () => ok(draftsPage(taken ? [] : [draft.draft])),
      '/api/workspaces/open-workspace': () =>
        ok({ ...writableWorkspace, head: taken ? MADE : HEAD }),
      [COMMIT]: () => {
        taken = true;
        return ok({ revision: MADE, receipt_id: NOTE, warnings: [] });
      },
    });
    renderAt(`/w/${WS}/i/${NOTE}`);
    await waitFor(() => field('Attendees'));
    await screen.findByRole('region', { name: 'Drafts (1)' });
    vi.useFakeTimers();
    set('Attendees', '0');
    // Before: the draft is ticked, and its row says what a Snapshot of it will not have.
    expect(box()).toHaveProperty('checked', true);
    expect(row()).toMatch(
      /saved .*; unsaved: what you changed in its properties is held back, and a Snapshot will not have it$/,
    );
    expect(saveState()).toMatch(
      /^Saved as a draft at .*, without what you changed in the properties: that is held back\.$/,
    );
    fireEvent.change(within(panel()).getByRole('textbox', { name: 'Snapshot name' }), {
      target: { value: 'Tidy' },
    });
    fireEvent.click(within(panel()).getByRole('button', { name: 'Snapshot 1 draft' }));
    await eventually(() => expect(api.callsTo(COMMIT)).toHaveLength(1));
    // After: said where the Snapshot is said, and the input is still in its field.
    await eventually(() =>
      expect(panel().querySelector('.snapshot-said')?.textContent).toBe(
        'Snapshot “Tidy” taken: 1 item. The workspace now shows revision cccccccccc, which the Snapshot made. ' +
          'What you changed in its properties is held back, and is not in the Snapshot.',
      ),
    );
    expect(field('Attendees')).toHaveProperty('value', '0');
    expect(saidAbove()).toContain(HELD_BACK);
    expect(screen.getByRole('tab', { name: 'Meeting notes (unsaved)' })).toBeDefined();
    expect(leaving()).toBe(true);
    // Nothing of the item is saved now: it is listed, and there is nothing to snapshot.
    await eventually(() => expect(row()).toContain('so there is nothing to snapshot'));
    expect(box()).toHaveProperty('checked', false);
    expect(api.callsTo(SAVE)).toHaveLength(0);
    // Once it fits, it is a new draft on the revision the Snapshot made.
    set('Attendees', '6');
    await pass(AUTOSAVE_DELAY_MS);
    await eventually(() => expect(api.callsTo(SAVE)).toHaveLength(1));
    expect(api.callsTo(SAVE).at(0)).toMatchObject({
      base_revision: MADE,
      body: draft.body,
      properties: { ...stored, attendees: 6 },
    });
    expect(api.violations).toEqual([]);
  });
});

async function eventuallyButton(name: string): Promise<HTMLElement> {
  await eventually(() => {
    screen.getByRole('button', { name });
  });
  return screen.getByRole('button', { name });
}

// A session whose item a read says is not a Note is refused: nothing of it is saved, and what
// the person typed is kept on screen, read-only. That is the text and what they entered in the
// properties, whether it went into the draft or was held back.
describe('the kept text of a refused session', () => {
  const NEWER = 'd'.repeat(40);
  // A View: no draft of it is taken. (A Source's draft is taken while its text is unchanged, so a
  // Source would not refuse a draft that changed only properties: UI M3.)
  const asView = summary(NOTE, 'notes/meeting.md', 'Meeting notes', 'meeting-note', 'view');
  const DRAFT_PROPERTIES = 'The properties of your draft, read only';
  const HELD = 'What you entered in the properties and was held back, read only';

  /** The note with its type's form; at the newer head the same item is a View. */
  function becomesView() {
    const held = { head: HEAD };
    const api = typed({
      '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: held.head }),
      [ITEM]: (body) =>
        ok(
          body.item_id === NOTE
            ? {
                ...itemDocument(JSON.stringify(body.at).includes(NEWER) ? asView : noteSummary),
                properties: stored,
              }
            : itemDocument(otherSummary),
        ),
    });
    return { api, held };
  }

  /** The workspace is refreshed to the head where the item is a View: the kept text is shown. */
  async function refuse(held: { head: string }) {
    held.head = NEWER;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await eventually(() =>
      expect(
        screen.getByText(`Refreshed: now showing revision ${NEWER.slice(0, 10)}.`),
      ).toBeDefined(),
    );
    await pass(500);
    await eventually(() => expect(screen.getByLabelText('Your text, read only')).toBeDefined());
    return screen.getByRole('region', { name: 'Draft of Meeting notes, not saved' });
  }

  /** The names and values a group lists, as "name: value". */
  const listed = (group: HTMLElement) =>
    [...group.querySelectorAll('.property')].map(
      (entry) =>
        `${entry.querySelector('dt')?.textContent}: ${entry.querySelector('dd')?.textContent}`,
    );

  it('shows the properties of the draft when the person changed one, and nothing as held back', async () => {
    const { api, held } = becomesView();
    await editNote();
    set('Owner', 'Grace');
    // Still waiting for its autosave when the read says the item is a View.
    await pass(100);
    const region = await refuse(held);
    const group = within(region).getByRole('group', { name: DRAFT_PROPERTIES });
    expect(listed(group)).toContain('owner: Grace');
    // The whole map of the draft, with what the form never drew.
    expect(listed(group)).toContain('attendees: 4');
    expect(listed(group).some((entry) => entry.startsWith('x-okf-review: '))).toBe(true);
    expect(within(region).queryByRole('group', { name: HELD })).toBeNull();
    // Read-only: the form's fields are gone, and nothing is sent.
    expect(screen.queryByLabelText(/^Owner/)).toBeNull();
    await pass(AUTOSAVE_DELAY_MS * 12);
    expect(api.callsTo(SAVE)).toHaveLength(0);
  }, 20_000);

  it('shows what was entered and held back as it was entered, an emptied field as empty, and the question names it', async () => {
    const { api, held } = becomesView();
    await editNote();
    // Neither fits the type: fewer attendees than one, and no owner. Both are held back.
    set('Attendees', '0');
    set('Owner', '');
    await pass(100);
    const region = await refuse(held);
    const group = within(region).getByRole('group', { name: HELD });
    expect(listed(group)).toEqual(['attendees: 0', 'owner: (empty)']);
    // The draft's own properties are the committed ones: nothing went into it.
    expect(within(region).queryByRole('group', { name: DRAFT_PROPERTIES })).toBeNull();
    fireEvent.click(within(region).getByRole('button', { name: 'Discard draft' }));
    expect(
      within(region).getByRole('group', { name: 'Discard this draft?' }).textContent,
    ).toContain(
      'Your text here, and what you changed in the properties that is not saved, will be removed. The committed text stays as it is.',
    );
    await pass(AUTOSAVE_DELAY_MS * 12);
    expect(api.callsTo(SAVE)).toHaveLength(0);
  }, 20_000);

  it('shows the text alone when nothing was entered in the properties', async () => {
    const { held } = becomesView();
    await editNote();
    typeInto(textBox('Text of Meeting notes'), ' and more');
    await pass(100);
    const region = await refuse(held);
    expect(screen.getByLabelText('Your text, read only')).toHaveProperty(
      'value',
      '# Meeting notes and more',
    );
    expect(within(region).queryByRole('group', { name: DRAFT_PROPERTIES })).toBeNull();
    expect(within(region).queryByRole('group', { name: HELD })).toBeNull();
  }, 20_000);
});
