/**
 * Plain text and code (UI M4a, ruling A2). A text or code original is read with the transport's
 * byte read (`download_object`, a range at a time: never `get_object`, never the whole of a large
 * file at once), decoded as UTF-8 with what is not UTF-8 shown as the replacement character and
 * marked, and shown read-only in CodeMirror, named, with a language only where an installed
 * `@codemirror/lang-*` fits. The page says how much of the original is shown and offers the rest,
 * one range per press; a `lines` selection is marked, and said in words.
 */
import { EditorView } from '@codemirror/view';
import { configure, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Selection } from '../../src/api/generated/types.gen';
import { TextOriginal } from '../../src/features/viewers/text/TextOriginal';
import { mockApi } from './support/api';
import { answerBytes, appearanceOf, renderViewer, utf8, viewerProps } from './support/m4a-sheets';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const PATH = 'notes.txt';
const LABEL = `Original text of ${PATH}`;

function renderText(
  bytes: Uint8Array<ArrayBuffer>,
  mediaType = 'text/plain',
  selection: Selection = { kind: 'all' },
  path = PATH,
) {
  return renderViewer(
    <TextOriginal {...viewerProps(path, appearanceOf(mediaType, bytes.length), selection)} />,
  );
}

/** The CodeMirror view behind a named text box. */
async function editorNamed(name: string = LABEL): Promise<EditorView> {
  const box = await screen.findByRole('textbox', { name });
  const view = EditorView.findFromDOM(box);
  if (view === null) throw new Error(`no editor behind ${name}`);
  return view;
}

