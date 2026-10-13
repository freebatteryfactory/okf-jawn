/**
 * An item's properties as YAML, beside the form: the same draft's property map, where a change in
 * one view shows in the other, a key no type knows is kept, the application's header is never
 * typed and never lost, and nothing that cannot be read or does not fit the type is sent.
 */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { asLeft, refusals, withHeaderBack } from '../../src/features/documents/PropertiesYaml';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { YAML_PAUSE_MS } from '../../src/lib/yaml';
import { setYamlEngine, TOO_COMPLEX_WORDS, YAML_LIMIT_MS } from '../../src/lib/yaml-reading';
import { type Handler, mockApi, ok } from './support/api';
import { CANCELLED, handed, TAKEN } from './support/drags';
import { findTextBox, replaceText, textBox, textIn } from './support/editor';
import {
  draftOf,
  explorerHandlers,
  itemDocument,
  NOTE,
  noteSummary,
  otherSummary,
  renderAt,
  WS,
  writableWorkspace,
} from './support/workspace';
import { heldEngine } from './support/yaml-engine';

const SAVE = '/api/items/save-draft';
const TYPES = '/api/types/list-types';
const ITEM = '/api/items/get-item';
const LATER = '2026-01-02T08:15:00.000Z';
const YAML = 'Properties as YAML';

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const meetingNote = {
  name: 'meeting-note',
  schema_version: 1,
  properties_schema: {
    type: 'object',
    required: ['owner'],
    properties: {
      owner: { type: 'string', title: 'Owner', minLength: 1 },
      attendees: { type: 'integer', title: 'Attendees', minimum: 1 },
    },
  },
  ui_schema: {},
};

/** What the note holds: the header in the middle, and keys its type does not name. */
const stored = {
  type: 'meeting-note',
  owner: 'Ada',
  okf_jawn: { id: NOTE },
  attendees: 4,
  'x-okf-review': { by: 'grace', scores: [1, 2, 3] },
  status_note: 'yes',
};
const storedYaml = [
  'type: meeting-note',
  'owner: Ada',
  'attendees: 4',
  'x-okf-review:',
  '  by: grace',
  '  scores:',
  '    - 1',
  '    - 2',
  '    - 3',
  'status_note: "yes"',
  '',
].join('\n');

function service(overrides: Record<string, Handler> = {}, types: unknown[] = [meetingNote]) {
  return mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      [ITEM]: (body) =>
        ok(
          body.item_id === NOTE
            ? { ...itemDocument(noteSummary), properties: stored }
            : itemDocument(otherSummary),
        ),
      [TYPES]: () => ok({ items: types }),
      [SAVE]: (body) => ok(draftOf(String(body.item_id), String(body.base_revision), LATER)),
      ...overrides,
    }),
  );
}

const pass = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
/** The pause after typing, and the turn in which the reading then answers. */
const READ_MS = YAML_PAUSE_MS + 1;
const tab = (name: string) => screen.getByRole('tab', { name });
/** The properties of the last `save_draft`. */
const saved = (api: ReturnType<typeof mockApi>) => api.callsTo(SAVE).at(-1)?.properties;

/** Open the note's editor and its properties' YAML view; the clock is then the test's. */
async function openYaml() {
  renderAt(`/w/${WS}/i/${NOTE}`);
  const edit = await screen.findByRole('button', { name: 'Edit' });
  await screen.findByText('Text of Meeting notes');
  fireEvent.click(edit);
  await waitFor(() => tab('YAML'));
  fireEvent.click(tab('YAML'));
  const box = await findTextBox(YAML);
  vi.useFakeTimers();
  return box;
}

