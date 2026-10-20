// @vitest-environment-options {"settings":{"navigation":{"disableChildFrameNavigation":true}}}
/**
 * The HTML sandbox viewer (UI M4a, ruling A3): an HTML original framed from the sandbox origin
 * through a capability `create_sandbox_capability` mints just before framing, in an
 * `<iframe sandbox="">` with no token and no referrer, its `src` exactly the capability's `url`.
 * Reloading mints a new capability, after expiry or at the person's word; nothing is framed for
 * another media type; failures read in words with their retry; the capability stays in the
 * component's state, out of the query cache and the browser's storage.
 *
 * The capability's `url` is never fetched here: happy-dom is told not to load a frame's page (the
 * environment options above; it would otherwise reach for the sandbox origin over the network),
 * and the service's sandbox origin is a separate transport (`serve_sandbox_representation`),
 * which the browser journey (`e2e/m4a-sandbox.spec.ts`) plays.
 */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HtmlSandbox } from '../../src/features/viewers/html/HtmlSandbox';
import { OriginalViewer } from '../../src/features/viewers/OriginalViewer';
import { fail, mockApi, notImplemented, ok, type Reply } from './support/api';
import {
  capability,
  HTML_OBJECT,
  htmlAppearance,
  htmlProps,
  renderViewer,
  SANDBOX_ORIGIN,
  watchSandboxAttributes,
} from './support/m4a-sandbox';
import { CREATE_SANDBOX_CAPABILITY } from './support/reader';
import { HEAD, OLD, SOURCE, WS } from './support/workspace';

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** The service mints a new capability for each request, numbered in order, living `lifetimeMs`. */
function minting(lifetimeMs = 300_000) {
  let minted = 0;
  return () => {
    minted += 1;
    return ok(capability(minted, lifetimeMs));
  };
}

/** One viewer whose revision the reader changes: an older one first, then the head. */
function Revisions() {
  const [revision, setRevision] = useState(OLD);
  return (
    <>
      <button type="button" onClick={() => setRevision(HEAD)}>
        Read the head
      </button>
      <button type="button" onClick={() => setRevision(OLD)}>
        Read the old revision
      </button>
      <HtmlSandbox {...htmlProps({ revision })} />
    </>
  );
}

/** The frame on the page, which must be there. */
async function frameShown(): Promise<HTMLIFrameElement> {
  const frame = await screen.findByTitle(/in the sandbox$/);
  if (!(frame instanceof HTMLIFrameElement)) throw new Error('the frame is not an iframe');
  return frame;
}

