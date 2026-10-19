/** Same-head rule upgrades: event lifecycle and explicit revisits refresh, idle time never polls. */
import { act, cleanup, configure, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { RECONNECT_MS } from '../../src/features/jobs/feed';
import { fail, mockApi, ok, type Reply } from './support/api';
import {
  attentionAnswer,
  attentionService,
  attentionView,
  finding,
  instructionWarning,
  RULE_V1,
  RULE_V2,
} from './support/attention';
import { ATTENTION, eventOf, JOBS, jobOf, mockEvents, pageShown } from './support/imports';
import { explorerHandlers, HEAD, NOTE, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const versionMessage = (version: string) => `Observed rule set ${version}.`;
/** A second resolved revision, distinct from the fixture workspace's head. */
const RULE_FIXTURE_REVISION = 'e'.repeat(40);
const answerWith = (version: string) =>
  attentionAnswer([
    {
      ...instructionWarning([finding(version)]),
      message: versionMessage(version),
    },
  ]);
const tick = async (ms: number) => {
  await act(() => vi.advanceTimersByTimeAsync(ms));
};

describe('Attention beside the other workspace subscribers', { timeout: 30_000 }, () => {
  it('shares Jobs events, removes only the Jobs subscription on leaving it, and closes on workspace unmount', async () => {
    const api = mockApi(
      explorerHandlers({
        [ATTENTION]: () => attentionAnswer([]),
        [JOBS]: () => ok({ items: [jobOf('succeeded')], next_cursor: null }),
      }),
    );
    const events = mockEvents();
    const view = renderAt(`/w/${WS}/jobs`);
    await screen.findByRole('heading', { name: 'Jobs' });
    await screen.findByText(/This list follows the workspace\u2019s events/);
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
    // One workspace has one transport, even though both features have their own invalidations.
    expect(events.streams).toHaveLength(1);
    const attentionBefore = api.callsTo(ATTENTION).length;
    const jobsBefore = api.callsTo(JOBS).length;
    act(() => events.streams.at(0)?.send('1', eventOf('imported', '1')));
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(attentionBefore + 1));
    await waitFor(() => expect(api.callsTo(JOBS)).toHaveLength(jobsBefore + 1));
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));

    fireEvent.click(screen.getByRole('link', { name: 'Documents' }));
    await waitFor(() => expect(screen.queryByRole('heading', { name: 'Jobs' })).toBeNull());
    expect(events.streams.at(0)?.closed).toBe(false);
    const attentionAfterLeaving = api.callsTo(ATTENTION).length;
    const jobsAfterLeaving = api.callsTo(JOBS).length;
    act(() => events.streams.at(0)?.send('2', eventOf('imported', '2')));
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(attentionAfterLeaving + 1));
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
    expect(api.callsTo(JOBS)).toHaveLength(jobsAfterLeaving);

    // A subscriber joining an already-live stream learns its state without opening another.
    fireEvent.click(screen.getByRole('link', { name: 'Jobs' }));
    await screen.findByRole('heading', { name: 'Jobs' });
    await screen.findByText(/This list follows the workspace\u2019s events/);
    expect(events.streams).toHaveLength(1);
    view.unmount();
    expect(events.streams.at(0)?.closed).toBe(true);
    expect(api.violations).toEqual([]);
  });

  it('shares the reader review stream while refreshing each feature for its own relevant event', async () => {
    const reviews = '/api/reviews/list-reviews';
    const api = mockApi(explorerHandlers({ [ATTENTION]: () => attentionAnswer([]) }));
    const events = mockEvents();
    const view = renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('article', { name: 'Meeting notes' });
    await waitFor(() => expect(api.callsTo(reviews).length).toBeGreaterThanOrEqual(2));
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
    expect(events.streams).toHaveLength(1);
    const attentionBefore = api.callsTo(ATTENTION).length;
    const reviewsBefore = api.callsTo(reviews).length;
    act(() => events.streams.at(0)?.send('3', eventOf('reviewed', '3', { item_id: NOTE })));
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(attentionBefore + 1));
    await waitFor(() => expect(api.callsTo(reviews)).toHaveLength(reviewsBefore + 1));
    expect(api.violations).toEqual([]);
  });

  it('reopens the shared stream when a reader is opened after signing in, without retrying an idle refusal', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const reviews = '/api/reviews/list-reviews';
    const api = mockApi(explorerHandlers({ [ATTENTION]: () => attentionAnswer([]) }));
    const events = mockEvents(fail(401, 'unauthenticated', 'no session'));
    const view = renderAt(`/w/${WS}`);
    await screen.findByRole('heading', { level: 1, name: 'Workshop notes' });
    await waitFor(() => expect(events.state.requests).toBe(1));
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
    await tick(600_000);
    expect(events.state.requests).toBe(1);
    expect(events.streams).toHaveLength(0);

    // Sign-in completes in this visible window; opening the reader is an explicit new visit.
    events.state.refusal = null;
    await act(async () => {
      await view.router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
      });
    });
    await screen.findByRole('article', { name: 'Meeting notes' });
    await waitFor(() => expect(api.callsTo(reviews).length).toBeGreaterThan(0));
    await waitFor(() => expect(events.state.requests).toBe(2));
    expect(events.streams).toHaveLength(1);
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
    const before = api.callsTo(reviews).length;
    act(() => events.streams.at(0)?.send('8', eventOf('reviewed', '8', { item_id: NOTE })));
    await waitFor(() => expect(api.callsTo(reviews)).toHaveLength(before + 1));
    expect(api.violations).toEqual([]);
  });

  it('does not share a workspace transport between separate QueryClients', async () => {
    const { api, events } = attentionService(() => attentionAnswer([]));
    const first = attentionView({ page: false, item: false });
    const second = attentionView({ page: false, item: false });
    await waitFor(() => expect(events.streams).toHaveLength(2));
    await waitFor(() => expect(first.client.isFetching() + second.client.isFetching()).toBe(0));
    first.unmount();
    expect(events.streams.filter((stream) => stream.closed)).toHaveLength(1);
    expect(events.streams.filter((stream) => !stream.closed)).toHaveLength(1);
    second.unmount();
    expect(events.streams.every((stream) => stream.closed)).toBe(true);
    expect(api.violations).toEqual([]);
  });

  it('the shared Explorer fixture answers Attention at the requested revision without adding a failure', async () => {
    const api = mockApi(explorerHandlers());
    mockEvents();
    const view = renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('article', { name: 'Meeting notes' });
    await waitFor(() => expect(api.callsTo(ATTENTION).length).toBeGreaterThan(0));
    await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
    expect(screen.queryByRole('alert')).toBeNull();
    const response = await fetch(new URL('/api/attention/get-attention', window.location.href), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        workspace_id: WS,
        at: { kind: 'revision', revision: RULE_FIXTURE_REVISION },
        page: { limit: 200, cursor: null },
      }),
    });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      revision: RULE_FIXTURE_REVISION,
      items: [],
      next_cursor: null,
    });
    expect(api.violations).toEqual([]);
  });
});

