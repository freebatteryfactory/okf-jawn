/**
 * An item's properties while its draft is edited: a form built from the schema of its type, part
 * of the draft, that never sends invalid input and never touches a property it does not hold.
 */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mergeProperties, namedProperties } from '../../src/features/documents/PropertiesForm';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { type Handler, mockApi, ok } from './support/api';
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
      [TYPES]: () => ok({ items: [meetingNote] }),
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

  it('reads the properties a schema names; none from a schema that is not an object’s', () => {
    expect(namedProperties(meetingNote.properties_schema)).toEqual(['owner', 'attendees', 'place']);
    expect(namedProperties({ type: 'string' })).toEqual([]);
    expect(namedProperties({ properties: ['not', 'a', 'map'] })).toEqual([]);
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
    expect(document.querySelector('.save-state')?.textContent).toBe('No changes to save.');
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
