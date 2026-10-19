/**
 * The YAML text box starting again (`restarts`) lands in the commit that asks for it, before the
 * browser paints it: a layout effect, not a passive one (UI M2 integration re-check, carried to
 * UI M3). A passive effect runs after the paint, so a frame would show the older text, and a
 * keystroke in that frame would be typed into text about to be replaced.
 *
 * Seen from a layout effect of a part of the page committed after the box: every layout effect of
 * a commit runs before any passive one, in the order of the tree, so that effect sees the new text
 * only when the box started again in a layout effect of its own.
 */
import { EditorView } from '@codemirror/view';
import { act, render, screen } from '@testing-library/react';
import { useLayoutEffect } from 'react';
import { describe, expect, it } from 'vitest';
import { YamlEditor } from '../../src/lib/YamlEditor';

/** The text the box holds, read from its own view; null before the box is built. */
function textOf(): string | null {
  const box = screen.queryByRole('textbox', { name: 'Rules' });
  if (box === null) return null;
  return EditorView.findFromDOM(box)?.state.doc.toString() ?? null;
}

/** What a layout effect committed after the box saw in it, commit by commit. */
const seen: string[] = [];

function After({ commit }: { commit: number }) {
  useLayoutEffect(() => {
    seen.push(`${commit}: ${textOf() ?? 'no box yet'}`);
  });
  return null;
}

function Page({ text, restarts }: { text: string; restarts: number }) {
  return (
    <>
      <YamlEditor label="Rules" initialText={text} onChange={() => undefined} restarts={restarts} />
      <After commit={restarts} />
    </>
  );
}

describe('the YAML box starting again', () => {
  it('holds the new text in the commit that asks for it, before any passive effect', async () => {
    seen.length = 0;
    const { rerender } = render(<Page text="prefix: old" restarts={0} />);
    // The box is built after the first commit (a passive effect); from then on it is there.
    await act(async () => undefined);
    expect(textOf()).toBe('prefix: old');
    act(() => rerender(<Page text="prefix: new" restarts={1} />));
    expect(seen.at(-1)).toBe('1: prefix: new');
    expect(textOf()).toBe('prefix: new');
  });
});
