/**
 * The workspace head: shown "as of" when it was resolved, re-resolved on Refresh and on window
 * focus, and a revision is called "the latest" only when a request made for that statement
 * resolved it (SPEC section 3: "At::Latest is a request selector resolved once before the
 * operation reads").
 */

import { focusManager } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fail, type Json, mockApi, ok } from './support/api';
import {
  explorerHandlers,
  HEAD,
  NOTE,
  noteSummary,
  OLD,
  readResponse,
  renderAt,
  WS,
  workspace,
} from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const OPEN = '/api/workspaces/open-workspace';
const LIST = '/api/items/list-items';
const READ = '/api/reads/read-item';
const NEW = 'e'.repeat(40);

/** open_workspace answers HEAD the first time and NEW from then on. */
function headMovesOn() {
  let opens = 0;
  return () => {
    opens += 1;
    return ok({ ...workspace, head: opens === 1 ? HEAD : NEW });
  };
}

/** A read answered at the revision it asked for; `at: latest` resolves to NEW. */
function readAt(extra: (at: Json) => Json = () => ({})) {
  return (body: Json) => {
    const at = body.at as Json;
    const resolved = at.kind === 'latest' ? NEW : String(at.revision);
    const response = readResponse(noteSummary, `Text at ${resolved.slice(0, 1)}`, extra(at));
    return ok({ ...response, source: { ...response.source, revision: resolved } });
  };
}

describe('the "latest version" is the latest the server resolved for that request', () => {
  it('reads the latest version with a fresh at-latest request, never the cached head, and pins what it resolved (P6, adapted)', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn(), [READ]: readAt() }));
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Read the latest version' }));
    // The tab and the URL are pinned to the revision the server resolved.
    await waitFor(() => expect(router.state.location.search).toEqual({ rev: NEW }));
    const reads = api.callsTo(READ);
    const latest = reads.findIndex((body) => (body.at as Json).kind === 'latest');
    expect(latest).toBeGreaterThanOrEqual(0);
    for (const body of reads.slice(latest)) {
      expect(body.at).not.toEqual({ kind: 'revision', revision: HEAD });
    }
    // The revision shown is the one the response carried.
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await within(article).findByText('Text at e');
    expect(within(article).getAllByTitle(NEW).length).toBeGreaterThan(0);
    expect(within(article).queryAllByTitle(HEAD)).toEqual([]);
    // The answer to that one read is the pane's text: no second read of the same part.
    expect(reads.slice(latest + 1).filter((body) => body.cursor === null)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it("continues the latest read through that read's own cursor, at the revision it resolved", async () => {
    const api = mockApi(
      explorerHandlers({
        [OPEN]: headMovesOn(),
        [READ]: readAt((at) =>
          at.kind === 'latest' ? { truncated: true, next_cursor: 'latest-part-2' } : {},
        ),
      }),
    );
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Read the latest version' }));
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await userEvent.click(await within(article).findByRole('button', { name: 'Continue reading' }));
    await waitFor(() => expect(api.callsTo(READ).at(-1)?.cursor).toBe('latest-part-2'));
    expect(api.callsTo(READ).at(-1)?.at).toEqual({ kind: 'revision', revision: NEW });
  });

  it('compares a pinned revision with a head fetched for that comparison, not the cached one', async () => {
    // The shell resolved HEAD; by the time the reader asks, the head has moved on to NEW.
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn() }));
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    expect(await screen.findByText(/it is not the latest version/)).toBeTruthy();
    expect(api.callsTo(OPEN).length).toBeGreaterThanOrEqual(2);
  });

  it('says a pinned revision is the latest only as of when it was checked', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    const notice = await screen.findByText(/the latest version when checked at/);
    expect(notice.querySelector('time')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Read the latest version' })).toBeNull();
  });

  it('shows a failed latest read, and keeps the pinned revision', async () => {
    mockApi(
      explorerHandlers({
        [OPEN]: headMovesOn(),
        [READ]: (body) =>
          (body.at as Json).kind === 'latest'
            ? fail(500, 'internal', 'storage stumbled')
            : readAt()(body),
      }),
    );
    const { router } = renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await userEvent.click(await screen.findByRole('button', { name: 'Read the latest version' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Reading the latest version:');
    expect(router.state.location.search).toEqual({ rev: OLD });
  });
});

describe('the shell head is shown as of when it was resolved', () => {
  it('shows the head it lists "as of" an instant, and Refresh re-resolves it', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn() }));
    renderAt(`/w/${WS}`);
    const line = await screen.findByText(/Showing the workspace as of/);
    expect(line.querySelector('time')).toBeTruthy();
    expect(within(line).getByTitle(HEAD)).toBeTruthy();
    await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await screen.findByText(`Refreshed: now showing revision ${NEW.slice(0, 10)}.`);
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).querySelector('code')?.title).toBe(
        NEW,
      ),
    );
    // The tree lists the re-resolved head.
    await waitFor(() =>
      expect(api.callsTo(LIST).some((body) => (body.at as Json).revision === NEW)).toBe(true),
    );
  });

  it('re-resolves the head when the window regains focus', async () => {
    const api = mockApi(explorerHandlers({ [OPEN]: headMovesOn() }));
    renderAt(`/w/${WS}`);
    await screen.findByText(/Showing the workspace as of/);
    const before = api.callsTo(OPEN).length;
    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    await waitFor(() => expect(api.callsTo(OPEN).length).toBeGreaterThan(before));
    act(() => focusManager.setFocused(undefined));
  });

  it('keeps the Explorer when a refresh fails, and says so', async () => {
    let opens = 0;
    mockApi(
      explorerHandlers({
        [OPEN]: () => {
          opens += 1;
          return opens === 1 ? ok(workspace) : fail(500, 'internal', 'storage stumbled');
        },
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Refreshing this workspace:');
    expect(screen.getByRole('tree', { name: 'Folders' })).toBeTruthy();
    expect(
      await screen.findByText(
        'The workspace could not be refreshed; still showing the version above.',
      ),
    ).toBeTruthy();
  });
});
