/**
 * The original beside its digest (UI M4a, ruling A2; SPEC section 7: "Original and digest may be
 * compared side by side"). For a text-like original, its text (the bytes, `download_object`) and
 * the converted text the reader shows (`read_item`, `view: 'text'`) stand side by side in
 * `@codemirror/merge`, read-only, each pane named for whose text it is, with the places that
 * differ counted and reachable from the keyboard. Each side is read as far as asked, and says so;
 * it is offered from the text viewer and from a CSV spreadsheet, and not where there is no
 * converted text.
 */
import { EditorView } from '@codemirror/view';
import { configure, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Extraction, ReadItemResponse } from '../../src/api/generated/types.gen';
import { SpreadsheetView } from '../../src/features/viewers/sheet/SpreadsheetView';
import { TextOriginal } from '../../src/features/viewers/text/TextOriginal';
import { fail, type Json, mockApi, ok } from './support/api';
import {
  answerBytes,
  appearanceOf,
  extractionOf,
  READ_ITEM,
  renderViewer,
  sourceRead,
  utf8,
  viewerProps,
} from './support/m4a-sheets';
import { HEAD, SOURCE, WS } from './support/workspace';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const PATH = 'notes.txt';
const COMPARE = 'Compare the original with its converted text';
const ORIGINAL_PANE = `Original: ${PATH}`;
const CONVERTED_PANE = 'Converted text: what the reader shows';

const original = ['Agenda', '', 'Budget: 1.2 M', 'Done.', ''].join('\n');
const converted = ['# Agenda', '', 'Budget: 1.2 M', '', 'Done.', ''].join('\n');

const wholeRead = {
  workspace_id: WS,
  item_id: SOURCE,
  at: { kind: 'revision', revision: HEAD },
  view: 'text',
  selection: { kind: 'all' },
  cursor: null,
  max_bytes: 65_536,
  max_images: 0,
};

function renderText(
  bytes: Uint8Array<ArrayBuffer>,
  extraction: Extraction = extractionOf(),
  mediaType = 'text/plain',
) {
  return renderViewer(
    <TextOriginal {...viewerProps(PATH, appearanceOf(mediaType, bytes.length, extraction))} />,
  );
}

async function pane(name: string): Promise<EditorView> {
  const box = await screen.findByRole('textbox', { name });
  const view = EditorView.findFromDOM(box);
  if (view === null) throw new Error(`no editor behind ${name}`);
  return view;
}

const readOf = (markdown: string, extra: Partial<ReadItemResponse> = {}) =>
  ok(sourceRead(PATH, markdown, extra));

