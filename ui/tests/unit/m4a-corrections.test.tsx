/**
 * Corrections of a source (UI M4a, ruling A4), in the reader: whose text is shown, said as of the
 * version it is; Correct text, a text box beside the text shown, saved with `correct_digest` on the
 * head the workspace shows and the item's digest there, after which the workspace shows the
 * revision it made; Keep the supplied text (`adopts`, with the proposal's own text); typed
 * refusals in words; and who may do either.
 *
 * Every request and answer is held to the generated schemas (`support/api.ts`); the service is the
 * small one of `support/m4a-corrections.ts`, which keeps the real one's rules.
 */
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Extraction } from '../../src/api/generated/types.gen';
import { fail, type Json, type MockedApi, mockApi } from './support/api';
import { findTextBox, replaceText, textIn, typeInto } from './support/editor';
import {
  CONVERTED,
  CORRECTED_AT,
  correctionsService,
  ELSEWHERE,
  extractionOf,
  GET_PROPOSAL,
  ITEM,
  PARTIAL,
  PROPOSAL,
  READ,
  type ServiceOptions,
  SUPPLIED,
  SUPPLIED_MARKDOWN,
  sourceDocument,
  UUID,
} from './support/m4a-corrections';
import { CORRECT_DIGEST } from './support/reader';
import { DIGEST, HEAD, OLD, renderAt, SOURCE, WS } from './support/workspace';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });
// A test waits for several answers in turn, each up to that long.
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const short = (revision: string) => revision.slice(0, 10);

async function open(options: ServiceOptions = {}, url = `/w/${WS}/i/${SOURCE}`) {
  const service = correctionsService(options);
  const api = mockApi(service.handlers);
  const rendered = renderAt(url);
  const section = await screen.findByRole('region', { name: 'Corrections' });
  return { api, service, section, ...rendered };
}

/** The section's words once the item has been read. */
async function says(section: HTMLElement, words: string) {
  await waitFor(() => expect(section.textContent).toContain(words));
}

function button(section: HTMLElement, name: string | RegExp) {
  return within(section).getByRole('button', { name });
}

async function startCorrecting(section: HTMLElement) {
  fireEvent.click(await within(section).findByRole('button', { name: 'Correct text' }));
  return findTextBox('Your correction');
}

const bodies = (api: MockedApi) => api.callsTo(CORRECT_DIGEST);
const describedBy = (element: HTMLElement) =>
  (element.getAttribute('aria-describedby') ?? '')
    .split(' ')
    .map((id) => document.getElementById(id)?.textContent ?? '')
    .join(' ');

describe('whose text is shown', () => {
  it('says the converter’s text is shown, as of the version the workspace shows', async () => {
    const { api, section } = await open();
    await says(
      section,
      `As of revision ${short(HEAD)}, the version this workspace shows, the text shown is the converter’s text.`,
    );
    expect(api.violations).toEqual([]);
  });

  it('says a person’s correction is shown over the converter’s text', async () => {
    const { section } = await open({ extraction: extractionOf({ corrected: true }) });
    await says(
      section,
      'the text shown is a person’s correction, shown over the converter’s text.',
    );
  });

  it('says an agent supplied the text shown, who accepted it, and the pages it claims', async () => {
    const { api, section } = await open({
      extraction: extractionOf({
        outcome: PARTIAL,
        text_origin: 'supplied_by_agent',
        supplied: SUPPLIED,
      }),
    });
    await says(
      section,
      'the text shown is text an agent supplied and a person accepted, not the converter’s.',
    );
    await says(section, 'agent-7 supplied it, through desk-client; ada accepted it on');
    await says(section, 'The agent says it covers pages 3–4: its claim, not a location.');
    const link = within(section).getByRole('link', { name: 'The proposal it came from' });
    expect(link.getAttribute('href')).toBe(`/w/${WS}/proposals/${PROPOSAL}`);
    expect(api.violations).toEqual([]);
  });

  it('says who kept the supplied text over the converter’s', async () => {
    const { section } = await open({
      extraction: extractionOf({
        text_origin: 'supplied_by_agent',
        supplied: { ...SUPPLIED, kept_by: 'grace' },
      }),
    });
    await says(
      section,
      'the text shown is text an agent supplied, which grace kept over the converter’s text.',
    );
    expect(within(section).queryByRole('button', { name: 'Keep the supplied text' })).toBeNull();
  });

  it('says a later conversion is shown while the supplied text stays in the history', async () => {
    const { section } = await open({ extraction: extractionOf({ supplied: SUPPLIED }) });
    await says(section, 'the text shown is the converter’s text.');
    await says(
      section,
      'Text an agent supplied, which ada accepted on',
      // The rest of the sentence follows the date.
    );
    await says(section, 'is kept in the history: this conversion’s text is shown in its place.');
  });
});

