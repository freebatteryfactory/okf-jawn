/**
 * A drag or drop that carries files is cancelled before the draft editor sees it, whether or not
 * the folder tree is listed (its drag backend cancels drags too, later), and a text drag is not.
 * The root guard listens in the capturing phase; CodeMirror skips an event that is already
 * cancelled, so it never reads a dropped file into a draft. That CodeMirror then inserts nothing
 * of a file and still inserts dragged text is shown in a browser (explorer-editing.spec.ts):
 * happy-dom has no layout for the editor to place a drop with.
 *
 * What the editor "sees" is a capturing listener on its own text box, which runs after the
 * window's capturing listener and before the editor's own.
 */
import { act, cleanup, configure, createEvent, fireEvent, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { resetDraftSessions } from '../../src/features/drafts/saving';
import { type Handler, mockApi, ok } from './support/api';
import { textBox } from './support/editor';
import { dropOf, droppedFile, fileOf } from './support/imports';
import {
  explorerHandlers,
  listing,
  NOTE,
  renderAt,
  WS,
  writableWorkspace,
} from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

const LIST = '/api/items/list-items';

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

/** The draft editor's text box, with a record of what its own handlers would be handed. */
async function editor(overrides: Record<string, Handler>) {
  mockApi(
    explorerHandlers({
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      ...overrides,
    }),
  );
  renderAt(`/w/${WS}/i/${NOTE}`);
  fireEvent.click(await screen.findByRole('button', { name: 'Edit' }));
  const box = textBox('Text of Meeting notes');
  const seen: Array<{ type: string; cancelled: boolean }> = [];
  for (const type of ['dragover', 'drop']) {
    box.addEventListener(
      type,
      (event) => {
        seen.push({ type, cancelled: event.defaultPrevented });
        // The editor's own handler is not run: this is what it would have been handed.
        event.stopImmediatePropagation();
      },
      { capture: true },
    );
  }
  return { box, seen };
}

/** A whole drag onto `target`, as a browser delivers one. */
function dragOnto(target: Element, carried: unknown) {
  fireEvent.dragEnter(target, { dataTransfer: carried });
  fireEvent(target, createEvent.dragOver(target, { dataTransfer: carried }));
  fireEvent(target, createEvent.drop(target, { dataTransfer: carried }));
}

const files = () => dropOf([droppedFile(fileOf('private.txt', 'DROPPED-FROM-DISK'))]);
const text = () => ({
  types: ['text/plain'],
  items: [],
  files: [],
  dropEffect: 'none',
  effectAllowed: 'all',
});

describe('a drag onto the draft editor', () => {
  // First: the tree's drag backend is one for the whole page and, once it has seen a drag of files
  // here, cancels the drops that follow in the same file. This test is about the guard alone.
  it('of text is not cancelled: the editor takes it', async () => {
    const { box, seen } = await editor({ [LIST]: () => ok(listing([], [])) });
    dragOnto(box, text());
    expect(seen).toEqual([
      { type: 'dragover', cancelled: false },
      { type: 'drop', cancelled: false },
    ]);
  });

  for (const [name, tree] of [
    ['the folder tree is listed', {}],
    ['the folder tree is not listed', { [LIST]: () => ok(listing([], [])) }],
  ] as const) {
    it(`of files is cancelled before the editor sees it when ${name}`, async () => {
      const { box, seen } = await editor(tree);
      dragOnto(box, files());
      expect(seen).toEqual([
        { type: 'dragover', cancelled: true },
        { type: 'drop', cancelled: true },
      ]);
    });
  }
});
