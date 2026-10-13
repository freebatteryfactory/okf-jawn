/**
 * A plain YAML text field (CodeMirror 6 with the YAML language, undo history and the default
 * keys), and the list of what is wrong with its text.
 *
 * Like the Markdown text box it is a multi-line text box to assistive technology, named by
 * `label`; Tab is not bound, so it moves keyboard focus on; no colors are added. It holds its own
 * text: `initialText` is read once, when it mounts, and every change is reported through
 * `onChange`. Its text is never set from outside while it is mounted, so a keystroke cannot be
 * undone by an older value arriving late.
 *
 * Reading the text is not done at every keystroke: `usePausedReading` reads it a short pause
 * after the last change (`YAML_PAUSE_MS`), and at once when asked (`flush`): when keyboard focus
 * leaves the text box, when the view changes, and before anything is sent. So what is sent or
 * shown as the value is never older than the text.
 *
 * `YamlFindings` shows what reading the text found: each problem with its line and column, as an
 * alert, and each warning as a note. The text box is described by it.
 */
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { yaml } from '@codemirror/lang-yaml';
import { Compartment, EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { useCallback, useEffect, useRef } from 'react';
import { keyed } from './keys';
import { describeProblem, readYaml, YAML_PAUSE_MS, type YamlRead } from './yaml';

export interface YamlEditorProps {
  /** The text box's accessible name. */
  label: string;
  /** The text the editor starts with. */
  initialText: string;
  onChange: (text: string) => void;
  /** The element that describes the text box: what is wrong with its text. */
  describedBy?: string;
  /** Keyboard focus left the text box. */
  onBlur?: () => void;
}

export interface PausedReading {
  /** The text changed: it is read a pause after the last change. */
  change: (text: string) => void;
  /** Read now what waits to be read. Answers that reading, or null when nothing waited. */
  flush: () => YamlRead | null;
}

/**
 * Read a text as YAML a short pause after its last change, not at every keystroke. `onRead` gets
 * each text that was read with what reading it found. A text still waiting when the caller leaves
 * the page is read then, so it is never dropped unread.
 */
export function usePausedReading(onRead: (text: string, read: YamlRead) => void): PausedReading {
  const report = useRef(onRead);
  useEffect(() => {
    report.current = onRead;
  }, [onRead]);
  const waiting = useRef<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const flush = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    const text = waiting.current;
    if (text === null) return null;
    waiting.current = null;
    const read = readYaml(text);
    report.current(text, read);
    return read;
  }, []);
  const change = useCallback(
    (text: string) => {
      waiting.current = text;
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = setTimeout(flush, YAML_PAUSE_MS);
    },
    [flush],
  );
  useEffect(() => () => void flush(), [flush]);
  return { change, flush };
}

const look = EditorView.theme({
  '&': { border: '1px solid GrayText', borderRadius: '0.25rem' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'ui-monospace, monospace', lineHeight: '1.5' },
  '.cm-content': { padding: '0.5rem', minHeight: '8rem' },
  // The text box itself shows keyboard focus, like every other control of the workspace.
  '.cm-content:focus-visible': { outline: '3px solid Highlight', outlineOffset: '-3px' },
});

export function YamlEditor({ label, initialText, onChange, describedBy, onBlur }: YamlEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const first = useRef(initialText);
  const report = useRef(onChange);
  const left = useRef(onBlur);
  const naming = useRef(new Compartment());

  useEffect(() => {
    report.current = onChange;
    left.current = onBlur;
  }, [onChange, onBlur]);

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
          yaml(),
          look,
          naming.current.of([]),
          EditorView.domEventHandlers({
            blur: () => {
              left.current?.();
            },
          }),
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

  return <div ref={hostRef} className="yaml-editor" />;
}

/** What reading a YAML text found; an empty element (kept for the description) when nothing. */
export function YamlFindings({ id, read }: { id: string; read: YamlRead | null }) {
  const problems = read !== null && !read.ok ? read.problems : [];
  const warnings = read?.ok === true ? read.warnings : [];
  return (
    <div id={id} className="yaml-findings">
      {problems.length > 0 && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>This YAML cannot be read. Nothing of it is sent.</strong>
          </p>
          <ul>
            {keyed(problems.map(describeProblem), (words) => words).map(({ key, value }) => (
              <li key={key}>{value}</li>
            ))}
          </ul>
        </div>
      )}
      {warnings.length > 0 && (
        <div role="note" className="notice">
          <p>Read as text; another YAML reader could take it differently. Quote it to be sure:</p>
          <ul>
            {keyed(warnings.map(describeProblem), (words) => words).map(({ key, value }) => (
              <li key={key}>{value}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
