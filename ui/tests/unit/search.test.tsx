/** Search: the query lives in the URL, results open items at their cited selection, and paging follows the cursor. */

import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SearchHit } from '../../src/api/generated/types.gen';
import { mockApi, notImplemented, ok } from './support/api';
import { explorerHandlers, HEAD, NOTE, OTHER, renderAt, WS } from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const SEARCH = '/api/search/search-items';
const READ = '/api/reads/read-item';

function hit(itemId: string, title: string, path: string, start: number): SearchHit {
  return {
    title,
    snippet: `…matching words in ${title}…`,
    score: 1,
    source: {
      workspace_id: WS,
      item_id: itemId,
      path,
      revision: HEAD,
      selection: { kind: 'lines', range: { start, end: start + 1 } },
      locations: [],
    },
  };
}

describe('search', () => {
  it('runs the search operation pinned to the head and keeps the query in the URL', async () => {
    const api = mockApi(
      explorerHandlers({
        [SEARCH]: () =>
          ok({ hits: [hit(NOTE, 'Meeting notes', 'notes/meeting.md', 4)], revision: HEAD }),
      }),
    );
    const { router } = renderAt(`/w/${WS}`);
    const input = await screen.findByRole('searchbox', { name: 'Search this workspace' });
    await userEvent.type(input, 'budget{Enter}');
    await waitFor(() => expect(router.state.location.search).toEqual({ q: 'budget' }));
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    expect(api.callsTo(SEARCH)).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'revision', revision: HEAD },
        query: 'budget',
        include_archived: false,
        page: { limit: 20, cursor: null },
      },
    ]);
    const link = await within(results).findByRole('link', { name: /Meeting notes/ });
    expect(link.textContent).toContain('notes/meeting.md, lines 4–5');
    expect(link.textContent).toContain('…matching words in Meeting notes…');
    expect(api.violations).toEqual([]);
  });

  it('opens a result at its exact selection, and the URL reopens it', async () => {
    const api = mockApi(
      explorerHandlers({
        [SEARCH]: () =>
          ok({ hits: [hit(NOTE, 'Meeting notes', 'notes/meeting.md', 4)], revision: HEAD }),
      }),
    );
    const { router } = renderAt(`/w/${WS}?q=budget`);
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    await userEvent.click(await within(results).findByRole('link', { name: /Meeting notes/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(router.state.location.search).toEqual({
      q: 'budget',
      sel: { kind: 'lines', range: { start: 4, end: 5 } },
    });
    expect(await screen.findByText('Showing lines 4–5.', { exact: false })).toBeTruthy();
    await waitFor(() =>
      expect(api.callsTo(READ).at(-1)?.selection).toEqual({
        kind: 'lines',
        range: { start: 4, end: 5 },
      }),
    );
  });

  it('pages through results with the operation cursor', async () => {
    const api = mockApi(
      explorerHandlers({
        [SEARCH]: (body) => {
          const page = body.page as { cursor: string | null };
          return page.cursor === 'more'
            ? ok({ hits: [hit(OTHER, 'Plan', 'notes/plan.md', 1)], revision: HEAD })
            : ok({
                hits: [hit(NOTE, 'Meeting notes', 'notes/meeting.md', 4)],
                revision: HEAD,
                next_cursor: 'more',
              });
        },
      }),
    );
    renderAt(`/w/${WS}?q=budget`);
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    expect(await within(results).findByText('1 result shown; more are available.')).toBeTruthy();
    await userEvent.click(within(results).getByRole('button', { name: 'Show more results' }));
    expect(await within(results).findByRole('link', { name: /Plan/ })).toBeTruthy();
    expect(within(results).getByRole('link', { name: /Meeting notes/ })).toBeTruthy();
    expect(within(results).queryByRole('button', { name: 'Show more results' })).toBeNull();
    const cursors = api.callsTo(SEARCH).map((body) => (body.page as { cursor: unknown }).cursor);
    expect(cursors).toEqual([null, 'more']);
  });

  it('says there are no matches, and goes back to the folders', async () => {
    mockApi(explorerHandlers({ [SEARCH]: () => ok({ hits: [], revision: HEAD }) }));
    const { router } = renderAt(`/w/${WS}?q=nothing`);
    expect(await screen.findByText('No matches for “nothing”.')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Back to folders' }));
    await waitFor(() => expect(router.state.location.search).toEqual({}));
    expect(await screen.findByRole('tree', { name: 'Folders' })).toBeTruthy();
  });

  it('labels a search snippet taken from text an agent supplied (P7)', async () => {
    mockApi(
      explorerHandlers({
        [SEARCH]: () =>
          ok({
            hits: [
              {
                ...hit(NOTE, 'Meeting notes', 'notes/meeting.md', 4),
                extraction: { status: 'partial', text_origin: 'supplied_by_agent', corrected: false },
              },
            ],
            revision: HEAD,
          }),
      }),
    );
    renderAt(`/w/${WS}?q=budget`);
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    const link = await within(results).findByRole('link', { name: /Meeting notes/ });
    expect(link.textContent).toMatch(/supplied by an agent/i);
  });

  it('labels a result exactly as the reader does: partial, agent-supplied, corrected', async () => {
    mockApi(
      explorerHandlers({
        [SEARCH]: () =>
          ok({
            hits: [
              {
                ...hit(NOTE, 'Meeting notes', 'notes/meeting.md', 4),
                extraction: { status: 'partial', text_origin: 'supplied_by_agent', corrected: true },
              },
              {
                ...hit(OTHER, 'Plan', 'notes/plan.md', 1),
                extraction: { status: 'completed', text_origin: 'converter', corrected: false },
              },
            ],
            revision: HEAD,
          }),
      }),
    );
    renderAt(`/w/${WS}?q=budget`);
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    const labelled = await within(results).findByRole('link', { name: /Meeting notes/ });
    expect(labelled.textContent).toContain('Partly converted.');
    expect(labelled.textContent).toContain(
      'The text shown was supplied by an agent and accepted by a person; it is not the converter’s.',
    );
    expect(labelled.textContent).toContain(
      'A person’s correction is shown over the converted text.',
    );
    const plain = within(results).getByRole('link', { name: /Plan/ });
    expect(plain.textContent).not.toMatch(/converted|supplied|correction/);
  });

  it('shows an unbuilt search as not available, never as no matches', async () => {
    mockApi(explorerHandlers({ [SEARCH]: () => notImplemented('search_items') }));
    renderAt(`/w/${WS}?q=budget`);
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    const alert = await within(results).findByRole('alert');
    expect(alert.textContent).toContain('Searching: Not available in this build.');
    expect(within(results).queryByText(/No matches/)).toBeNull();
  });
});