describe('what the YAML may put into the draft', () => {
  it('puts the header back at its place, and keeps the order the text has', () => {
    expect(withHeaderBack(stored, { owner: 'Bo', type: 'meeting-note', extra: 1 })).toEqual({
      owner: 'Bo',
      type: 'meeting-note',
      okf_jawn: { id: NOTE },
      extra: 1,
    });
    expect(Object.keys(withHeaderBack(stored, { owner: 'Bo', type: 'x', extra: 1 }))).toEqual([
      'owner',
      'type',
      'okf_jawn',
      'extra',
    ]);
    // Fewer properties than the header's old place: it goes last. No header: none is made.
    expect(Object.keys(withHeaderBack(stored, { owner: 'Bo' }))).toEqual(['owner', 'okf_jawn']);
    expect(withHeaderBack({ a: 1 }, { b: 2 })).toEqual({ b: 2 });
  });

  it('refuses what is not a mapping, the header, and what does not fit the type', () => {
    const schema = meetingNote.properties_schema;
    expect(refusals(schema, { owner: 'Ada', anything: [1] })).toEqual([]);
    expect(refusals(schema, ['a'])).toEqual([
      'The properties are a mapping of names to values, such as “owner: Ada”.',
    ]);
    expect(refusals(schema, null).at(0)).toMatch(/mapping of names to values/);
    expect(refusals(schema, { owner: 'Ada', okf_jawn: {} })).toEqual([
      '“okf_jawn” is kept by okf-jawn and is not set here. Remove it.',
    ]);
    expect(refusals(schema, { attendees: 0 })).toEqual([
      '“owner” is required, and this item does not have it.',
      '“attendees” must be >= 1.',
    ]);
    // Without a type there is nothing to fit; while the types are not read, nothing is judged.
    expect(refusals(null, { attendees: 0 })).toEqual([]);
    expect(refusals(undefined, { owner: 'Ada' }).at(0)).toMatch(
      /types of this workspace are not read yet/,
    );
  });
});

describe('the properties as YAML', () => {
  it('shows every property but the header, as the draft holds them', async () => {
    service();
    const box = await openYaml();
    expect(textIn(box)).toBe(storedYaml);
  });

  it('saves a change with the draft: unknown keys as typed, the header as it was, in its place', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(
      box,
      storedYaml.replace('owner: Ada', 'owner: Grace').replace('by: grace', 'by: ada'),
    );
    await pass(READ_MS + AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({
      ...stored,
      owner: 'Grace',
      'x-okf-review': { by: 'ada', scores: [1, 2, 3] },
    });
    expect(Object.keys(saved(api) as object)).toEqual(Object.keys(stored));
    expect(api.violations).toEqual([]);
  });

  it('shows a change made here in the form, and one made in the form here', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(box, storedYaml.replace('owner: Ada', 'owner: Grace'));
    fireEvent.click(tab('Form'));
    // The change of view started the reading; until it answers there is no form to edit.
    expect(screen.queryByLabelText(/^Owner\*?$/)).toBeNull();
    expect(screen.getByText('Reading the text of the YAML view…')).toBeTruthy();
    await pass(0);
    const owner = screen.getByLabelText(/^Owner\*?$/);
    expect(owner).toHaveProperty('value', 'Grace');
    fireEvent.change(owner, { target: { value: 'Hedy' } });
    fireEvent.click(tab('YAML'));
    await pass(0);
    expect(textIn(textBox(YAML))).toBe(storedYaml.replace('owner: Ada', 'owner: Hedy'));
    await pass(READ_MS + AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, owner: 'Hedy' });
  });

  it('never sends YAML that cannot be read: its problem is at its line, and the text stays', async () => {
    const api = service();
    const box = await openYaml();
    const broken = 'owner: Grace\nattendees: [\n';
    replaceText(box, broken);
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('This YAML cannot be read. Nothing of it is sent.');
    expect(alert.textContent).toMatch(/Line 3, column 1: /);
    const described = box.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(described)?.contains(alert)).toBe(true);
    // The form shows the draft's properties, which are as they were; the text is kept as typed.
    fireEvent.click(tab('Form'));
    expect(screen.getByLabelText(/^Owner\*?$/)).toHaveProperty('value', 'Ada');
    fireEvent.click(tab('YAML'));
    await pass(0);
    expect(textIn(textBox(YAML))).toBe(broken);
  });

  it('holds back what does not fit the type, a list, and the header, and says why', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(box, storedYaml.replace('owner: Ada\n', ''));
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(screen.getByRole('alert').textContent).toContain(
      '“owner” is required, and this item does not have it.',
    );
    replaceText(box, '- a\n- b\n');
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(screen.getByRole('alert').textContent).toMatch(/mapping of names to values/);
    replaceText(box, `${storedYaml}okf_jawn:\n  id: forged\n`);
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(screen.getByRole('alert').textContent).toContain('“okf_jawn” is kept by okf-jawn');
    expect(api.callsTo(SAVE)).toEqual([]);
    // Fitting again, it is saved.
    replaceText(box, storedYaml.replace('attendees: 4', 'attendees: 5'));
    await pass(READ_MS + AUTOSAVE_DELAY_MS);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(saved(api)).toEqual({ ...stored, attendees: 5 });
  });

  it('is there for an item whose type the workspace does not define', async () => {
    const api = service({}, []);
    const box = await openYaml();
    replaceText(box, `${storedYaml}added: true\n`);
    await pass(READ_MS + AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, added: true });
    expect(Object.keys(saved(api) as object).at(-1)).toBe('added');
  });
});

