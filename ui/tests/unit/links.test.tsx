/**
 * An item's links beside it: what it links to and what links to it (its backlinks), at the
 * revision the tab reads, bounded by pages (SPEC section 7: "Links/backlinks and a bounded graph
 * derive from the same source state").
 */
import { cleanup, configure, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Link } from '../../src/api/generated/types.gen';
import { LINKS_PAGE } from '../../src/features/graph/ItemLinks';
import { type Handler, mockApi, ok } from './support/api';
import { LINKS } from './support/reviews';
import { explorerHandlers, HEAD, NOTE, OLD, OTHER, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const SOMEONE = '88888888-8888-4888-8888-888888888888';

const LINKED: Link[] = [
  {
    from: NOTE,
    from_path: 'notes/meeting.md',
    to: OTHER,
    to_path: 'notes/plan.md',
    label: 'the plan',
    line: 4,
  },
  {
    from: NOTE,
    from_path: 'notes/meeting.md',
    to_path: 'notes/gone.md',
    label: 'old minutes',
    line: 7,
  },
  {
    from: SOMEONE,
    from_path: 'elsewhere/someone.md',
    to: NOTE,
    to_path: 'notes/meeting.md',
    label: 'see the meeting',
    line: 12,
  },
];

function setup(links: Handler) {
  return mockApi(explorerHandlers({ [LINKS]: links }));
}

async function linksPanel(url = `/w/${WS}/i/${NOTE}`) {
  renderAt(url);
  return screen.findByRole('region', { name: 'Links' });
}

describe('an item’s links', { timeout: 30_000 }, () => {
  it('asks both ways at the revision read and lists what it links to and what links to it', async () => {
    const api = setup(() => ok({ revision: OLD, links: LINKED, next_cursor: null }));
    const panel = await linksPanel(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const from = await within(panel).findByRole('list', { name: 'From this item' });
    const to = within(panel).getByRole('list', { name: 'To this item' });
    expect(api.callsTo(LINKS)).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        at: { kind: 'revision', revision: OLD },
        direction: 'both',
        page: { limit: LINKS_PAGE },
      },
    ]);
    const plan = within(from).getByRole('link', { name: 'the plan' });
    // Pinned to the revision the links were read at.
    expect(plan.getAttribute('href')).toBe(`/w/${WS}/i/${OTHER}?rev=${OLD}`);
    expect(within(from).getAllByRole('listitem').at(0)?.textContent).toContain('notes/plan.md');
    // A link that leads nowhere is said to, and offers nothing to follow.
    const broken = within(from).getAllByRole('listitem').at(1);
    expect(broken?.textContent).toContain('old minutes');
    expect(broken?.textContent).toContain('leads nowhere');
    expect(broken?.querySelector('a')).toBeNull();
    // A backlink opens the linking item at the line that links here.
    const back = within(to).getByRole('link', { name: 'see the meeting' });
    expect(back.getAttribute('href')).toContain(`/w/${WS}/i/${SOMEONE}?`);
    expect(back.getAttribute('href')).toContain(`rev=${OLD}`);
    expect(decodeURIComponent(back.getAttribute('href') ?? '')).toContain('"start":12');
    expect(within(to).getAllByRole('listitem').at(0)?.textContent).toContain('line 12');
    expect(api.violations).toEqual([]);
  });

  it('says when nothing links either way', async () => {
    setup(() => ok({ revision: HEAD, links: [], next_cursor: null }));
    const panel = await linksPanel();
    await within(panel).findByText('This item links to nothing, and nothing links to it.');
  });

  it('asks for more links from the cursor, from the revision its first page read', async () => {
    const api = setup((body) => {
      const page = body.page as { cursor?: string | null };
      return page.cursor === 'next'
        ? ok({ revision: OLD, links: LINKED.slice(2), next_cursor: null })
        : ok({ revision: OLD, links: LINKED.slice(0, 2), next_cursor: 'next' });
    });
    const panel = await linksPanel();
    await within(panel).findByRole('link', { name: 'the plan' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Show more links' }));
    await within(panel).findByRole('link', { name: 'see the meeting' });
    expect(api.callsTo(LINKS).at(1)).toMatchObject({
      at: { kind: 'revision', revision: HEAD },
      page: { limit: LINKS_PAGE, cursor: 'next' },
    });
    await waitFor(() =>
      expect(within(panel).queryByRole('button', { name: 'Show more links' })).toBeNull(),
    );
  });
});
