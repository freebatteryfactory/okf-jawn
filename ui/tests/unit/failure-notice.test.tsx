/**
 * Every ApiError code reads as plain language with the action it allows; NotImplemented is never
 * success. A retry keeps the notice and its focused button on screen until it settles.
 */

import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, StrictMode, useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { ErrorCode } from '../../src/api/generated/types.gen';
import { zErrorCode } from '../../src/api/generated/zod.gen';
import { FailureNotice } from '../../src/features/errors/FailureNotice';
import { classifyFailure, messageFor, shouldRetry } from '../../src/features/errors/failure';
import { useRetry } from '../../src/features/errors/retry';

/** The notice's way home is a router link, so it renders inside a minimal router. */
async function renderNotice(element: ReactNode) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => element }),
    history: createMemoryHistory({ initialEntries: ['/somewhere'] }),
  });
  render(<RouterProvider router={router} />);
  await screen.findByRole('alert');
  return router;
}

const codes: ErrorCode[] = zErrorCode.options.flatMap((option) => [...option.values]);
const apiError = (code: ErrorCode, extra: Record<string, unknown> = {}) => ({
  code,
  message: `server words for ${code}`,
  ...extra,
});

/** A failed operation with its retry, as a caller holds it: `run` repeats the operation. */
function Retried({
  context,
  error,
  run,
}: {
  context: string;
  error: unknown;
  run: () => Promise<unknown>;
}) {
  const retry = useRetry({ request: 'one', failed: true, error, run, loaded: () => null });
  return <FailureNotice context={context} error={retry.error} retry={retry} />;
}

/** What a repeated operation answers: its content, or a failure with this code. */
type Answer = 'content' | ErrorCode;

/**
 * An operation as a query with nothing loaded behaves: it leaves its failed state the moment it
 * is asked again, and holds each answer until the test gives it. Pressing "Elsewhere" renders the
 * caller again, as any change of its state does, and counts the presses.
 */
function Operation({
  asked,
  request = 'one',
}: {
  asked: Array<(answer: Answer) => void>;
  request?: string;
}) {
  const [presses, setPresses] = useState(0);
  const [state, setState] = useState<{ status: 'failed' | 'asking' | 'loaded'; error: unknown }>({
    status: 'failed',
    error: apiError('internal'),
  });
  const retry = useRetry({
    request,
    failed: state.status === 'failed',
    error: state.error,
    run: () =>
      new Promise<void>((resolve) => {
        setState({ status: 'asking', error: null });
        asked.push((answer) => {
          setState(
            answer === 'content'
              ? { status: 'loaded', error: null }
              : { status: 'failed', error: apiError(answer) },
          );
          resolve();
        });
      }),
    loaded: () => document.getElementById('content'),
  });
  return (
    <>
      <button type="button" data-presses={presses} onClick={() => setPresses(presses + 1)}>
        Elsewhere
      </button>
      {state.status === 'asking' && !retry.retrying && <p>Asking…</p>}
      {retry.shown && <FailureNotice context="Opening" error={retry.error} retry={retry} />}
      {state.status === 'loaded' && (
        <h2 id="content" tabIndex={-1}>
          The content
        </h2>
      )}
    </>
  );
}

/** Render the operation, failed, with keyboard focus on its retry button. */
function failedWithFocus(request?: string) {
  const asked: Array<(answer: Answer) => void> = [];
  const view = render(
    request === undefined ? (
      <Operation asked={asked} />
    ) : (
      <Operation asked={asked} request={request} />
    ),
  );
  const button = screen.getByRole('button', { name: 'Try again' });
  const notice = button.parentElement;
  if (notice === null) throw new Error('the retry button is not in a notice');
  act(() => button.focus());
  /** Give the retry numbered `nth` (from 1) its answer. */
  const answer = (nth: number, given: Answer) => {
    const give = asked[nth - 1];
    if (give === undefined) throw new Error(`the operation was not asked ${nth} times`);
    act(() => give(given));
  };
  return { ...view, asked, button, notice, answer };
}