describe('the original beside its converted text (A2)', () => {
  it('compares an empty retained original with its digest without requesting an invalid byte range', async () => {
    // SPEC section 7 permits original/digest comparison; zero bytes is a valid original.
    // The expected panes are authored here: empty original and the supplied digest text.
    const api = mockApi({ [READ_ITEM]: () => readOf('An empty original.\n') });
    const object = answerBytes(new Uint8Array());
    renderText(new Uint8Array());
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    expect((await pane(ORIGINAL_PANE)).state.doc.toString()).toBe('');
    expect((await pane(CONVERTED_PANE)).state.doc.toString()).toBe('An empty original.\n');
    expect(object.reads).toEqual([]);
    expect(screen.getByText('All 0 bytes of the original are compared.')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('stands the two side by side, read-only, each pane named, with the places that differ counted', async () => {
    const api = mockApi({ [READ_ITEM]: () => readOf(converted) });
    answerBytes(utf8(original));
    renderText(utf8(original));
    const toggle = await screen.findByRole('button', { name: COMPARE });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    // Nothing of the converted text is read before the comparison is asked for.
    expect(api.callsTo(READ_ITEM)).toEqual([]);
    await userEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');

    const left = await pane(ORIGINAL_PANE);
    const right = await pane(CONVERTED_PANE);
    expect(left.state.doc.toString()).toBe(original);
    expect(right.state.doc.toString()).toBe(converted);
    for (const name of [ORIGINAL_PANE, CONVERTED_PANE]) {
      expect(screen.getByRole('textbox', { name }).getAttribute('aria-readonly')).toBe('true');
    }
    // One merge view holds both panes, the original first; each pane's name is shown above it.
    const merge = left.dom.closest('.cm-mergeView');
    expect(merge?.contains(right.dom)).toBe(true);
    const comparison = screen.getByRole('region', {
      name: 'The original beside its converted text',
    });
    expect(comparison.textContent).toContain(ORIGINAL_PANE);
    expect(comparison.textContent).toContain(CONVERTED_PANE);
    expect(await screen.findByText('2 places differ.')).toBeTruthy();
    expect(api.callsTo(READ_ITEM)).toEqual([wholeRead]);
    expect(api.violations).toEqual([]);
  });

  it('says there is no difference when the two are the same', async () => {
    const api = mockApi({ [READ_ITEM]: () => readOf(original) });
    answerBytes(utf8(original));
    renderText(utf8(original));
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    expect(
      await screen.findByText('No difference: the converted text is the original’s text.'),
    ).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('names the shown text for whose it is: a person’s correction, an agent’s text', async () => {
    let corrected = true;
    const api = mockApi({
      [READ_ITEM]: () =>
        readOf(converted, {
          extraction: corrected
            ? { status: 'completed', text_origin: 'converter', corrected: true }
            : { status: 'failed', text_origin: 'supplied_by_agent', corrected: false },
        }),
    });
    answerBytes(utf8(original));
    const { unmount } = renderText(utf8(original), extractionOf({ corrected: true }));
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    expect(await pane('Corrected text: what the reader shows')).toBeTruthy();
    unmount();

    corrected = false;
    renderText(utf8(original), extractionOf({ text_origin: 'supplied_by_agent' }));
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    expect(await pane('Text supplied by an agent: what the reader shows')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('reads each side as far as asked, says how far, and continues each by its own way', async () => {
    const long = `${'a\n'.repeat(40_000)}`;
    const api = mockApi({
      [READ_ITEM]: (body: Json) =>
        body.cursor === 'more'
          ? readOf('second part\n')
          : readOf('first part\n', { truncated: true, next_cursor: 'more' }),
    });
    const object = answerBytes(utf8(long));
    renderText(utf8(long));
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    const right = await pane(CONVERTED_PANE);
    expect(right.state.doc.toString()).toBe('first part\n');
    expect(
      screen.getByText(
        'Only what is read of each is compared: what is not read yet shows as a difference at the end.',
      ),
    ).toBeTruthy();
    expect(
      screen.getByText('The first part of the converted text is compared; more can be read.'),
    ).toBeTruthy();
    expect(screen.getByText('Bytes 1–65,536 of 80,000 of the original are compared.')).toBeTruthy();

    await userEvent.click(screen.getByRole('button', { name: 'Read more of the converted text' }));
    await waitFor(() => expect(right.state.doc.toString()).toBe('first part\nsecond part\n'));
    expect(api.callsTo(READ_ITEM)).toEqual([wholeRead, { ...wholeRead, cursor: 'more' }]);
    expect(screen.getByText('All the converted text is compared, in 2 parts.')).toBeTruthy();
    // The part read is where keyboard focus goes: the button has gone.
    await waitFor(() => expect(document.activeElement).toBe(right.contentDOM));

    await userEvent.click(screen.getByRole('button', { name: 'Read more of the original' }));
    const left = await pane(ORIGINAL_PANE);
    await waitFor(() => expect(left.state.doc.length).toBe(80_000));
    expect(object.reads.map((read) => read.range)).toEqual(['bytes=0-65535', 'bytes=65536-79999']);
    expect(screen.getByText('All 80,000 bytes of the original are compared.')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('moves the caret and keyboard focus to the next and the previous place that differs', async () => {
    const api = mockApi({ [READ_ITEM]: () => readOf(converted) });
    answerBytes(utf8(original));
    renderText(utf8(original));
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    const left = await pane(ORIGINAL_PANE);
    await screen.findByText('2 places differ.');
    await userEvent.click(screen.getByRole('button', { name: 'Next difference' }));
    expect(document.activeElement).toBe(left.contentDOM);
    const first = left.state.selection.main.head;
    await userEvent.click(screen.getByRole('button', { name: 'Next difference' }));
    const second = left.state.selection.main.head;
    expect(second).toBeGreaterThan(first);
    await userEvent.click(screen.getByRole('button', { name: 'Previous difference' }));
    expect(left.state.selection.main.head).toBe(first);
    expect(api.violations).toEqual([]);
  });

  it('shows a refused read of the converted text in words, with its retry', async () => {
    let refuse = true;
    const api = mockApi({
      [READ_ITEM]: () => (refuse ? fail(500, 'internal', 'the store failed') : readOf(converted)),
    });
    answerBytes(utf8(original));
    renderText(utf8(original));
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    expect(
      await screen.findByText(/Reading the converted text: Something went wrong/),
    ).toBeTruthy();
    refuse = false;
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await pane(CONVERTED_PANE)).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('is not offered where there is no converted text, and says why', async () => {
    const api = mockApi({});
    answerBytes(utf8(original));
    renderText(
      utf8(original),
      extractionOf({ digest: null, outcome: { status: 'unsupported' }, text_origin: 'none' }),
      'text/x-rust',
    );
    expect(
      await screen.findByText(
        'There is no converted text to compare the original with: Bytes kept; text extraction is not supported for this format.',
      ),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: COMPARE })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('is offered from a CSV spreadsheet, whose original is text, and not from a workbook', async () => {
    const csv = 'Quarter,Revenue\nQ1,1.2\n';
    const table = '| Quarter | Revenue |\n| --- | --- |\n| Q1 | 1.2 |\n';
    const api = mockApi({ [READ_ITEM]: () => ok(sourceRead('figures.csv', table)) });
    answerBytes(utf8(csv));
    const { unmount } = renderViewer(
      <SpreadsheetView {...viewerProps('figures.csv', appearanceOf('text/csv', csv.length))} />,
    );
    await userEvent.click(await screen.findByRole('button', { name: COMPARE }));
    expect((await pane('Original: figures.csv')).state.doc.toString()).toBe(csv);
    expect((await pane(CONVERTED_PANE)).state.doc.toString()).toBe(table);
    unmount();

    renderViewer(
      <SpreadsheetView
        {...viewerProps(
          'figures.xlsx',
          appearanceOf('application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', 9000),
        )}
      />,
    );
    await screen.findByRole('table');
    expect(screen.queryByRole('button', { name: COMPARE })).toBeNull();
    expect(api.violations).toEqual([]);
  });
});
