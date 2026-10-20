/** A1: a cached read never inherits a continuation's focus request from another selection/revision. */
import { act, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useContinuation } from '../../src/features/reader/continuation';

function Reading({ identity, parts, more, fetch }: {
  identity: string;
  parts: number;
  more: boolean;
  fetch: () => Promise<unknown>;
}) {
  const continuation = useContinuation({
    parts, continuing: false, failed: false, fetchNextPage: fetch,
  }, identity);
  return <>
    <div ref={continuation.bodyRef}>
      {Array.from({ length: parts }, (_, index) => <div key={index} data-part tabIndex={-1}>Part {index + 1}</div>)}
    </div>
    {more && <button type="button" ref={continuation.buttonRef} onClick={continuation.continueReading}>Continue reading</button>}
  </>;
}

it.each(['selection', 'revision'])('cancels pending focus when a cached multipart %s replaces the read', async (dimension) => {
  let release: (() => void) | undefined;
  const fetch = vi.fn(() => new Promise<void>((resolve) => { release = resolve; }));
  const { rerender } = render(<Reading identity="original" parts={1} more fetch={fetch} />);
  const button = screen.getByRole('button', { name: 'Continue reading' });
  act(() => { button.focus(); button.click(); });
  expect(fetch).toHaveBeenCalledOnce();
  // Like a pointer selection in WebKit, selection does not focus its control. The old
  // continuation button leaves with focus on it; the new answer is already cached in 3 parts.
  rerender(<Reading identity={`different-${dimension}`} parts={3} more={false} fetch={fetch} />);
  expect(document.activeElement?.closest('[data-part]')).toBeNull();
  await act(async () => { release?.(); });
  expect(document.activeElement?.closest('[data-part]')).toBeNull();
});