describe('Attention rule freshness', { timeout: 30_000 }, () => {
  it('cancels the first check when hidden before the initial event connection becomes live', async () => {
    const old = Promise.withResolvers<Reply>();
    const opening = Promise.withResolvers<void>();
    let first = true;
    const { api, events } = attentionService(() => {
      if (!first) return answerWith(RULE_V2);
      first = false;
      return old.promise;
    });
    events.state.hold = opening.promise;
    const view = attentionView();
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(1));
    await waitFor(() => expect(events.state.requests).toBe(1));
    act(() => pageShown(false));
    await act(async () => {
      old.resolve(answerWith(RULE_V1));
      await old.promise;
    });
    await waitFor(() => expect(view.client.isFetching()).toBe(0));
    // Even a first answer must not become current evidence after contact was lost.
    expect(screen.queryAllByText(versionMessage(RULE_V1))).toHaveLength(0);
    expect(api.callsTo(ATTENTION)).toHaveLength(1);

    events.state.hold = null;
    act(() => pageShown(true));
    await screen.findAllByText(versionMessage(RULE_V2));
    await act(async () => {
      opening.resolve();
      await opening.promise;
    });
    expect(screen.queryAllByText(versionMessage(RULE_V1))).toHaveLength(0);
    expect(api.violations).toEqual([]);
  });

  it('uses one lifecycle owner for both surfaces and replaces v1 after a same-head service reconnect', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let version = RULE_V1;
    const { api, events } = attentionService(() => answerWith(version));
    attentionView();
    await screen.findAllByText(versionMessage(RULE_V1));
    await waitFor(() => expect(events.streams).toHaveLength(1));
    const before = api.callsTo(ATTENTION).length;
    act(() => events.streams.at(0)?.send('7', eventOf('job_updated', '7')));
    await tick(50);
    const afterEvent = api.callsTo(ATTENTION).length;
    version = RULE_V2;
    act(() => events.streams.at(0)?.end());
    await tick(RECONNECT_MS + 50);
    await waitFor(() => expect(events.streams).toHaveLength(2));
    await screen.findAllByText(versionMessage(RULE_V2));
    expect(screen.getAllByText(versionMessage(RULE_V2))).toHaveLength(2);
    expect(screen.queryAllByText(versionMessage(RULE_V1))).toHaveLength(0);
    expect(api.callsTo(ATTENTION).length).toBe(afterEvent + 1);
    expect(afterEvent).toBeGreaterThanOrEqual(before);
    expect(events.streams.at(1)?.lastEventId).toBe('7');
    expect(
      api
        .callsTo(ATTENTION)
        .every(
          (body) =>
            JSON.stringify(body.at) === JSON.stringify({ kind: 'revision', revision: HEAD }),
        ),
    ).toBe(true);
    expect(api.violations).toEqual([]);
  });

  it('cancels an older first read before subscribing to the restarted service, so its late answer cannot win', async () => {
    let releaseOld: ((reply: Reply) => void) | undefined;
    let first = true;
    const { api, events } = attentionService(() => {
      if (!first) return answerWith(RULE_V2);
      first = false;
      return new Promise<Reply>((resolve) => {
        releaseOld = resolve;
      });
    });
    let open: (() => void) | undefined;
    events.state.hold = new Promise<void>((resolve) => {
      open = resolve;
    });
    attentionView();
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(1));
    act(() => open?.());
    await screen.findAllByText(versionMessage(RULE_V2));
    await act(async () => {
      releaseOld?.(answerWith(RULE_V1));
      await Promise.resolve();
    });
    expect(screen.queryAllByText(versionMessage(RULE_V1))).toHaveLength(0);
    expect(screen.getAllByText(versionMessage(RULE_V2))).toHaveLength(2);
    expect(api.callsTo(ATTENTION)).toHaveLength(2);
    expect(api.violations).toEqual([]);
  });

  for (const empty of [false, true]) {
    it(`labels retained ${empty ? 'empty checks' : 'findings'} as previous while refreshing and after failure on both surfaces`, async () => {
      let failRefresh = false;
      let release: ((reply: Reply) => void) | undefined;
      attentionService(() =>
        failRefresh
          ? new Promise<Reply>((resolve) => {
              release = resolve;
            })
          : empty
            ? attentionAnswer([])
            : answerWith(RULE_V1),
      );
      attentionView();
      if (empty) await screen.findByText('No warnings found.');
      else await screen.findAllByText(versionMessage(RULE_V1));
      failRefresh = true;
      fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
      await waitFor(() => expect(release).toBeTypeOf('function'));
      expect(screen.getAllByText(/showing (the )?previous (findings|check)/i)).toHaveLength(2);
      if (empty) expect(screen.queryByText('No warnings found.')).toBeNull();
      await act(async () => {
        release?.(fail(500, 'internal', 'rule refresh failed'));
      });
      await screen.findAllByText(/could not check with the current service/i);
      expect(screen.getAllByText(/showing (the )?previous (findings|check)/i)).toHaveLength(2);
      if (empty) expect(screen.getAllByText('Previous check found no warnings.')).toHaveLength(2);
      else expect(screen.getAllByText(versionMessage(RULE_V1))).toHaveLength(2);
    });
  }

  it('closes the stream while hidden and rechecks the same head on return without polling', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let version = RULE_V1;
    const { api, events } = attentionService(() => answerWith(version));
    attentionView();
    await screen.findAllByText(versionMessage(RULE_V1));
    await waitFor(() => expect(events.streams).toHaveLength(1));
    const before = api.callsTo(ATTENTION).length;
    act(() => pageShown(false));
    expect(events.streams.at(0)?.closed).toBe(true);
    await tick(600_000);
    expect(api.callsTo(ATTENTION)).toHaveLength(before);
    version = RULE_V2;
    act(() => pageShown(true));
    await screen.findAllByText(versionMessage(RULE_V2));
    expect(events.streams).toHaveLength(2);
    expect(api.callsTo(ATTENTION)).toHaveLength(before + 1);
  });

  it('rechecks on window return when SSE is unavailable, retaining no idle scan timer', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let version = RULE_V1;
    const { api, events } = attentionService(() => answerWith(version));
    events.state.refusal = fail(501, 'not_implemented', 'no event stream');
    attentionView();
    await screen.findAllByText(versionMessage(RULE_V1));
    await waitFor(() => expect(events.state.requests).toBe(1));
    const before = api.callsTo(ATTENTION).length;
    await tick(600_000);
    expect(api.callsTo(ATTENTION)).toHaveLength(before);
    version = RULE_V2;
    act(() => {
      window.dispatchEvent(new Event('blur'));
      window.dispatchEvent(new Event('focus'));
    });
    await screen.findAllByText(versionMessage(RULE_V2));
    expect(api.callsTo(ATTENTION)).toHaveLength(before + 1);
  });

  it('multiple observers share one query and ten minutes with a live stream adds zero scans', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { api, events } = attentionService();
    const view = attentionView();
    await screen.findAllByText('This text may contain instructions aimed at an AI assistant.');
    await waitFor(() => expect(events.streams).toHaveLength(1));
    const before = api.callsTo(ATTENTION).length;
    await tick(600_000);
    expect(api.callsTo(ATTENTION)).toHaveLength(before);
    expect(events.streams).toHaveLength(1);
    expect(
      view.client
        .getQueryCache()
        .getAll()
        .filter((query) => JSON.stringify(query.queryKey).includes('getAttention')),
    ).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('rechecks on opening Attention at the same head even when SSE is unavailable', async () => {
    let version = RULE_V1;
    const api = mockApi(explorerHandlers({ [ATTENTION]: () => answerWith(version) }));
    mockEvents(fail(501, 'not_implemented', 'no event stream'));
    renderAt(`/w/${WS}`);
    await screen.findByRole('link', { name: 'Attention (1)' });
    const before = api.callsTo(ATTENTION).length;
    version = RULE_V2;
    fireEvent.click(screen.getByRole('link', { name: 'Attention (1)' }));
    await screen.findByRole('heading', { name: 'Attention' });
    await screen.findByText(versionMessage(RULE_V2));
    expect(api.callsTo(ATTENTION)).toHaveLength(before + 1);
    expect(screen.queryByText(versionMessage(RULE_V1))).toBeNull();
    expect(api.violations).toEqual([]);
  });

  for (const transition of ['hidden', 'unmounted'] as const) {
    it(`does not resume a queued refresh after the lifecycle becomes ${transition}`, async () => {
      const { api, events } = attentionService(() => answerWith(RULE_V1));
      const view = attentionView();
      await screen.findAllByText(versionMessage(RULE_V1));
      await waitFor(() => expect(events.streams).toHaveLength(1));
      await waitFor(() => expect(view.client.isFetching()).toBe(0));
      const before = api.callsTo(ATTENTION).length;
      const queries = view.client.getQueryCache().getAll();
      expect(queries).toHaveLength(1);
      expect(queries.at(0)?.state.isInvalidated).toBe(false);

      // askAgain awaits cancellation before invalidating. Hold that exact boundary, so a
      // focus refresh is pending when the lifecycle changes; no sleep races a timer here.
      const cancellation = Promise.withResolvers<void>();
      const cancel = vi.spyOn(view.client, 'cancelQueries');
      cancel.mockImplementationOnce(() => cancellation.promise);
      act(() => window.dispatchEvent(new Event('focus')));
      await waitFor(() => expect(cancel).toHaveBeenCalledTimes(1));
      if (transition === 'hidden') act(() => pageShown(false));
      else view.unmount();
      expect(events.streams.at(0)?.closed).toBe(true);
      await act(async () => {
        cancellation.resolve();
        await cancellation.promise;
      });

      expect(api.callsTo(ATTENTION)).toHaveLength(before);
      if (transition === 'unmounted') {
        // With no active observer an obsolete invalidation may not issue a network request,
        // but must still not mark this previous workspace's settled cache uncertain.
        expect(queries.at(0)?.state.isInvalidated).toBe(false);
      }
      expect(api.violations).toEqual([]);
    });
  }
});