describe('Correct text', () => {
  it('opens a text box beside the text shown, started from it, and saved on the head shown', async () => {
    const { api, section } = await open();
    const box = await startCorrecting(section);
    // Beside it, the text shown, read only.
    const shown = within(section).getByRole('textbox', { name: 'The text shown now' });
    expect(shown.getAttribute('aria-readonly')).toBe('true');
    expect(textIn(shown)).toBe(CONVERTED);
    expect(textIn(box)).toBe(CONVERTED);
    expect(document.activeElement).toBe(box);
    expect(describedBy(box)).toContain('It starts from the text shown: the converter’s text.');
    // Nothing to save until the text differs.
    const save = button(section, 'Save the correction');
    expect(save.getAttribute('aria-disabled')).toBe('true');
    expect(describedBy(save)).toContain('Your text is the text shown: nothing is changed yet.');
    fireEvent.click(save);
    expect(bodies(api)).toEqual([]);

    replaceText(box, '# Annual report\n\nRevenue grew by 21% in 2025.');
    await waitFor(() =>
      expect(describedBy(save)).toContain(
        'Your text differs from the text shown in 1 place: line 3.',
      ),
    );
    expect(save.getAttribute('aria-disabled')).toBe('false');
    fireEvent.click(save);

    const status = await within(section).findByText(
      `Saved: your correction is revision ${short(CORRECTED_AT)}. The workspace now shows it, and the text above is your correction.`,
    );
    expect(bodies(api)).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        base_revision: HEAD,
        digest: DIGEST,
        corrected_markdown: '# Annual report\n\nRevenue grew by 21% in 2025.',
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    // Focus is on what was done, not on the page body.
    expect(document.activeElement).toBe(status.closest('[tabindex="-1"]'));
    // The workspace shows the revision the correction made, and the reader reads it there.
    await waitFor(() =>
      expect(
        api
          .callsTo(READ)
          .some(
            (body) =>
              JSON.stringify(body.at) ===
              JSON.stringify({ kind: 'revision', revision: CORRECTED_AT }),
          ),
      ).toBe(true),
    );
    expect(await screen.findByText('Revenue grew by 21% in 2025.')).toBeTruthy();
    expect(
      await screen.findByText('A person’s correction is shown over the converted text.'),
    ).toBeTruthy();
    await says(
      section,
      'the text shown is a person’s correction, shown over the converter’s text.',
    );
    expect(within(section).queryByRole('textbox', { name: 'Your correction' })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('keeps the key while the same correction is tried again, and takes a new one after an answer', async () => {
    let answers = 0;
    const service = correctionsService();
    const real = service.handlers[CORRECT_DIGEST];
    const api = mockApi({
      ...service.handlers,
      [CORRECT_DIGEST]: (body: Json) => {
        answers += 1;
        if (answers === 1) return fail(503, 'unavailable', 'the store is busy');
        if (real === undefined) throw new Error('no correction handler');
        return real(body);
      },
    });
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const section = await screen.findByRole('region', { name: 'Corrections' });
    const box = await startCorrecting(section);
    typeInto(box, ' A first fix.');
    fireEvent.click(button(section, 'Save the correction'));
    const retry = await within(section).findByRole('button', { name: 'Try again' });
    expect(section.textContent).toContain(
      'Saving the correction: The service is temporarily unavailable.',
    );
    fireEvent.click(retry);
    await within(section).findByText(/^Saved: your correction is revision/);
    const [first, second] = bodies(api);
    expect(second?.idempotency_key).toBe(first?.idempotency_key);

    // Another correction, after the answer: a new request with a new key, on the new head.
    const again = await startCorrecting(section);
    typeInto(again, ' A second fix.');
    fireEvent.click(button(section, 'Save the correction'));
    await waitFor(() => expect(bodies(api)).toHaveLength(3));
    const third = bodies(api).at(2);
    expect(third?.idempotency_key).not.toBe(first?.idempotency_key);
    expect(third?.base_revision).toBe(CORRECTED_AT);
    expect(api.violations).toEqual([]);
  });

  it('says when the base moved, keeps the text, and saves again on the version shown then', async () => {
    const { api, service, section } = await open();
    const box = await startCorrecting(section);
    typeInto(box, ' Fixed.');
    // Someone else changed the workspace meanwhile.
    act(() => service.moveHead(ELSEWHERE));
    fireEvent.click(button(section, 'Save the correction'));
    await says(
      section,
      'Saving the correction: The workspace changed since the version your correction was made on.',
    );
    await says(section, `It now holds revision ${short(ELSEWHERE)}. Your text is kept here.`);
    const reload = within(section).getByRole('button', {
      name: `Show revision ${short(ELSEWHERE)} and keep my text`,
    });
    reload.focus();
    fireEvent.click(reload);
    await says(
      section,
      `As of revision ${short(ELSEWHERE)}, the version this workspace shows, the text shown is the converter’s text.`,
    );
    // The text box is the same one, with the person's text in it.
    expect(within(section).getByRole('textbox', { name: 'Your correction' })).toBe(box);
    expect(textIn(box)).toBe(`${CONVERTED} Fixed.`);
    await waitFor(() => expect(section.textContent).not.toContain('The workspace changed since'));
    // The notice left with focus in it: focus is on the button that saves again.
    expect(document.activeElement).toBe(button(section, 'Save the correction'));
    fireEvent.click(button(section, 'Save the correction'));
    await within(section).findByText(/^Saved: your correction is revision/);
    const [stale, saved] = bodies(api);
    expect(stale?.base_revision).toBe(HEAD);
    expect(saved?.base_revision).toBe(ELSEWHERE);
    expect(saved?.idempotency_key).not.toBe(stale?.idempotency_key);
    expect(api.violations).toEqual([]);
  });

  it('says a correction is not available in this build, and that nothing was done', async () => {
    const { api, section } = await open({
      correct: () =>
        fail(501, 'not_implemented', 'correct_digest is not implemented in this build'),
    });
    const box = await startCorrecting(section);
    typeInto(box, ' Fixed.');
    fireEvent.click(button(section, 'Save the correction'));
    await says(section, 'Saving the correction: Not available in this build.');
    await says(section, 'Nothing was done.');
    expect(within(section).queryByText(/^Saved/)).toBeNull();
    expect(textIn(box)).toBe(`${CONVERTED} Fixed.`);
    expect(api.violations).toEqual([]);
  });

  it('says there is nothing to correct when no conversion has a digest', async () => {
    const { api, section } = await open({
      extraction: {
        outcome: { status: 'unsupported' },
        text_origin: 'none',
        corrected: false,
        warnings: [],
      },
      body: '',
    });
    await says(
      section,
      'Correct text needs a conversion of this file to correct, and it has none: the converter has not made one.',
    );
    expect(within(section).queryByRole('button', { name: 'Correct text' })).toBeNull();
    expect(api.violations).toEqual([]);
  });
});

describe('Keep the supplied text', () => {
  const later: Extraction = extractionOf({ supplied: SUPPLIED });

  it('keeps the proposal’s own text over the conversion shown, with `adopts`', async () => {
    const { api, section } = await open({ extraction: later });
    const keep = await within(section).findByRole('button', { name: 'Keep the supplied text' });
    await waitFor(() => expect(keep.getAttribute('aria-disabled')).toBe('false'));
    expect(api.callsTo(GET_PROPOSAL)).toEqual([{ workspace_id: WS, proposal_id: PROPOSAL }]);
    fireEvent.click(keep);
    await within(section).findByText(
      `Kept: the supplied text is shown in place of this conversion’s text, in revision ${short(CORRECTED_AT)}.`,
    );
    expect(bodies(api)).toEqual([
      {
        workspace_id: WS,
        item_id: SOURCE,
        base_revision: HEAD,
        digest: DIGEST,
        corrected_markdown: SUPPLIED_MARKDOWN,
        adopts: PROPOSAL,
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    await says(
      section,
      'the text shown is text an agent supplied, which ada kept over the converter’s text.',
    );
    expect(api.violations).toEqual([]);
  });

  it('says in words that the service would not keep it', async () => {
    const { api, section } = await open({
      extraction: later,
      overrides: {
        [GET_PROPOSAL]: () => ({
          status: 200,
          body: {
            id: PROPOSAL,
            workspace_id: WS,
            title: 'Supply the text',
            description: '',
            kind: 'supply_extraction',
            status: 'accepted',
            base_revision: HEAD,
            proposal_revision: 'c'.repeat(40),
            content_digest: 'd'.repeat(64),
            created_at: '2026-01-01T23:30:00.000Z',
            created_by: 'agent-7',
            created_via: 'mcp_delegation',
            changes: [{ kind: 'supply_extraction', item_id: SOURCE, markdown: 'Some other text.' }],
          },
        }),
      },
    });
    const keep = await within(section).findByRole('button', { name: 'Keep the supplied text' });
    await waitFor(() => expect(keep.getAttribute('aria-disabled')).toBe('false'));
    fireEvent.click(keep);
    await says(section, 'Keeping the supplied text: The supplied text could not be kept.');
    await says(section, 'the corrected text is not the text the proposal supplied');
    expect(within(section).queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(api.violations).toEqual([]);
  });
});

describe('who may correct', () => {
  it('tells a reader without `write` why neither action is offered', async () => {
    const { api, section } = await open({ permissions: ['read'] });
    await says(
      section,
      'Correcting the text needs permission to change this workspace, which you do not have.',
    );
    await says(
      section,
      'Reconverting this file needs permission to change this workspace, which you do not have.',
    );
    expect(within(section).queryByRole('button', { name: 'Correct text' })).toBeNull();
    expect(within(section).queryByRole('button', { name: /Reconvert/ })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('keeps both actions in an archived workspace, says why they cannot run, and sends nothing', async () => {
    const { api, section } = await open({
      archivedAt: '2026-01-03T08:00:00.000Z',
      extraction: extractionOf({ supplied: SUPPLIED }),
    });
    const correct = await within(section).findByRole('button', { name: 'Correct text' });
    const reconvert = button(section, 'Reconvert the whole file');
    const keep = button(section, 'Keep the supplied text');
    for (const action of [correct, reconvert, keep]) {
      expect(action.getAttribute('aria-disabled')).toBe('true');
      expect(describedBy(action)).toContain('nothing in it can be changed until it is unarchived');
      fireEvent.click(action);
    }
    expect(within(section).queryByRole('textbox', { name: 'Your correction' })).toBeNull();
    expect(bodies(api)).toEqual([]);
    expect(api.callsTo('/api/imports/redigest-item')).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('offers neither action on a tab that reads another revision than the one shown', async () => {
    const service = correctionsService();
    const api = mockApi({
      ...service.handlers,
      [ITEM]: (body) =>
        JSON.stringify(body.at) === JSON.stringify({ kind: 'revision', revision: OLD })
          ? { status: 200, body: sourceDocument(OLD, extractionOf()) }
          : (service.handlers[ITEM]?.(body) ?? fail(404, 'not_found', 'none')),
    });
    renderAt(`/w/${WS}/i/${SOURCE}?rev=${OLD}`);
    const section = await screen.findByRole('region', { name: 'Corrections' });
    await says(
      section,
      `This tab reads revision ${short(OLD)}, not the version this workspace shows (revision ${short(HEAD)}). Corrections start from the version the workspace shows.`,
    );
    const correct = button(section, 'Correct text');
    expect(correct.getAttribute('aria-disabled')).toBe('true');
    expect(describedBy(correct)).toContain(`This tab reads revision ${short(OLD)}`);
    fireEvent.click(correct);
    expect(within(section).queryByRole('textbox', { name: 'Your correction' })).toBeNull();
    expect(bodies(api)).toEqual([]);
    expect(api.violations).toEqual([]);
  });
});