describe('a text or code original (A2)', () => {
  it('explicitly keeps the named read-only content in the tab order inside its scroll region', async () => {
    // A2's keyboard/zero-axe requirement: CodeMirror's scroll region must contain one declared
    // Tab stop. The expected tabindex is the accessible content's, never an extra wrapper stop.
    const bytes = utf8('{"value": 1}\n');
    const api = mockApi({});
    answerBytes(bytes);
    renderText(bytes, 'application/json');
    const view = await editorNamed();
    expect(view.contentDOM.getAttribute('tabindex')).toBe('0');
    expect(view.scrollDOM.querySelectorAll('[tabindex="0"]')).toHaveLength(1);
    expect(view.contentDOM.getAttribute('aria-readonly')).toBe('true');
    expect(api.violations).toEqual([]);
  });

  it('reads a short original in one range, decodes it as UTF-8 and shows it read-only, named', async () => {
    const text = 'Agenda\n\nBudget: 1.2 M €\nDone.\n';
    const bytes = utf8(text);
    const api = mockApi({});
    const object = answerBytes(bytes);
    renderText(bytes);
    const view = await editorNamed();
    expect(view.state.doc.toString()).toBe(text);
    const box = screen.getByRole('textbox', { name: LABEL });
    expect(box.getAttribute('aria-readonly')).toBe('true');
    expect(view.state.readOnly).toBe(true);
    expect(object.reads).toEqual([{ method: 'GET', range: `bytes=0-${bytes.length - 1}` }]);
    expect(screen.getByText(`All ${bytes.length} bytes of the original are shown.`)).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Show more of the original/ })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('shows the first part of a long original, says how much, and reads the rest on request, one range per press', async () => {
    // A two-byte character lies across the end of the first part.
    const text = `${'a'.repeat(65_535)}é${'b'.repeat(100_000 - 65_537)}`;
    const bytes = utf8(text);
    expect(bytes.length).toBe(100_000);
    const api = mockApi({});
    const object = answerBytes(bytes);
    renderText(bytes);
    const view = await editorNamed();
    expect(view.state.doc.length).toBe(65_535);
    expect(view.state.doc.toString()).not.toContain('�');
    expect(
      screen.getByText('Bytes 1–65,536 of 100,000 of the original are shown (64 KB of 98 KB).'),
    ).toBeTruthy();
    // Nothing more is read until the person asks.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(object.reads).toEqual([{ method: 'GET', range: 'bytes=0-65535' }]);

    await userEvent.click(screen.getByRole('button', { name: 'Show more of the original' }));
    await waitFor(() => expect(view.state.doc.length).toBe(text.length));
    expect(view.state.doc.toString()).toBe(text);
    expect(object.reads).toEqual([
      { method: 'GET', range: 'bytes=0-65535' },
      { method: 'GET', range: 'bytes=65536-99999' },
    ]);
    expect(screen.getByText('All 100,000 bytes of the original are shown.')).toBeTruthy();
    // Keyboard focus goes to the text, where the part read begins: the button has gone.
    await waitFor(() => expect(document.activeElement).toBe(view.contentDOM));
    expect(view.state.selection.main.head).toBe(65_535);
    expect(api.violations).toEqual([]);
  });

  it('shows bytes that are not UTF-8 as the replacement character, marks each and says so', async () => {
    const bytes = new Uint8Array([0x61, 0xff, 0x62, 0x0a, 0xc3, 0x28, 0x0a]);
    const api = mockApi({});
    answerBytes(bytes);
    renderText(bytes);
    const view = await editorNamed();
    expect(view.state.doc.toString()).toBe('a�b\n�(\n');
    expect(
      screen.getByText(
        'Some bytes of the original are not UTF-8: each place is shown as the replacement character “�” and marked (2 in what is shown).',
      ),
    ).toBeTruthy();
    const marks = view.contentDOM.querySelectorAll('.cm-not-utf8');
    expect([...marks].map((mark) => mark.textContent)).toEqual(['�', '�']);
    expect(api.violations).toEqual([]);
  });

  it('says so when the original itself holds the replacement character', async () => {
    const bytes = utf8('a�b\n');
    const api = mockApi({});
    answerBytes(bytes);
    renderText(bytes);
    await editorNamed();
    expect(
      screen.getByText(
        'The original holds the replacement character “�” itself, marked (1 in what is shown).',
      ),
    ).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('marks the lines a selection names, puts the caret on the first, and says how they count', async () => {
    const text = ['one', 'two', 'three', 'four', 'five', 'six', ''].join('\n');
    const bytes = utf8(text);
    const api = mockApi({});
    answerBytes(bytes);
    renderText(bytes, 'text/plain', { kind: 'lines', range: { start: 2, end: 3 } });
    const view = await editorNamed();
    const marked = [...view.contentDOM.querySelectorAll('.cm-selected-lines')];
    expect(marked.map((line) => line.textContent)).toEqual(['two', 'three']);
    expect(view.state.selection.main.head).toBe(view.state.doc.line(2).from);
    expect(
      screen.getByText(
        'Lines 2–3 are marked. The selection counts the lines of the item’s text, which are the original’s lines only where the conversion kept them line for line.',
      ),
    ).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('says when the lines a selection names are past the part shown, or past the end', async () => {
    const long = utf8(`${'line\n'.repeat(20_000)}`);
    const api = mockApi({});
    answerBytes(long);
    const { unmount } = renderText(long, 'text/plain', {
      kind: 'lines',
      range: { start: 15_000, end: 15_001 },
    });
    await editorNamed();
    expect(
      screen.getByText(
        'Lines 15,000–15,001 are past the part shown, which ends at line 13,108. Show more of the original to reach them.',
      ),
    ).toBeTruthy();
    unmount();

    const short = utf8('one\ntwo\n');
    answerBytes(short);
    renderText(short, 'text/plain', { kind: 'lines', range: { start: 10, end: 12 } });
    await editorNamed();
    expect(screen.getByText('The original has 2 lines: lines 10–12 are not in it.')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('gives a language only where an installed one fits', async () => {
    const api = mockApi({});
    const cases: [string, string, string | null][] = [
      ['application/json', 'data.json', 'json'],
      ['text/markdown; charset=utf-8', 'notes.md', 'markdown'],
      ['application/yaml', 'rules.yaml', 'yaml'],
      ['text/x-rust', 'main.rs', null],
    ];
    for (const [mediaType, path, language] of cases) {
      const bytes = utf8('{}\n');
      answerBytes(bytes);
      const { unmount } = renderText(bytes, mediaType, { kind: 'all' }, path);
      const view = await editorNamed(`Original text of ${path}`);
      expect(view.contentDOM.getAttribute('data-language')).toBe(language);
      unmount();
    }
    expect(api.violations).toEqual([]);
  });

  it('reads nothing of an empty original, and says it is empty', async () => {
    const api = mockApi({});
    const object = answerBytes(new Uint8Array([]));
    renderText(new Uint8Array([]));
    expect(await screen.findByText('The original is empty: 0 bytes.')).toBeTruthy();
    expect(object.reads).toEqual([]);
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('shows a refused read in words, with its retry, and the text once it is read', async () => {
    const bytes = utf8('Hello\n');
    const api = mockApi({});
    const object = answerBytes(bytes);
    object.refuse(500, 'internal', 'the store failed');
    renderText(bytes);
    expect(
      await screen.findByText(/Reading the original: Something went wrong in the service\./),
    ).toBeTruthy();
    object.answer();
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    const view = await editorNamed();
    expect(view.state.doc.toString()).toBe('Hello\n');
    expect(api.violations).toEqual([]);
  });
});
