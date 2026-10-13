/**
 * The Markdown text box in unit tests. It is found by its role and name, like any control; its text
 * is changed through CodeMirror's own view (happy-dom has no layout and no real key input), as the
 * typing a person does. Real typing is covered by the browser specs.
 */
import { EditorView } from '@codemirror/view';
import { act, screen } from '@testing-library/react';

function viewOf(box: HTMLElement): EditorView {
  const view = EditorView.findFromDOM(box);
  if (view === null) throw new Error('this text box is not a Markdown editor');
  return view;
}

/** The Markdown text box with this accessible name. */
export function textBox(name: string | RegExp): HTMLElement {
  return screen.getByRole('textbox', { name });
}

export async function findTextBox(name: string | RegExp): Promise<HTMLElement> {
  return screen.findByRole('textbox', { name });
}

/** The text the box holds. */
export function textIn(box: HTMLElement): string {
  return viewOf(box).state.doc.toString();
}

/** Type `text` at the end of the box. */
export function typeInto(box: HTMLElement, text: string) {
  const view = viewOf(box);
  act(() => {
    view.dispatch({
      changes: { from: view.state.doc.length, insert: text },
      userEvent: 'input.type',
    });
  });
}

/** Replace everything in the box with `text`. */
export function replaceText(box: HTMLElement, text: string) {
  const view = viewOf(box);
  act(() => {
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: text },
      userEvent: 'input.type',
    });
  });
}

/** Where the caret is in the box: the place in its text, counted from the start. */
export function caretIn(box: HTMLElement): number {
  return viewOf(box).state.selection.main.head;
}

/** Put the caret at `place` in the box, as a click or an arrow key does. */
export function putCaret(box: HTMLElement, place: number) {
  const view = viewOf(box);
  act(() => {
    view.dispatch({ selection: { anchor: place } });
  });
}

/** Type `text` where the caret is. */
export function typeAtCaret(box: HTMLElement, text: string) {
  const view = viewOf(box);
  act(() => {
    view.dispatch(view.state.replaceSelection(text), { userEvent: 'input.type' });
  });
}