/**
 * An operation whose state the test sets, as it changes when something other than its retry asks
 * it again: a query refetched when the window regains focus.
 */
function Watched({
  status,
  code = 'internal',
  notice = 'one',
}: {
  status: 'failed' | 'asking' | 'loaded';
  code?: ErrorCode;
  /** The notice's key: under another key it is another notice, a new element. */
  notice?: string;
}) {
  const [presses, setPresses] = useState(0);
  const retry = useRetry({
    request: 'one',
    failed: status === 'failed',
    error: apiError(code),
    run: () => Promise.resolve(),
    loaded: () => document.getElementById('content'),
  });
  return (
    <>
      <h1 tabIndex={-1}>The page</h1>
      <button type="button" data-presses={presses} onClick={() => setPresses(presses + 1)}>
        Elsewhere
      </button>
      {retry.shown && (
        <FailureNotice key={notice} context="Opening" error={retry.error} retry={retry} />
      )}
      {status === 'loaded' && (
        <h2 id="content" tabIndex={-1}>
          The content
        </h2>
      )}
    </>
  );
}

/**
 * An operation that keeps running when its component asks something else, as a mutation does, or
 * a query another component still reads: whatever is asked has failed, and each run ends when the
 * test says so.
 */
function Outliving({ request, runs }: { request: string; runs: Array<() => void> }) {
  const retry = useRetry({
    request,
    failed: true,
    error: apiError('internal'),
    run: () =>
      new Promise<void>((resolve) => {
        runs.push(resolve);
      }),
    loaded: () => null,
  });
  return <FailureNotice context="Opening" error={retry.error} retry={retry} />;
}

/** Ask for a retry of "one" from the keyboard, then have the component ask "two" instead. */
async function retryOutlived() {
  const runs: Array<() => void> = [];
  const view = render(<Outliving request="one" runs={runs} />);
  const button = screen.getByRole('button', { name: 'Try again' });
  const notice = button.parentElement;
  if (notice === null) throw new Error('the retry button is not in a notice');
  act(() => button.focus());
  await userEvent.keyboard('{Enter}');
  expect(button.textContent).toBe('Trying again…');
  view.rerender(<Outliving request="two" runs={runs} />);
  // "two" has failed too, and no retry of it has been asked for.
  expect(button.textContent).toBe('Try again');
  /** End the run numbered `nth` (from 1). */
  const end = async (nth: number) => {
    const run = runs[nth - 1];
    if (run === undefined) throw new Error(`the operation was not run ${nth} times`);
    await act(async () => run());
  };
  return { runs, button, notice, end };
}

/** Render the watched operation, failed, with keyboard focus on its retry button. */
function watchedWithFocus() {
  const view = render(<Watched status="failed" />);
  const button = screen.getByRole('button', { name: 'Try again' });
  const notice = button.parentElement;
  if (notice === null) throw new Error('the retry button is not in a notice');
  act(() => button.focus());
  return { ...view, button, notice };
}

