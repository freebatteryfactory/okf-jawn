/**
 * What the properties form costs: the properties are judged against the item's type when they
 * change, and not when the text beside them does.
 *
 * The validator is the real one, counted: every call that judges data against a schema
 * (`validateFormData`, `isValid`), whoever makes it (this application, or the form library for its
 * own drawing), goes through the one validator the form module makes.
 */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AUTOSAVE_DELAY_MS, resetDraftSessions } from '../../src/features/drafts/saving';
import { mockApi, ok } from './support/api';
import { textBox, typeInto } from './support/editor';
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

const judged = vi.hoisted(() => ({ times: 0 }));

vi.mock('@rjsf/validator-ajv8', async (importOriginal) => {
  const real = await importOriginal<typeof import('@rjsf/validator-ajv8')>();
  /** The same validator, with its two ways of judging data counted. */
  const counting = <V extends object>(validator: V): V =>
    new Proxy(validator, {
      get(target, name, receiver) {
        const value: unknown = Reflect.get(target, name, receiver);
        if ((name !== 'validateFormData' && name !== 'isValid') || typeof value !== 'function') {
          return value;
        }
        return (...given: unknown[]) => {
          judged.times += 1;
          return Reflect.apply(value, target, given);
        };
      },
    });
  return {
    ...real,
    customizeValidator: (...options: Parameters<typeof real.customizeValidator>) =>
      counting(real.customizeValidator(...options)),
  };
});

const SAVE = '/api/items/save-draft';

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  judged.times = 0;
});

const pass = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const owner = () => screen.getByLabelText(/^Owner\*?$/);

/** The note's editor, with a form whose type has a rule that depends on the data. */
async function editing() {
  const api = mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      '/api/items/get-item': (body) =>
        ok(
          body.item_id === NOTE
            ? {
                ...itemDocument(noteSummary),
                properties: { type: 'meeting-note', owner: 'Ada', okf_jawn: { id: NOTE } },
              }
            : itemDocument(otherSummary),
        ),
      '/api/types/list-types': () =>
        ok({
          items: [
            {
              name: 'meeting-note',
              schema_version: 1,
              ui_schema: { owner: { 'ui:help': 'Who called the meeting' } },
              properties_schema: {
                type: 'object',
                required: ['owner'],
                properties: {
                  owner: { type: 'string', title: 'Owner', minLength: 1 },
                  attendees: { type: 'integer', title: 'Attendees', minimum: 1 },
                },
                dependencies: { attendees: { properties: { room: { type: 'string' } } } },
              },
            },
          ],
        }),
      [SAVE]: (body) => ok(draftOf(String(body.item_id), String(body.base_revision))),
    }),
  );
  renderAt(`/w/${WS}/i/${NOTE}`);
  fireEvent.click(await screen.findByRole('button', { name: 'Edit' }));
  await waitFor(() => owner());
  vi.useFakeTimers();
  await pass(50);
  return api;
}

describe('the cost of the properties form', () => {
  it('judges the properties when they change, and not at a keystroke in the text', async () => {
    const api = await editing();
    // Opening the form judged its data; the counter is alive.
    expect(judged.times).toBeGreaterThan(0);
    const opened = judged.times;

    // Ten keystrokes in the text, each drawn, and their save: the properties are not judged.
    const box = textBox('Text of Meeting notes');
    for (const letter of ' ten more.') typeInto(box, letter);
    await pass(AUTOSAVE_DELAY_MS);
    await waitForSaves(api, 1);
    await pass(50);
    expect(judged.times).toBe(opened);

    // A change of a property is judged.
    fireEvent.change(owner(), { target: { value: 'Grace' } });
    await pass(0);
    const first = judged.times - opened;
    expect(first).toBeGreaterThan(0);
    // The next like change costs the same: one judging per change, however many came before.
    fireEvent.change(owner(), { target: { value: 'Grace H' } });
    await pass(0);
    expect(judged.times - opened - first).toBe(first);
    await pass(AUTOSAVE_DELAY_MS);
    await waitForSaves(api, 2);
    await pass(50);

    // And the text again, after the properties changed and were saved: still nothing.
    const settled = judged.times;
    for (const letter of ' again') typeInto(box, letter);
    await pass(AUTOSAVE_DELAY_MS);
    await waitForSaves(api, 3);
    await pass(50);
    expect(judged.times).toBe(settled);
    expect(api.callsTo(SAVE).at(-1)?.properties).toMatchObject({ owner: 'Grace H' });
  });
});

async function waitForSaves(api: ReturnType<typeof mockApi>, count: number) {
  for (let turn = 0; turn < 200 && api.callsTo(SAVE).length < count; turn += 1) await pass(0);
  expect(api.callsTo(SAVE)).toHaveLength(count);
}