describe('the HTML sandbox viewer (A3)', () => {
  it('frames the capability it mints: an empty sandbox, no referrer, its url exactly', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    const frame = await frameShown();
    expect(frame.hasAttribute('sandbox')).toBe(true);
    expect(frame.getAttribute('sandbox')).toBe('');
    expect(frame.getAttribute('referrerpolicy')).toBe('no-referrer');
    expect(frame.getAttribute('src')).toBe(capability(1).url);
    expect(frame.getAttribute('title')).toBe('welcome.html, as written, in the sandbox');
    // The capability is bound to the exact object of the exact revision the reader shows.
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toEqual([
      { workspace_id: WS, item_id: SOURCE, revision: HEAD, object: HTML_OBJECT },
    ]);
    expect(api.violations).toEqual([]);
  });

  it('says that scripts in the file do not run, and that its links and forms do nothing', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    await frameShown();
    const note = screen.getByRole('note');
    expect(note.textContent).toContain('Scripts in it do not run');
    expect(note.textContent).toContain('its links and forms do nothing');
    expect(note.textContent).toContain('a sandbox on a separate origin');
    expect(api.violations).toEqual([]);
  });

  it('never sets an allow-* token, through minting, expiry, a reload and a failure', async () => {
    let minted = 0;
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: () => {
        minted += 1;
        // The second request fails; the first has already expired; the third frames.
        if (minted === 2) return fail(503, 'unavailable', 'The sandbox is restarting');
        return ok(capability(minted, minted === 1 ? -1_000 : 300_000));
      },
    });
    const watched = watchSandboxAttributes(document.body);
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    await frameShown();
    await screen.findByText(/expired at/);
    await userEvent.click(screen.getByRole('button', { name: 'Reload the page' }));
    await screen.findByText(/Opening the page in the sandbox: The service is temporarily/);
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(async () =>
      expect((await frameShown()).getAttribute('src')).toBe(capability(3).url),
    );
    watched.stop();
    expect(watched.seen.length).toBeGreaterThan(0);
    // Present, and empty, every time a frame was there.
    expect(new Set(watched.seen)).toEqual(new Set(['']));
    for (const frame of document.querySelectorAll('iframe')) {
      expect(frame.getAttribute('sandbox')).toBe('');
      expect(frame.sandbox.length).toBe(0);
    }
    expect(api.violations).toEqual([]);
  });

  it('mints a new capability when the frame is reloaded after its capability expired', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true, toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting(60_000) });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    const first = await frameShown();
    const firstUrl = first.getAttribute('src');
    expect(screen.queryByText(/expired at/)).toBeNull();
    expect(screen.getByText(/good until/)).toBeTruthy();

    // Past `expires_at` the page says so; the frame still shows what it loaded.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(61_000);
    });
    const status = await screen.findByText(/expired at/);
    expect(status.closest('[role="status"]')).not.toBeNull();
    expect(status.textContent).toContain('reloading asks for a new one');
    expect((await frameShown()).getAttribute('src')).toBe(firstUrl);
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(1);

    await user.click(screen.getByRole('button', { name: 'Reload the page' }));
    await waitFor(async () => expect((await frameShown()).getAttribute('src')).not.toBe(firstUrl));
    const second = await frameShown();
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(2);
    expect(second.getAttribute('src')).toBe(`${SANDBOX_ORIGIN}${new URL(second.src).pathname}`);
    expect(second.getAttribute('src')).toContain('/sandbox/token-2-');
    expect(screen.queryByText(/expired at/)).toBeNull();
    // A new frame, not the old one sent elsewhere: no entry of the old address is left behind it.
    expect(second).not.toBe(first);
    expect(api.violations).toEqual([]);
  });

  it('frames an answer that has already expired, and says so, until a reload mints anew', async () => {
    let minted = 0;
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: () => {
        minted += 1;
        return ok(capability(minted, minted === 1 ? -1_000 : 300_000));
      },
    });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    expect((await frameShown()).getAttribute('src')).toBe(capability(1).url);
    await screen.findByText(/expired at/);
    await userEvent.click(screen.getByRole('button', { name: 'Reload the page' }));
    await waitFor(async () =>
      expect((await frameShown()).getAttribute('src')).toBe(capability(2).url),
    );
    expect(screen.queryByText(/expired at/)).toBeNull();
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(2);
    expect(api.violations).toEqual([]);
  });

  it('mints a new capability for a reload the person asks for, never reusing the framed one', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    expect((await frameShown()).getAttribute('src')).toBe(capability(1).url);
    const reload = screen.getByRole('button', { name: 'Reload the page' });
    await userEvent.click(reload);
    await waitFor(async () =>
      expect((await frameShown()).getAttribute('src')).toBe(capability(2).url),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Reload the page' }));
    await waitFor(async () =>
      expect((await frameShown()).getAttribute('src')).toBe(capability(3).url),
    );
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(3);
    // Keyboard focus stays on the button through each reload.
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Reload the page' }));
    expect(api.violations).toEqual([]);
  });

  it('keeps the button, with focus, while a reload is minting: aria-disabled, one request', async () => {
    let answer: (reply: Reply) => void = () => undefined;
    let minted = 0;
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: () => {
        minted += 1;
        if (minted === 1) return ok(capability(1));
        return new Promise<Reply>((resolve) => {
          answer = resolve;
        });
      },
    });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    await frameShown();
    await userEvent.click(screen.getByRole('button', { name: 'Reload the page' }));
    const busy = await screen.findByRole('button', { name: 'Reloading the page…' });
    expect(busy.getAttribute('aria-disabled')).toBe('true');
    expect(busy.hasAttribute('disabled')).toBe(false);
    expect(document.activeElement).toBe(busy);
    await userEvent.click(busy);
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(2);
    // The frame already shown stays while the new address is asked for.
    expect((await frameShown()).getAttribute('src')).toBe(capability(1).url);
    answer(ok(capability(2)));
    await waitFor(async () =>
      expect((await frameShown()).getAttribute('src')).toBe(capability(2).url),
    );
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Reload the page' }));
    expect(api.violations).toEqual([]);
  });

  it('keeps the capability out of the query cache and the browser’s storage', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    const { queryClient } = await renderViewer(<HtmlSandbox {...htmlProps()} />);
    const url = capability(1).url;
    expect((await frameShown()).getAttribute('src')).toBe(url);
    const token = new URL(url).pathname.split('/').pop() ?? '';
    expect(token.length).toBeGreaterThan(0);
    const cached = JSON.stringify([
      queryClient
        .getQueryCache()
        .getAll()
        .map((query) => query.state),
      queryClient
        .getMutationCache()
        .getAll()
        .map((mutation) => mutation.state),
    ]);
    expect(cached).not.toContain(token);
    const stored = JSON.stringify([{ ...localStorage }, { ...sessionStorage }]);
    expect(stored).not.toContain(token);
    expect(api.violations).toEqual([]);
  });

  it('frames nothing for another media type, and asks for no capability', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    for (const mediaType of [
      'application/xhtml+xml',
      'image/svg+xml',
      'text/plain',
      'text/htmlx',
    ]) {
      const { unmount } = await renderViewer(
        <HtmlSandbox {...htmlProps({ appearance: htmlAppearance(mediaType) })} />,
      );
      const said = await screen.findByText(/nothing is framed here/);
      expect(said.textContent).toContain(mediaType);
      expect(document.querySelector('iframe')).toBeNull();
      unmount();
    }
    // Through the viewer an original goes to, XHTML is not framed either.
    const { unmount } = await renderViewer(
      <OriginalViewer {...htmlProps({ appearance: htmlAppearance('application/xhtml+xml') })} />,
    );
    expect(document.querySelector('iframe')).toBeNull();
    unmount();
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('frames text/html whatever its parameters and case, through the viewer an original goes to', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    await renderViewer(
      <OriginalViewer {...htmlProps({ appearance: htmlAppearance('Text/HTML; charset=utf-8') })} />,
    );
    expect((await frameShown()).getAttribute('src')).toBe(capability(1).url);
    expect(api.violations).toEqual([]);
  });

  it('shows a failure in words, keeps focus on its retry, and frames what the retry mints', async () => {
    let minted = 0;
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: () => {
        minted += 1;
        return minted === 1
          ? fail(503, 'unavailable', 'The sandbox origin is not answering')
          : ok(capability(minted));
      },
    });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain(
      'Opening the page in the sandbox: The service is temporarily unavailable.',
    );
    expect(alert.textContent).toContain('The sandbox origin is not answering');
    expect(document.querySelector('iframe')).toBeNull();
    const retry = screen.getByRole('button', { name: 'Try again' });
    retry.focus();
    await userEvent.click(retry);
    expect((await frameShown()).getAttribute('src')).toBe(capability(2).url);
    expect(screen.queryByRole('alert')).toBeNull();
    // Focus that was on the retry goes to what loaded in the failure's place.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Reload the page' })),
    );
    expect(api.violations).toEqual([]);
  });

  it('says “Not available in this build.” when the service has no sandbox, and offers no retry', async () => {
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: () => notImplemented('create_sandbox_capability'),
    });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain(
      'Opening the page in the sandbox: Not available in this build.',
    );
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(document.querySelector('iframe')).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('frames nothing the service answers on this page’s own origin or outside http', async () => {
    for (const url of [`${window.location.origin}/sandbox/token`, 'javascript:alert(1)']) {
      const api = mockApi({
        [CREATE_SANDBOX_CAPABILITY]: () =>
          ok({ url, expires_at: new Date(Date.now() + 300_000).toISOString() }),
      });
      const { unmount } = await renderViewer(<HtmlSandbox {...htmlProps()} />);
      const alert = await screen.findByRole('alert');
      expect(alert.textContent).toContain(
        'Opening the page in the sandbox: The service answered an address this page does not frame.',
      );
      expect(document.querySelector('iframe')).toBeNull();
      expect(api.violations).toEqual([]);
      unmount();
      vi.unstubAllGlobals();
    }
  });

  it('mints anew for another revision, and an answer for the old one frames nothing', async () => {
    let answerOld: (reply: Reply) => void = () => undefined;
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: (body) =>
        body.revision === OLD
          ? new Promise<Reply>((resolve) => {
              answerOld = resolve;
            })
          : ok(capability(2)),
    });
    await renderViewer(<Revisions />);
    await waitFor(() => expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(1));
    // The same viewer is given another revision while the first capability is asked for.
    await userEvent.click(screen.getByRole('button', { name: 'Read the head' }));
    expect((await frameShown()).getAttribute('src')).toBe(capability(2).url);
    answerOld(ok(capability(1)));
    await act(async () => {
      await Promise.resolve();
    });
    const frames = [...document.querySelectorAll('iframe')].map((frame) =>
      frame.getAttribute('src'),
    );
    expect(frames).toEqual([capability(2).url]);
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY).map((body) => body.revision)).toEqual([
      OLD,
      HEAD,
    ]);
    expect(api.violations).toEqual([]);
  });

  it('frames the original only: a capability for the item’s converted text is never asked', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    await frameShown();
    const asked = api.callsTo(CREATE_SANDBOX_CAPABILITY).map((body) => body.object);
    expect(asked).toEqual([HTML_OBJECT]);
    expect(asked).not.toContain(htmlAppearance().extraction.digest);
    const region = screen.getByRole('region', { name: 'welcome.html in the sandbox' });
    expect(within(region).getAllByTitle(/in the sandbox$/)).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('does not reuse an earlier capability when returning to its revision', async () => {
    // A3: mint just before framing, even when this binding was shown earlier. The expected
    // addresses are independently numbered service answers, never derived from the viewer.
    let answerReturn: (reply: Reply) => void = () => undefined;
    let requests = 0;
    const api = mockApi({
      [CREATE_SANDBOX_CAPABILITY]: () => {
        requests += 1;
        if (requests === 1) return ok(capability(1));
        return new Promise<Reply>((resolve) => {
          if (requests === 3) answerReturn = resolve;
        });
      },
    });
    await renderViewer(<Revisions />);
    expect((await frameShown()).getAttribute('src')).toBe(capability(1).url);
    await userEvent.click(screen.getByRole('button', { name: 'Read the head' }));
    await waitFor(() => expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(2));
    expect(document.querySelector('iframe')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Read the old revision' }));
    await waitFor(() => expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(3));
    expect(document.querySelector('iframe')).toBeNull();
    answerReturn(ok(capability(3)));
    expect((await frameShown()).getAttribute('src')).toBe(capability(3).url);
    expect(api.violations).toEqual([]);
  });

  it('mints nothing when the frame loads again by itself: only the person’s reload does', async () => {
    const api = mockApi({ [CREATE_SANDBOX_CAPABILITY]: minting() });
    await renderViewer(<HtmlSandbox {...htmlProps()} />);
    const frame = await frameShown();
    // A file that reloads itself, or a link followed inside the frame, loads the frame again.
    for (let times = 0; times < 5; times += 1) fireEvent.load(frame);
    await act(async () => {
      await Promise.resolve();
    });
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(1);
    expect((await frameShown()).getAttribute('src')).toBe(capability(1).url);
    await userEvent.click(screen.getByRole('button', { name: 'Reload the page' }));
    await waitFor(async () =>
      expect((await frameShown()).getAttribute('src')).toBe(capability(2).url),
    );
    expect(api.callsTo(CREATE_SANDBOX_CAPABILITY)).toHaveLength(2);
    expect(api.violations).toEqual([]);
  });
});
