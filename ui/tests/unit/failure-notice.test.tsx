/** Every ApiError code reads as plain language with the action it allows; NotImplemented is never success. */

import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { ErrorCode } from '../../src/api/generated/types.gen';
import { zErrorCode } from '../../src/api/generated/zod.gen';
import { FailureNotice } from '../../src/features/errors/FailureNotice';
import { classifyFailure, messageFor, shouldRetry } from '../../src/features/errors/failure';

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
    const onRetry = vi.fn();
    await renderNotice(
      <FailureNotice
        context="Reading this item"
        error={apiError('not_implemented', { message: 'read_item is not implemented yet' })}
        onRetry={onRetry}
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
    const onRetry = vi.fn();
    await renderNotice(
      <FailureNotice context="Opening" error={apiError(code)} onRetry={onRetry} />,
    );
    expect(screen.getByRole('alert').textContent).toContain(`Opening: ${title}`);
    expect(screen.getByRole('alert').textContent).toContain(`server words for ${code}`);
    if (button === null) {
      expect(screen.queryByRole('button')).toBeNull();
    } else {
      await userEvent.click(screen.getByRole('button', { name: button }));
      expect(onRetry).toHaveBeenCalledTimes(1);
    }
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
    const onRetry = vi.fn();
    render(
      <FailureNotice context="Listing" error={new TypeError('fetch failed')} onRetry={onRetry} />,
    );
    expect(screen.getByRole('alert').textContent).toContain(
      'Listing: The workspace service could not be reached.',
    );
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(onRetry).toHaveBeenCalledOnce();
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
