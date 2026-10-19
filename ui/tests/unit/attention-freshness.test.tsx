/** Same-head rule upgrades: event lifecycle and explicit revisits refresh, idle time never polls. */
import { act, cleanup, configure, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { RECONNECT_MS } from '../../src/features/jobs/feed';
import { fail, mockApi, type Reply } from './support/api';
import {
  attentionAnswer,
  attentionService,
  attentionView,
  finding,
  instructionWarning,
  RULE_V1,
  RULE_V2,
} from './support/attention';
import { ATTENTION, eventOf, mockEvents, pageShown } from './support/imports';
import { explorerHandlers, HEAD, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const versionMessage = (version: string) => `Observed rule set ${version}.`;
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

describe('Attention rule freshness', { timeout: 30_000 }, () => {
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
