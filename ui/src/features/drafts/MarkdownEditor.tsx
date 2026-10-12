/**
 * A plain Markdown text field: CodeMirror 6 with the Markdown language, undo history and the
 * default keys.
 *
 * It is a multi-line text box to assistive technology, named by `label`. Tab is not bound, so it
 * moves keyboard focus on as in any text field, and the selection is the browser's own. No colors
 * are added: the look is the owner's call.
 *
 * The editor holds its own text. `initialText` is read once, when it mounts; every change is
 * reported through `onChange`. Its text is never set from outside while it is mounted, so a
 * keystroke cannot be undone by an older value arriving late; a parent that must replace the text
 * mounts another editor (a new `key`).
 */
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { markdown } from '@codemirror/lang-markdown';
import { Compartment, EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { useEffect, useRef } from 'react';

export interface MarkdownEditorProps {
  /** The text box's accessible name. */
  label: string;
  /** The text the editor starts with. */
  initialText: string;
  onChange: (text: string) => void;
  /** The element that describes the text box: how its save stands. */
  describedBy?: string;
}

const look = EditorView.theme({
  '&': { border: '1px solid GrayText', borderRadius: '0.25rem' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'ui-monospace, monospace', lineHeight: '1.5' },
  '.cm-content': { padding: '0.5rem', minHeight: '12rem' },
  // The text box itself shows keyboard focus, like every other control of the workspace.
  '.cm-content:focus-visible': { outline: '3px solid Highlight', outlineOffset: '-3px' },
});

export function MarkdownEditor({ label, initialText, onChange, describedBy }: MarkdownEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const first = useRef(initialText);
  const report = useRef(onChange);
  const naming = useRef(new Compartment());

  useEffect(() => {
    report.current = onChange;
  }, [onChange]);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) return;
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: first.current,
        extensions: [
          history(),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          markdown(),
          EditorView.lineWrapping,
          look,
          naming.current.of([]),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) report.current(update.state.doc.toString());
          }),
        ],
      }),
    });
    viewRef.current = view;
    return () => {
      viewRef.current = null;
      view.destroy();
    };
  }, []);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: naming.current.reconfigure(
        EditorView.contentAttributes.of({
          'aria-label': label,
          ...(describedBy === undefined ? {} : { 'aria-describedby': describedBy }),
        }),
      ),
    });
  }, [label, describedBy]);

  return <div ref={hostRef} className="markdown-editor" />;
}
