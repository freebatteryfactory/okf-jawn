/**
 * A plain YAML text field (CodeMirror 6 with the YAML language, undo history and the default
 * keys), and the list of what is wrong with its text.
 *
 * Like the Markdown text box it is a multi-line text box to assistive technology, named by
 * `label`; Tab is not bound, so it moves keyboard focus on; no colors are added. It holds its own
 * text: `initialText` is read when it mounts, and every change is reported through `onChange`.
 * Its text is never set from outside as a value is, so a keystroke cannot be undone by an older
 * value arriving late.
 *
 * The one way its text changes from outside is starting again (`restarts`): the caller says that
 * the box holds nothing of the person’s and that what it shows is no longer what it should start
 * from. The box then starts from the `initialText` of that commit, as a box just mounted would:
 * that text, the caret at its start, no undo history, and nothing reported through `onChange`. It
 * is the same element on the page, so keyboard focus that was in it is still in it, and focus
 * that was elsewhere is not moved. A caller that remounted the box for this (a `key`) would take
 * focus to the page body.
 *
 * Reading the text is not done at every keystroke, and never on the page's thread
 * (`lib/yaml-reading.ts`): `usePausedReading` starts reading it a short pause after the last
 * change (`YAML_PAUSE_MS`), and at once when asked (`flush`): when keyboard focus leaves the text
 * box, when the view changes, when a button is pressed that would send. The answer comes a moment
 * later. Until it has come the text is not read (`unread`, `pending`), and the callers send
 * nothing meanwhile: what is sent is never older than the text. A change of the text ends a
 * reading still under way, whose answer would be of an older text, so it is never reported.
 *
 * `YamlFindings` shows what reading the text found: each problem with its line and column, as an
 * alert, and each warning as a note. The text box is described by it.
 */
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { yaml } from '@codemirror/lang-yaml';
import { Compartment, EditorState } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { keyed } from './keys';
import { describeProblem, YAML_PAUSE_MS, type YamlRead } from './yaml';
import { readOffThread } from './yaml-reading';

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
  /**
   * Counts the times the caller started the text again. When it changes, the box starts again
   * from `initialText`, in place: the caller does so only while the box holds nothing typed.
   */
  restarts?: number;
}

export interface PausedReading {
  /**
   * The text changed: its reading starts a pause after the last change. A reading still under way
   * is of an older text: it is ended, and its answer is never reported.
   */
  change: (text: string) => void;
  /** Start reading now the text that waits for the pause to pass. */
  flush: () => void;
  /** Forget the text that waits and end the reading under way: nothing is reported of either. */
  cancel: () => void;
  /**
   * A text waits to be read or is being read, so what was reported last is older than the text.
   * For an event handler: it is true from the change itself, before anything is drawn again.
   */
  unread: () => boolean;
  /** The same, for drawing. */
  pending: boolean;
}

/**
 * Read a text as YAML a short pause after its last change, not at every keystroke, and off the
 * page's thread. `onRead` gets each text that was read with what reading it found (a refusal
 * when it took too long: `readOffThread`), unless the text changed meanwhile. A text still
 * waiting when the caller leaves the page is read then, and reported to the last `onRead`, so
 * it is never dropped unread. A reading already under way is not ended by leaving: it runs to
 * its answer, or to the one-second limit, and the answer goes to the `onRead` of the caller
 * that left, which no longer draws anything. `cancel` is different: it drops the text that
 * waits and ends the reading under way, so that nothing of either is reported.
 */
export function usePausedReading(onRead: (text: string, read: YamlRead) => void): PausedReading {
  const report = useRef(onRead);
  useEffect(() => {
    report.current = onRead;
  }, [onRead]);
  const waiting = useRef<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** What ends the reading under way; null when none is. */
  const underWay = useRef<(() => void) | null>(null);
  const [pending, setPending] = useState(false);
  const endUnderWay = useCallback(() => {
    underWay.current?.();
    underWay.current = null;
  }, []);
  const flush = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    const text = waiting.current;
    if (text === null) return;
    waiting.current = null;
    underWay.current = readOffThread(text, (read) => {
      underWay.current = null;
      setPending(false);
      report.current(text, read);
    });
  }, []);
  const change = useCallback(
    (text: string) => {
      endUnderWay();
      waiting.current = text;
      setPending(true);
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = setTimeout(flush, YAML_PAUSE_MS);
    },
    [flush, endUnderWay],
  );
  const cancel = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    waiting.current = null;
    endUnderWay();
    setPending(false);
  }, [endUnderWay]);
  const unread = useCallback(() => waiting.current !== null || underWay.current !== null, []);
  useEffect(() => () => flush(), [flush]);
  return { change, flush, cancel, unread, pending };
}

const look = EditorView.theme({
  '&': { border: '1px solid GrayText', borderRadius: '0.25rem' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'ui-monospace, monospace', lineHeight: '1.5' },
  '.cm-content': { padding: '0.5rem', minHeight: '8rem' },
  // The text box itself shows keyboard focus, like every other control of the workspace.
  '.cm-content:focus-visible': { outline: '3px solid Highlight', outlineOffset: '-3px' },
});

/** The text box’s name and description, as attributes of the box itself. */
const named = (label: string, describedBy: string | undefined) =>
  EditorView.contentAttributes.of({
    'aria-label': label,
    ...(describedBy === undefined ? {} : { 'aria-describedby': describedBy }),
  });

export function YamlEditor({
  label,
  initialText,
  onChange,
  describedBy,
  onBlur,
  restarts = 0,
}: YamlEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const report = useRef(onChange);
  const left = useRef(onBlur);
  const naming = useRef(new Compartment());
  // What the box starts from, as the last commit gave it: read when it mounts and when it starts
  // again, never in between.
  const start = useRef({ initialText, label, describedBy });
  useLayoutEffect(() => {
    start.current = { initialText, label, describedBy };
  }, [initialText, label, describedBy]);

  useEffect(() => {
    report.current = onChange;
    left.current = onBlur;
  }, [onChange, onBlur]);

  /** The box as it starts: the text, the caret at its start, and no undo history. */
  const startState = useCallback(() => {
    const from = start.current;
    return EditorState.create({
      doc: from.initialText,
      extensions: [
        history(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        yaml(),
        look,
        naming.current.of(named(from.label, from.describedBy)),
        EditorView.domEventHandlers({
          blur: () => {
            left.current?.();
          },
        }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) report.current(update.state.doc.toString());
        }),
      ],
    });
  }, []);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) return;
    const view = new EditorView({ parent: host, state: startState() });
    viewRef.current = view;
    return () => {
      viewRef.current = null;
      view.destroy();
    };
  }, [startState]);

  // Starting again, in the commit that asks for it and before it is painted. The view keeps its
  // element and gives focus back to it only when it had it (`EditorView.setState`), so focus is
  // where it was. A box not built yet starts from the same text when it is built.
  const started = useRef(restarts);
  useLayoutEffect(() => {
    if (started.current === restarts) return;
    started.current = restarts;
    viewRef.current?.setState(startState());
  }, [restarts, startState]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: naming.current.reconfigure(named(label, describedBy)),
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
