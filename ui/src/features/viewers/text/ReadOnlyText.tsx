/**
 * A text shown read-only in CodeMirror (`@codemirror/view` 6.41, `@codemirror/state` 6.6): an
 * original's text (`TextOriginal.tsx`) and each pane of the original beside its converted text
 * (`OriginalBesideDigest.tsx`).
 *
 * It is a multi-line text box to assistive technology, named by its label and marked read-only
 * (`EditorState.readOnly`: CodeMirror then sets `aria-readonly`). It stays focusable, so the
 * keyboard moves the caret and selects through the text, and nothing typed changes it; Tab is not
 * bound, so it moves keyboard focus on. Line numbers are drawn beside it, and long lines wrap.
 *
 * A language is given only where an installed `@codemirror/lang-*` fits the media type (JSON,
 * Markdown, YAML). No highlight style is installed (`@codemirror/language` is not a dependency of
 * the page), so a language adds structure, not colors: the look is the owner's call.
 *
 * Every replacement character is marked (`cm-not-utf8`, with a title saying what it is), and the
 * lines a selection names are marked as one band (`cm-selected-lines`). The words for both are the
 * caller's, beside the text: a mark is only drawn, not announced.
 *
 * CodeMirror inserts its styles as a `<style>` element in the document (style-mod; a CSP nonce
 * can be given with `EditorView.cspNonce`), and sets its inline styles through the CSSOM.
 */
import { json } from '@codemirror/lang-json';
import { markdown } from '@codemirror/lang-markdown';
import { yaml } from '@codemirror/lang-yaml';
import { EditorState, type Extension, RangeSetBuilder, StateField } from '@codemirror/state';
import {
  Decoration,
  type DecorationSet,
  EditorView,
  lineNumbers,
  MatchDecorator,
  ViewPlugin,
  type ViewUpdate,
} from '@codemirror/view';
import { type RefObject, useLayoutEffect, useRef } from 'react';
import type { TextRange } from '../../../api/generated/types.gen';

/** The media type without its parameters, in lower case. */
const essence = (mediaType: string) => (mediaType.split(';', 1)[0] ?? '').trim().toLowerCase();

const MARKDOWN: ReadonlySet<string> = new Set(['text/markdown', 'text/x-markdown']);
const YAML: ReadonlySet<string> = new Set([
  'application/yaml',
  'application/x-yaml',
  'text/yaml',
  'text/x-yaml',
]);

/** The installed language that fits a media type, or none. */
export function languageFor(mediaType: string): Extension {
  const type = essence(mediaType);
  if (type === 'application/json') return json();
  if (MARKDOWN.has(type)) return markdown();
  if (YAML.has(type)) return yaml();
  return [];
}

export const NOT_UTF8_TITLE =
  'The replacement character “�”: bytes of the original that are not UTF-8, or the character itself';

const look = EditorView.theme({
  '&': { border: '1px solid GrayText', borderRadius: '0.25rem', maxHeight: '32rem' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'ui-monospace, monospace', lineHeight: '1.5', overflow: 'auto' },
  // The text box itself shows keyboard focus, like every other control of the workspace.
  '.cm-content:focus-visible': { outline: '3px solid Highlight', outlineOffset: '-3px' },
  '.cm-gutters': { backgroundColor: 'Canvas', color: 'CanvasText', borderRight: '1px solid' },
  '.cm-selected-lines': {
    backgroundColor: 'color-mix(in srgb, Highlight 20%, transparent)',
    boxShadow: 'inset 3px 0 0 Highlight',
  },
  '.cm-not-utf8': { outline: '1px solid CanvasText', backgroundColor: 'Mark', color: 'MarkText' },
});

const replacements = new MatchDecorator({
  regexp: /�/g,
  decoration: Decoration.mark({ class: 'cm-not-utf8', attributes: { title: NOT_UTF8_TITLE } }),
});