describe('when the properties YAML is read', () => {
  const broken = 'owner: Grace\nattendees: [\n';
  const cannotBeRead = () =>
    screen.queryAllByRole('alert').some((alert) => alert.textContent?.includes('cannot be read'));

  it('reads the text a pause after the last change, not at every change', async () => {
    const held = heldEngine();
    setYamlEngine(held.engine);
    service();
    const box = await openYaml();
    replaceText(box, broken);
    await pass(YAML_PAUSE_MS - 1);
    expect(held.readings).toHaveLength(0);
    // Another change starts the pause again.
    replaceText(box, `${broken}#\n`);
    await pass(YAML_PAUSE_MS - 1);
    expect(held.readings).toHaveLength(0);
    await pass(1);
    expect(held.readings.map((reading) => reading.text)).toEqual([`${broken}#\n`]);
    expect(cannotBeRead()).toBe(false);
    act(() => {
      held.readings.at(0)?.began();
      held.readings.at(0)?.done();
    });
    expect(cannotBeRead()).toBe(true);
  });

  it('ends the reading of a text that was changed meanwhile, and never shows its answer', async () => {
    const held = heldEngine();
    setYamlEngine(held.engine);
    const api = service();
    const box = await openYaml();
    replaceText(box, broken);
    await pass(YAML_PAUSE_MS);
    const [older] = held.readings;
    act(() => older?.began());
    expect(older?.ended).toBe(false);
    // The person types on while the older text is still being read.
    replaceText(box, storedYaml.replace('owner: Ada', 'owner: Grace'));
    expect(older?.ended).toBe(true);
    await pass(YAML_PAUSE_MS);
    expect(held.readings).toHaveLength(2);
    // The older reading answers all the same: nothing of it is shown.
    act(() => older?.done());
    expect(cannotBeRead()).toBe(false);
    act(() => {
      held.readings.at(1)?.began();
      held.readings.at(1)?.done();
    });
    expect(cannotBeRead()).toBe(false);
    await pass(AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, owner: 'Grace' });
  });

  it('holds back a text whose reading takes longer than the limit, and says that it is too complex', async () => {
    const held = heldEngine();
    setYamlEngine(held.engine);
    const api = service();
    const box = await openYaml();
    const typed = `deep: [${'a: '.repeat(100)}]\n`;
    replaceText(box, typed);
    await pass(YAML_PAUSE_MS);
    act(() => held.readings.at(0)?.began());
    await pass(YAML_LIMIT_MS - 1);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(held.readings.at(0)?.ended).toBe(false);
    await pass(1);
    expect(screen.getByRole('alert').textContent).toContain(TOO_COMPLEX_WORDS);
    expect(held.readings.at(0)?.ended).toBe(true);
    // Nothing of it is saved, and the text is still in the box.
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    expect(textIn(box)).toBe(typed);
  });

  it('reads the text at once when keyboard focus leaves the text box', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(box, storedYaml.replace('owner: Ada', 'owner: Grace'));
    fireEvent.blur(box);
    // No pause passed: only the save's own wait.
    await pass(AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, owner: 'Grace' });
  });

  it('reads the text that still waits when the editor is left, so it is saved with the draft', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(box, storedYaml.replace('owner: Ada', 'owner: Grace'));
    // Left before the pause has passed: back to the committed text.
    fireEvent.click(screen.getByRole('button', { name: 'Read the committed text' }));
    await pass(AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, owner: 'Grace' });
  });

  it('saves nothing of a whole number a number cannot hold, and says so', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(box, `${storedYaml}ticket: 12345678901234567890\naccount: 9007199254740993\n`);
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    const alert = screen.getByRole('alert').textContent ?? '';
    expect(alert).toContain('Line 11, column 9: This whole number cannot be sent exactly');
    expect(alert).toContain('Line 12, column 10: This whole number cannot be sent exactly');
  });

  it('saves nothing of a text with a directive behind a byte order mark', async () => {
    const api = service();
    const box = await openYaml();
    replaceText(box, `﻿%YAML 1.1\n---\n${storedYaml}started: 2026-10-12\nratio: 1:30\n`);
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    expect(screen.getByRole('alert').textContent).toContain(
      'A directive line (starting with %) is not accepted here.',
    );
  });
});

