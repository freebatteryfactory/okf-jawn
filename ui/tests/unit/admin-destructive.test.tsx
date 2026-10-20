/**
 * The destructive actions' dialog (ruling C3), shared by a workspace purge, an item purge, a
 * restore and a connector's revocation: an alert dialog that says what will happen, bound to what
 * it was opened with (a `base_revision` among it), whose retry repeats the request under the same
 * idempotency key, with keyboard focus moved into it and back to what opened it. No further
 * challenge is asked: the service's grant, the human session and the revision are the guard.
 */

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { useRef, useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  type Attempt,
  DestructiveDialog,
  useDestructive,
} from '../../src/features/admin/destructive';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

interface Purge {
  workspaceId: string;
  baseRevision: string;
}

class Refused extends Error {
  constructor() {
    super('refused');
  }
}

/** A purge button over the head `head`, its dialog, and what each confirmation sent. */
function Harness({
  head,
  act: send,
}: {
  head: string;
  act: (attempt: Attempt<Purge>) => Promise<string>;
}) {
  const opener = useRef<HTMLButtonElement | null>(null);
  const [done, setDone] = useState('');
  const purge = useDestructive<Purge, string>({
    act: send,
    returnFocus: opener,
    onDone: (result) => setDone(result),
  });
  return (
    <>
      <button
        type="button"
        ref={opener}
        onClick={() => purge.open({ workspaceId: 'w1', baseRevision: head })}
      >
        Purge this workspace…
      </button>
      <p role="status">{done}</p>
      <DestructiveDialog
        action={purge}
        title="Purge this workspace?"
        description={<p>Everything in it is removed.</p>}
        confirmLabel="Purge it"
        busyLabel="Purging…"
        context="Purging this workspace"
      >
        <p>At revision {purge.opened?.baseRevision}</p>
      </DestructiveDialog>
    </>
  );
}

function renderHarness(head: string, send: (attempt: Attempt<Purge>) => Promise<string>) {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <Harness head={head} act={send} />
    </QueryClientProvider>,
  );
  const rerender = (next: string) =>
    view.rerender(
      <QueryClientProvider client={client}>
        <Harness head={next} act={send} />
      </QueryClientProvider>,
    );
  return { rerender };
}

const opener = () => screen.getByRole('button', { name: 'Purge this workspace…' });
const dialog = () => screen.findByRole('alertdialog', { name: 'Purge this workspace?' });

describe('a destructive action’s dialog (C3)', { timeout: 30_000 }, () => {
  it('says what will happen, starts on Cancel, and gives focus back to what opened it', async () => {
    const send = vi.fn(async () => 'done');
    renderHarness('a'.repeat(40), send);
    opener().focus();
    fireEvent.click(opener());
    const open = await dialog();
    expect(open.textContent).toContain('Everything in it is removed.');
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' })),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener()));
    expect(send).not.toHaveBeenCalled();
  });

  it('is bound to the revision it was opened at, whatever the head has become since', async () => {
    const send = vi.fn(async (_attempt: Attempt<Purge>) => 'done');
    const { rerender } = renderHarness('a'.repeat(40), send);
    fireEvent.click(opener());
    await dialog();
    rerender('b'.repeat(40));
    expect((await dialog()).textContent).toContain(`At revision ${'a'.repeat(40)}`);
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0]?.[0].values).toEqual({
      workspaceId: 'w1',
      baseRevision: 'a'.repeat(40),
    });
  });

  it('repeats a failed request under the same key, in the dialog, then closes with what it answered', async () => {
    let answer: () => Promise<string> = async () => {
      throw new Refused();
    };
    const send = vi.fn((_attempt: Attempt<Purge>) => answer());
    renderHarness('a'.repeat(40), send);
    fireEvent.click(opener());
    await dialog();
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    expect(await screen.findByText(/Purging this workspace/)).toBeTruthy();
    expect(screen.getByRole('alertdialog')).toBeTruthy();
    answer = async () => 'purged';
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Try again/ }));
    });
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(screen.getByRole('status').textContent).toBe('purged');
    const keys = send.mock.calls.map(([attempt]) => attempt.key);
    expect(keys).toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
    await waitFor(() => expect(document.activeElement).toBe(opener()));
  });

  it('keeps the key when the dialog is closed after a failure and opened again on the same revision', async () => {
    const send = vi.fn(async (_attempt: Attempt<Purge>): Promise<string> => {
      throw new Refused();
    });
    renderHarness('a'.repeat(40), send);
    fireEvent.click(opener());
    await dialog();
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    await screen.findByText(/Purging this workspace/);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    fireEvent.click(opener());
    // A dialog opened again starts with no failure in it.
    expect((await dialog()).textContent).not.toContain('Purging this workspace');
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    const [first, second] = send.mock.calls.map(([attempt]) => attempt.key);
    expect(second).toBe(first);
  });

  it('asks under a new key once the service has answered, or for another revision', async () => {
    const send = vi.fn(async (_attempt: Attempt<Purge>) => 'done');
    const { rerender } = renderHarness('a'.repeat(40), send);
    fireEvent.click(opener());
    await dialog();
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    fireEvent.click(opener());
    await dialog();
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    rerender('b'.repeat(40));
    fireEvent.click(opener());
    await dialog();
    fireEvent.click(screen.getByRole('button', { name: 'Purge it' }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(3));
    const keys = send.mock.calls.map(([attempt]) => attempt.key);
    expect(new Set(keys).size).toBe(3);
  });
});