/** Every replacement character, marked. */
const markReplacements = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = replacements.createDeco(view);
    }
    update(update: ViewUpdate) {
      this.decorations = replacements.updateDeco(update, this.decorations);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

const selectedLine = Decoration.line({ class: 'cm-selected-lines' });

/** The lines `range` names, as far as the text has them, marked. */
function markLines(range: TextRange): Extension {
  const build = (state: EditorState): DecorationSet => {
    const builder = new RangeSetBuilder<Decoration>();
    const last = Math.min(range.end, state.doc.lines);
    for (let number = range.start; number <= last; number += 1) {
      const line = state.doc.line(number);
      builder.add(line.from, line.from, selectedLine);
    }
    return builder.finish();
  };
  return StateField.define<DecorationSet>({
    create: build,
    update: (marks, transaction) => (transaction.docChanged ? build(transaction.state) : marks),
    provide: (field) => EditorView.decorations.from(field),
  });
}

/** What a read-only text is drawn with: its name, its language, its marks. */
export function readOnlyText(label: string, mediaType: string, marked: TextRange | null) {
  return [
    EditorState.readOnly.of(true),
    EditorView.contentAttributes.of({ 'aria-label': label }),
    EditorView.lineWrapping,
    lineNumbers(),
    languageFor(mediaType),
    markReplacements,
    marked === null ? [] : markLines(marked),
    look,
  ];
}

/** Put the caret on the first marked line once the text has it, and only once. */
function placeCaret(view: EditorView, marked: TextRange | null, placed: RefObject<boolean>) {
  if (placed.current || marked === null || marked.start > view.state.doc.lines) return;
  placed.current = true;
  const at = view.state.doc.line(marked.start).from;
  view.dispatch({
    selection: { anchor: at },
    effects: EditorView.scrollIntoView(at, { y: 'start' }),
  });
}

export interface ReadOnlyTextHandle {
  /** Put the caret at `position` of the text, scroll there, and keyboard focus in the text box. */
  focusAt: (position: number) => void;
}

export interface ReadOnlyTextProps {
  /** The text box's accessible name. */
  label: string;
  text: string;
  mediaType: string;
  /** Lines to mark; the caret starts on the first when the text has it. */
  marked: TextRange | null;
  /** The element that describes the text box. */
  describedBy?: string | undefined;
  /** Set, while the text is on the page, to what moves the caret. */
  handle?: RefObject<ReadOnlyTextHandle | null>;
}

/**
 * The text read-only. Its name, language and marks are read when it mounts; a text that grows
 * (more of the original read) is added at its end, so the caret and the scroll stay where they
 * were. A text that is not a continuation of the one shown replaces it.
 */
export function ReadOnlyText({
  label,
  text,
  mediaType,
  marked,
  describedBy,
  handle,
}: ReadOnlyTextProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const first = useRef({ label, text, mediaType, marked, describedBy });
  // The caret goes to the first marked line once the text has it, and only once.
  const placed = useRef(false);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (host === null) return;
    const start = first.current;
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: start.text,
        extensions: [
          readOnlyText(start.label, start.mediaType, start.marked),
          start.describedBy === undefined
            ? []
            : EditorView.contentAttributes.of({ 'aria-describedby': start.describedBy }),
        ],
      }),
    });
    viewRef.current = view;
    placeCaret(view, start.marked, placed);
    return () => {
      viewRef.current = null;
      view.destroy();
    };
  }, []);

  useLayoutEffect(() => {
    const view = viewRef.current;
    if (view === null) return;
    const shown = view.state.doc.toString();
    if (shown === text) return;
    view.dispatch({
      changes: text.startsWith(shown)
        ? { from: shown.length, insert: text.slice(shown.length) }
        : { from: 0, to: shown.length, insert: text },
    });
    placeCaret(view, first.current.marked, placed);
  }, [text]);

  useLayoutEffect(() => {
    if (handle === undefined) return;
    handle.current = {
      focusAt: (position) => {
        const view = viewRef.current;
        if (view === null) return;
        const at = Math.min(position, view.state.doc.length);
        view.dispatch({
          selection: { anchor: at },
          effects: EditorView.scrollIntoView(at, { y: 'start' }),
        });
        view.focus();
      },
    };
    return () => {
      handle.current = null;
    };
  }, [handle]);

  return <div ref={hostRef} className="read-only-text" />;
}
