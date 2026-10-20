/**
 * The text box of a correction, beside the text it corrects: a CodeMirror merge view
 * (`@codemirror/merge` `MergeView`) of two editors side by side. On the left, the text shown now,
 * read only (`EditorState.readOnly`: it can be focused and read with the caret, and announces
 * itself read only); on the right, the person's text, which starts as the same text and is
 * theirs to change, with undo history and the default keys. Changed lines are marked on both
 * sides, by a gutter mark and a line of color under the changed words; since color and marks are
 * not words, the places that differ are also reported (`onChange`, as lines of the person's
 * text) for the caller to say in words.
 *
 * Each editor is a multi-line text box to assistive technology, named by its label. Tab is not
 * bound, so it moves keyboard focus on, as in any text field. No colors are added beyond the
 * merge view's own marks: the look is the owner's call, as for `drafts/MarkdownEditor.tsx`.
 *
 * Text. The person's side holds its own text: `shown` is its first text, read once, when it
 * mounts, and is never set again from outside, so a keystroke cannot be undone by an older value
 * arriving late. The left side follows `shown`: when the text shown changes (the workspace shows
 * another version), the left side is given it, and the places are compared again; the person's
 * text stays as it is.
 *
 * Focus. With `autoFocus`, keyboard focus goes into the person's side when it mounts.
 */
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { markdown } from '@codemirror/lang-markdown';
import { getChunks, MergeView } from '@codemirror/merge';
import { Compartment, EditorState, type Text } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, type ViewUpdate } from '@codemirror/view';
import { useEffect, useLayoutEffect, useRef } from 'react';
import type { LineSpan } from './corrections';

export interface CorrectionEditorProps {
  /** The text shown now: the left side, and the person's first text. */
  shown: string;
  /** The left side's accessible name. */
  shownLabel: string;
  /** The person's side's accessible name. */
  label: string;
  /** The element that describes the person's side: what it started from. */
  describedBy?: string;
  /** The person's text after each change, and the lines of it that differ from the text shown. */
  onChange: (text: string, places: LineSpan[]) => void;
  /** Put keyboard focus in the person's side when it mounts. */
  autoFocus?: boolean;
}

const look = EditorView.theme({
  '&': { border: '1px solid GrayText', borderRadius: '0.25rem' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'ui-monospace, monospace', lineHeight: '1.5' },
  '.cm-content': { padding: '0.5rem 0', minHeight: '10rem' },
  '.cm-gutters': { background: 'Canvas', color: 'GrayText', borderRight: '1px solid GrayText' },
  // The text box itself shows keyboard focus, like every other control of the workspace.
  '.cm-content:focus-visible': { outline: '3px solid Highlight', outlineOffset: '-3px' },
});

/** The lines of `b` each changed chunk covers, one-based and inclusive. */
function placesIn(state: EditorState, b: Text): LineSpan[] {
  const chunks = getChunks(state)?.chunks ?? [];
  return chunks.map((chunk) => ({
    start: b.lineAt(chunk.fromB).number,
    end: b.lineAt(chunk.endB).number,
  }));
}

/** The accessible name, and description, of one side's text box. */
function naming(label: string, describedBy?: string) {
  return EditorView.contentAttributes.of({
    'aria-label': label,
    ...(describedBy === undefined ? {} : { 'aria-describedby': describedBy }),
  });
}

export function CorrectionEditor({
  shown,
  shownLabel,
  label,
  describedBy,
  onChange,
  autoFocus = false,
}: CorrectionEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const mergeRef = useRef<MergeView | null>(null);
  const first = useRef({ shown, shownLabel, label, describedBy, autoFocus });
  const report = useRef(onChange);
  const namingA = useRef(new Compartment());
  const namingB = useRef(new Compartment());

  useEffect(() => {
    report.current = onChange;
  }, [onChange]);

  // Made in a layout effect, so focus is in the text box in the commit that shows it.
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (host === null) return;
    const start = first.current;
    // Either side's change compares the two again; what is reported is the person's side.
    const changed = (update: ViewUpdate) => {
      const merge = mergeRef.current;
      if (merge === null || !update.docChanged) return;
      const b = merge.b.state.doc;
      report.current(b.toString(), placesIn(update.state, b));
    };
    const shared = [markdown(), EditorView.lineWrapping, lineNumbers(), look];
    const merge = new MergeView({
      parent: host,
      a: {
        doc: start.shown,
        extensions: [
          ...shared,
          EditorState.readOnly.of(true),
          keymap.of(defaultKeymap),
          namingA.current.of(naming(start.shownLabel)),
          EditorView.updateListener.of(changed),
        ],
      },
      b: {
        doc: start.shown,
        extensions: [
          ...shared,
          history(),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          namingB.current.of(naming(start.label, start.describedBy)),
          EditorView.updateListener.of(changed),
        ],
      },
    });
    mergeRef.current = merge;
    if (start.autoFocus) merge.b.focus();
    return () => {
      mergeRef.current = null;
      merge.destroy();
    };
  }, []);

  // The left side follows the text shown; the person's side is theirs.
  useEffect(() => {
    const a = mergeRef.current?.a;
    if (a === undefined || a.state.doc.toString() === shown) return;
    a.dispatch({ changes: { from: 0, to: a.state.doc.length, insert: shown } });
  }, [shown]);

  useEffect(() => {
    mergeRef.current?.a.dispatch({
      effects: namingA.current.reconfigure(naming(shownLabel)),
    });
  }, [shownLabel]);

  useEffect(() => {
    mergeRef.current?.b.dispatch({
      effects: namingB.current.reconfigure(naming(label, describedBy)),
    });
  }, [label, describedBy]);

  return <div ref={hostRef} className="correction-editor" />;
}