describe('what the form holds back, in the YAML view', () => {
  it('puts held-back input in its place among the properties as the person left them', () => {
    const held = {
      form: { owner: 'Ada', attendees: 0 },
      entered: ['attendees'],
      opened: [],
      takenOut: [],
    };
    expect(asLeft(stored, null)).toBe(stored);
    expect(asLeft(stored, held)).toEqual({ ...stored, attendees: 0 });
    expect(Object.keys(asLeft(stored, held))).toEqual(Object.keys(stored));
  });

  it('shows it as the person left it, says why it is held back, and takes the repair made there', async () => {
    const api = service();
    await openYaml();
    fireEvent.click(tab('Form'));
    await pass(0);
    // Not a number the type allows: the form holds it back.
    fireEvent.change(screen.getByLabelText(/^Attendees/), { target: { value: '0' } });
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    const line = screen.getByText(/What you changed in the properties is held back/);
    expect(line.textContent).toContain(
      'It can be repaired in the YAML view, which shows the properties as you left them.',
    );

    fireEvent.click(tab('YAML'));
    await pass(0);
    // As the person left it, not as the draft holds it; and the reason is listed.
    const box = textBox(YAML);
    expect(textIn(box)).toBe(storedYaml.replace('attendees: 4', 'attendees: 0'));
    expect(screen.getByRole('alert').textContent).toContain('“attendees” must be >= 1.');
    expect(api.callsTo(SAVE)).toEqual([]);

    replaceText(box, storedYaml.replace('attendees: 4', 'attendees: 9'));
    await pass(READ_MS + AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, attendees: 9 });
    expect(screen.queryByRole('alert')).toBeNull();
    // Nothing is held back any more, in either view.
    fireEvent.click(tab('Form'));
    await pass(0);
    expect(screen.getByLabelText(/^Attendees/)).toHaveProperty('value', '9');
    expect(screen.queryByText(/What you changed in the properties is held back/)).toBeNull();
  });

  it('opens on the draft’s properties when nothing is held back', async () => {
    service();
    const box = await openYaml();
    expect(textIn(box)).toBe(storedYaml);
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

describe('a YAML reading under way when the draft is discarded', () => {
  /** The draft holds a change of its text, so there is something to discard; the YAML reads on. */
  const changeTheText = () => replaceText(textBox('Text of Meeting notes'), 'Changed text');
  const discard = async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard the draft' }));
    await pass(0);
  };

  it('brings nothing back when it answers after the draft is gone: no save, and a new edit starts from the committed text', async () => {
    const held = heldEngine();
    setYamlEngine(held.engine);
    const api = service();
    const box = await openYaml();
    changeTheText();
    replaceText(box, storedYaml.replace('owner: Ada', 'owner: Grace'));
    fireEvent.blur(box);
    expect(held.readings).toHaveLength(1);
    act(() => held.readings.at(0)?.began());
    await discard();
    // The answer arrives anyway, as one from a worker already on its way would.
    act(() => held.readings.at(0)?.done());
    await pass(AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    expect(screen.queryByRole('textbox', { name: YAML })).toBeNull();
    // A new draft starts from what is committed, not from the discarded text.
    await pass(0);
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    await pass(0);
    fireEvent.click(tab('YAML'));
    await pass(0);
    expect(textIn(textBox(YAML))).toBe(storedYaml);
  });

  it('brings nothing back when the text still waited for its pause: it is read at the discard and answers to nothing', async () => {
    const api = service();
    const box = await openYaml();
    changeTheText();
    replaceText(box, storedYaml.replace('owner: Ada', 'owner: Grace'));
    await discard();
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS * 2);
    expect(api.callsTo(SAVE)).toEqual([]);
    await pass(0);
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    await pass(0);
    fireEvent.click(tab('YAML'));
    await pass(0);
    expect(textIn(textBox(YAML))).toBe(storedYaml);
  });
});

describe('a drag onto the properties of a draft', () => {
  it('of files is cancelled before the YAML text box or a field of the form is handed it, and one of text is not', async () => {
    service();
    const box = await openYaml();
    expect(handed(box)).toEqual({ text: TAKEN, files: CANCELLED });
    fireEvent.click(tab('Form'));
    await pass(0);
    const fields = [screen.getByLabelText(/^Owner\*?$/), screen.getByLabelText(/^Attendees/)];
    for (const each of fields) {
      expect(handed(each)).toEqual({ text: TAKEN, files: CANCELLED });
    }
  });
});
