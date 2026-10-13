/**
 * An item's properties as YAML, beside the form: the same draft's property map, where a change in
 * one view shows in the other, a key no type knows is kept, the application's header is never
 * typed and never lost, and nothing that cannot be read or does not fit the type is sent.
 */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { refusals, withHeaderBack } from '../../src/features/documents/PropertiesYaml';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { YAML_PAUSE_MS } from '../../src/lib/yaml';
import { type Handler, mockApi, ok } from './support/api';
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
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS);
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
    const owner = screen.getByLabelText(/^Owner\*?$/);
    expect(owner).toHaveProperty('value', 'Grace');
    fireEvent.change(owner, { target: { value: 'Hedy' } });
    fireEvent.click(tab('YAML'));
    await pass(0);
    expect(textIn(textBox(YAML))).toBe(storedYaml.replace('owner: Ada', 'owner: Hedy'));
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS);
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
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(saved(api)).toEqual({ ...stored, attendees: 5 });
  });

  it('is there for an item whose type the workspace does not define', async () => {
    const api = service({}, []);
    const box = await openYaml();
    replaceText(box, `${storedYaml}added: true\n`);
    await pass(YAML_PAUSE_MS + AUTOSAVE_DELAY_MS);
    expect(saved(api)).toEqual({ ...stored, added: true });
    expect(Object.keys(saved(api) as object).at(-1)).toBe('added');
  });
});

describe('when the properties YAML is read', () => {
  const broken = 'owner: Grace\nattendees: [\n';
  const cannotBeRead = () =>
    screen.queryAllByRole('alert').some((alert) => alert.textContent?.includes('cannot be read'));

  it('reads the text a pause after the last change, not at every change', async () => {
    service();
    const box = await openYaml();
    replaceText(box, broken);
    await pass(YAML_PAUSE_MS - 1);
    expect(cannotBeRead()).toBe(false);
    // Another change starts the pause again.
    replaceText(box, `${broken}#\n`);
    await pass(YAML_PAUSE_MS - 1);
    expect(cannotBeRead()).toBe(false);
    await pass(1);
    expect(cannotBeRead()).toBe(true);
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