describe('failure messages', () => {
  it('covers every generated error code with its own words and an action decision', () => {
    expect(codes.length).toBe(13);
    const titles = new Set<string>();
    for (const code of codes) {
      const message = messageFor(classifyFailure(apiError(code)));
      expect(message.title.length).toBeGreaterThan(0);
      expect(message.advice.length).toBeGreaterThan(0);
      titles.add(message.title);
    }
    expect(titles.size).toBe(codes.length);
  });

  it('reads not_implemented as not available in this build and offers no success or retry', async () => {
    const run = vi.fn(async () => undefined);
    await renderNotice(
      <Retried
        context="Reading this item"
        error={apiError('not_implemented', { message: 'read_item is not implemented yet' })}
        run={run}
      />,
    );
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('Reading this item: Not available in this build.');
    expect(alert.textContent).toContain('has not been built yet');
    expect(alert.textContent).toContain('read_item is not implemented yet');
    expect(screen.queryByRole('button')).toBeNull();
  });

  it.each([
    ['invalid_input', 'The request was not valid.', null],
    ['unauthenticated', 'You are not signed in.', 'Try again after signing in'],
    ['forbidden', 'You do not have permission to do this.', null],
    ['not_found', 'This could not be found.', null],
    ['conflict', 'This changed since it was loaded.', 'Reload'],
    ['already_issued', 'This was already issued.', null],
    ['in_progress', 'This is still in progress.', 'Try again'],
    ['too_large', 'This is too large to show at once.', null],
    ['unsupported', 'This is not supported for this item.', null],
    ['unavailable', 'The service is temporarily unavailable.', 'Try again'],
    ['cancelled', 'The operation was cancelled.', 'Try again'],
    ['internal', 'Something went wrong in the service.', 'Try again'],
    ['not_implemented', 'Not available in this build.', null],
  ] as const)('shows %s as "%s" with action %s', async (code, title, button) => {
    const run = vi.fn(async () => undefined);
    await renderNotice(<Retried context="Opening" error={apiError(code)} run={run} />);
    expect(screen.getByRole('alert').textContent).toContain(`Opening: ${title}`);
    expect(screen.getByRole('alert').textContent).toContain(`server words for ${code}`);
    if (button === null) {
      expect(screen.queryByRole('button')).toBeNull();
    } else {
      await userEvent.click(screen.getByRole('button', { name: button }));
      expect(run).toHaveBeenCalledTimes(1);
      // The retry settled: the button offers the action again.
      expect(await screen.findByRole('button', { name: button })).toBeTruthy();
    }
  });

  it('shows a failure that allows no retry as the alert alone, as before, where a retry could be offered', () => {
    const run = vi.fn(async () => undefined);
    render(<Retried context="Reading" error={apiError('forbidden')} run={run} />);
    const alert = screen.getByRole('alert');
    expect(alert.className).toBe('failure-notice');
    expect(alert.hasAttribute('tabindex')).toBe(false);
    expect(alert.parentElement?.className).not.toBe('failure-notice');
    expect(alert.textContent).toContain('Reading: You do not have permission to do this.');
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('offers no retry without the retry that keeps the notice on screen', () => {
    render(<FailureNotice context="Refreshing" error={apiError('unavailable')} />);
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('Refreshing: The service is temporarily unavailable.');
    expect(alert.className).toBe('failure-notice');
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('offers the way home for not_found as a router navigation, and shows the field and reference', async () => {
    const router = await renderNotice(
      <FailureNotice
        context="Opening"
        error={apiError('not_found', { field: '/item_id', request_id: 'req-7' })}
      />,
    );
    const home = screen.getByRole('link', { name: 'Go to your workspaces' });
    expect(home.getAttribute('href')).toBe('/');
    expect(screen.getByText('/item_id')).toBeTruthy();
    expect(screen.getByText('req-7')).toBeTruthy();
    await userEvent.click(home);
    expect(router.state.location.pathname).toBe('/');
  });

  it('inside a workspace, not_found goes back to that workspace', async () => {
    const workspaceId = '11111111-1111-4111-8111-111111111111';
    await renderNotice(
      <FailureNotice
        context="Reading this item"
        error={apiError('not_found')}
        workspaceId={workspaceId}
      />,
    );
    expect(screen.getByRole('link', { name: 'Go to this workspace' }).getAttribute('href')).toBe(
      `/w/${workspaceId}`,
    );
  });

  it('uses the typed retry_after of in_progress', () => {
    const failure = classifyFailure(
      apiError('in_progress', {
        detail: {
          kind: 'in_progress',
          mutation_id: '33333333-3333-4333-8333-333333333333',
          retry_after: 12,
        },
      }),
    );
    expect(messageFor(failure).advice).toBe('Try again in 12 seconds.');
  });

  it('keeps a server error whose detail this client cannot read as that server error', () => {
    const failure = classifyFailure(apiError('conflict', { detail: { kind: 'from_the_future' } }));
    expect(failure.kind).toBe('api');
    expect(messageFor(failure).title).toBe('This changed since it was loaded.');
  });

  it('reads a thrown network error as an unreachable service with a retry', async () => {
    const run = vi.fn(async () => undefined);
    render(<Retried context="Listing" error={new TypeError('fetch failed')} run={run} />);
    expect(screen.getByRole('alert').textContent).toContain(
      'Listing: The workspace service could not be reached.',
    );
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(run).toHaveBeenCalledOnce();
    expect(await screen.findByRole('button', { name: 'Try again' })).toBeTruthy();
  });

  it('retries once only what may pass by itself', () => {
    expect(shouldRetry(0, apiError('unavailable'))).toBe(true);
    expect(shouldRetry(0, new TypeError('offline'))).toBe(true);
    expect(shouldRetry(1, apiError('unavailable'))).toBe(false);
    for (const code of ['not_implemented', 'forbidden', 'not_found', 'invalid_input'] as const) {
      expect(shouldRetry(0, apiError(code))).toBe(false);
    }
  });
});

describe('a retry never unmounts the focused control', () => {
  it('keeps the notice and its focused button while the operation is asked again, and says so politely', async () => {
    const { asked, button, notice, answer } = failedWithFocus();
    const alert = screen.getByRole('alert');
    const words = alert.textContent;
    expect(screen.getByRole('status').textContent).toBe('');
    await userEvent.keyboard('{Enter}');
    expect(asked).toHaveLength(1);
    // The operation has left its failed state; the notice is still the same one.
    expect(notice.isConnected).toBe(true);
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);
    expect(button.textContent).toBe('Trying again…');
    expect(button.getAttribute('aria-disabled')).toBe('true');
    expect(button.hasAttribute('disabled')).toBe(false);
    // Said by a polite status beside the alert; the alert itself is untouched, so not re-announced.
    const status = screen.getByRole('status');
    expect(status.textContent).toBe('Opening: trying again…');
    expect(alert.contains(status)).toBe(false);
    expect(alert.contains(button)).toBe(false);
    expect(screen.getByRole('alert')).toBe(alert);
    expect(alert.textContent).toBe(words);
    expect(screen.queryByText('Asking…')).toBeNull();
    // A second press while it is in flight asks nothing more.
    await userEvent.keyboard('{Enter}');
    expect(asked).toHaveLength(1);
    answer(1, 'content');
    await waitFor(() => expect(notice.isConnected).toBe(false));
  });

  it('moves focus to the content a successful retry loaded', async () => {
    const { notice, answer } = failedWithFocus();
    await userEvent.keyboard('{Enter}');
    answer(1, 'content');
    const content = await screen.findByRole('heading', { name: 'The content' });
    await waitFor(() => expect(document.activeElement).toBe(content));
    expect(notice.isConnected).toBe(false);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('keeps focus on the retry button when the retry fails again, under a new alert', async () => {
    const { button, notice, answer } = failedWithFocus();
    const first = screen.getByRole('alert');
    await userEvent.keyboard('{Enter}');
    answer(1, 'unavailable');
    await waitFor(() => expect(button.textContent).toBe('Try again'));
    expect(notice.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);
    expect(button.getAttribute('aria-disabled')).toBe('false');
    expect(screen.getByRole('status').textContent).toBe('');
    // The words are a new alert, so the second failure is announced like the first.
    const second = screen.getByRole('alert');
    expect(second).not.toBe(first);
    expect(first.isConnected).toBe(false);
    expect(second.textContent).toContain('Opening: The service is temporarily unavailable.');
    // And it can be tried again from where focus is.
    await userEvent.keyboard('{Enter}');
    answer(2, 'content');
    const content = await screen.findByRole('heading', { name: 'The content' });
    await waitFor(() => expect(document.activeElement).toBe(content));
  });

  it('moves focus to the notice itself when the retry ends in a failure that allows none', async () => {
    const { button, notice, answer } = failedWithFocus();
    await userEvent.keyboard('{Enter}');
    answer(1, 'forbidden');
    await waitFor(() => expect(button.isConnected).toBe(false));
    expect(notice.isConnected).toBe(true);
    expect(screen.getByRole('alert').textContent).toContain(
      'Opening: You do not have permission to do this.',
    );
    expect(screen.queryByRole('button', { name: /Try|Trying/ })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(notice));
    // Script can focus it: without the attribute a browser ignores the call, and happy-dom does not.
    expect(notice.getAttribute('tabindex')).toBe('-1');
  });

  it('leaves focus the person moved elsewhere during the retry where it is', async () => {
    const { answer } = failedWithFocus();
    await userEvent.keyboard('{Enter}');
    const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
    act(() => elsewhere.focus());
    answer(1, 'content');
    await screen.findByRole('heading', { name: 'The content' });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(document.activeElement).toBe(elsewhere);
  });

  it('moves focus from the notice’s words to its retry button when the retry fails again', async () => {
    const { button, notice, answer } = failedWithFocus();
    await userEvent.keyboard('{Enter}');
    // The person clicks the words of the notice: focus is in the notice, not on its button.
    act(() => notice.focus());
    answer(1, 'unavailable');
    await waitFor(() => expect(button.textContent).toBe('Try again'));
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);
  });

  it.each([
    ['succeeded', 'content', 'The content'],
    ['failed again', 'unavailable', 'Try again'],
  ] as const)(
    'follows a retry that %s once: when the caller renders again, focus the person has moved stays where it is',
    async (_outcome, given, followedTo) => {
      const { answer } = failedWithFocus();
      await userEvent.keyboard('{Enter}');
      answer(1, given);
      await waitFor(() => expect(document.activeElement?.textContent).toBe(followedTo));
      // The person moves on, and each press renders the caller again.
      const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
      await userEvent.click(elsewhere);
      await userEvent.click(elsewhere);
      expect(elsewhere.getAttribute('data-presses')).toBe('2');
      expect(document.activeElement).toBe(elsewhere);
    },
  );

  it('does not show a retry in flight for another request as this one’s', async () => {
    const { asked, rerender, answer } = failedWithFocus('one');
    await userEvent.keyboard('{Enter}');
    expect(screen.getByRole('button', { name: 'Trying again…' })).toBeTruthy();
    // The component now asks something else, which is being asked for the first time.
    rerender(<Operation asked={asked} request="two" />);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByText('Asking…')).toBeTruthy();
    answer(1, 'internal');
    // The answer is this request's state now; a retry of it starts afresh.
    const button = await screen.findByRole('button', { name: 'Try again' });
    expect(button.getAttribute('aria-disabled')).toBe('false');
  });

  it('does not end the retry of what is asked now when an earlier request’s retry ends late', async () => {
    const { runs, button, end } = await retryOutlived();
    await userEvent.keyboard('{Enter}');
    expect(runs).toHaveLength(2);
    expect(button.textContent).toBe('Trying again…');
    // The retry of "one" ends: the retry of "two" is still in flight.
    await end(1);
    expect(button.textContent).toBe('Trying again…');
    expect(button.getAttribute('aria-disabled')).toBe('true');
    expect(screen.getByRole('status').textContent).toBe('Opening: trying again…');
    // It ends when its own run does.
    await end(2);
    expect(button.textContent).toBe('Try again');
  });

  it('does not follow an earlier request’s retry that ends late: focus in the notice of what is asked now stays where it is', async () => {
    const { button, notice, end } = await retryOutlived();
    // The person clicks the words of the notice: focus is in the notice, not on its button.
    act(() => notice.focus());
    await end(1);
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(notice);
  });
});

describe('a failure that changes without a retry', () => {
  it('moves focus to the content when the failure goes away by itself, as after a retry that succeeded, once', async () => {
    const { rerender, notice } = watchedWithFocus();
    rerender(<Watched status="loaded" />);
    expect(notice.isConnected).toBe(false);
    const content = screen.getByRole('heading', { name: 'The content' });
    expect(document.activeElement).toBe(content);
    // The person moves on, and each press renders the caller again: focus is not taken back.
    const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
    await userEvent.click(elsewhere);
    await userEvent.click(elsewhere);
    expect(elsewhere.getAttribute('data-presses')).toBe('2');
    expect(document.activeElement).toBe(elsewhere);
  });

  it('moves focus to the page’s level-one heading when the failure goes away and nothing is named in its place', () => {
    const { rerender, notice } = watchedWithFocus();
    // Asked again by something else: the failure is gone and nothing is loaded yet.
    rerender(<Watched status="asking" />);
    expect(notice.isConnected).toBe(false);
    expect(screen.queryByRole('heading', { name: 'The content' })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole('heading', { level: 1 }));
  });

  it('leaves focus that was not in the notice where it is when the failure goes away by itself', () => {
    const { rerender, notice } = watchedWithFocus();
    const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
    act(() => elsewhere.focus());
    rerender(<Watched status="loaded" />);
    expect(notice.isConnected).toBe(false);
    expect(screen.getByRole('heading', { name: 'The content' })).toBeTruthy();
    expect(document.activeElement).toBe(elsewhere);
  });

  it('keeps focus on the retry button when the failure shown is replaced by another that allows a retry, and moves it to the notice itself when by one that allows none', () => {
    const { rerender, button, notice } = watchedWithFocus();
    rerender(<Watched status="failed" code="unavailable" />);
    expect(screen.getByRole('alert').textContent).toContain(
      'Opening: The service is temporarily unavailable.',
    );
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);

    rerender(<Watched status="failed" code="forbidden" />);
    expect(screen.getByRole('alert').textContent).toContain(
      'Opening: You do not have permission to do this.',
    );
    expect(button.isConnected).toBe(false);
    expect(screen.queryByRole('button', { name: /Try|Trying/ })).toBeNull();
    // The same notice, which script can focus: the browser would ignore the call otherwise.
    expect(notice.isConnected).toBe(true);
    expect(notice.getAttribute('tabindex')).toBe('-1');
    expect(document.activeElement).toBe(notice);

    // Focus on the notice itself follows the failure off the page like focus on its button.
    rerender(<Watched status="loaded" />);
    expect(notice.isConnected).toBe(false);
    expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'The content' }));
  });

  it('moves focus to the retry button of the notice that replaces the one shown', () => {
    const { rerender, button, notice } = watchedWithFocus();
    rerender(<Watched status="failed" code="unavailable" notice="two" />);
    expect(notice.isConnected).toBe(false);
    expect(button.isConnected).toBe(false);
    const replacement = screen.getByRole('button', { name: 'Try again' });
    expect(document.activeElement).toBe(replacement);
  });

  it('does not take a notice that StrictMode mounts a second time for one that left the page', async () => {
    // In development StrictMode clears and sets again the refs of what has just mounted. The notice
    // that replaces the one shown takes focus in the commit it mounts in, so its refs are cleared
    // with focus in it, and nothing is removed.
    const view = render(
      <StrictMode>
        <Watched status="failed" />
      </StrictMode>,
    );
    act(() => screen.getByRole('button', { name: 'Try again' }).focus());
    view.rerender(
      <StrictMode>
        <Watched status="failed" code="unavailable" notice="two" />
      </StrictMode>,
    );
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Try again' }));
    // The person moves on, and the caller renders again: focus is not taken back to the button.
    const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
    await userEvent.click(elsewhere);
    expect(elsewhere.getAttribute('data-presses')).toBe('1');
    expect(document.activeElement).toBe(elsewhere);
  });
});
