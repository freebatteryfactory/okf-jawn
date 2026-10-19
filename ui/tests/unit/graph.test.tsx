/**
 * The graph page: the bounded graph the service answers (`get_graph`), drawn, and the same items
 * and links as a table and a list that a keyboard and a screen reader can use (SPEC section 7:
 * "Links/backlinks and a bounded graph derive from the same source state"; ruling R8: the graph
 * shows what the operation answers and says when it was cut short, never a walk of the workspace).
 */
import { cleanup, configure, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { GetGraphResponse } from '../../src/api/generated/types.gen';
import { GRAPH_MAX_NODES, graphElements } from '../../src/features/graph/graph';
import { fail, type Handler, mockApi, ok } from './support/api';
import { GRAPH } from './support/reviews';
import {
  explorerHandlers,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  renderAt,
  SOURCE,
  sourceSummary,
  WS,
} from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const OUTSIDE = '88888888-8888-4888-8888-888888888888';

const GRAPH_ANSWER: GetGraphResponse = {
  revision: OLD,
  nodes: [noteSummary, otherSummary, sourceSummary],
  edges: [
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
      to: SOURCE,
      to_path: 'report.pdf.md',
      label: 'the report',
      line: 6,
    },
    {
      from: OTHER,
      from_path: 'notes/plan.md',
      to_path: 'notes/gone.md',
      label: 'old minutes',
      line: 2,
    },
    {
      from: OTHER,
      from_path: 'notes/plan.md',
      to: OUTSIDE,
      to_path: 'elsewhere/far.md',
      label: 'far away',
      line: 9,
    },
  ],
  truncated: false,
};

function setup(graph: Handler) {
  return mockApi(explorerHandlers({ [GRAPH]: graph }));
}

async function openGraph() {
  renderAt(`/w/${WS}/graph`);
  return screen.findByRole('region', { name: 'Graph' });
}

describe('the graph page', { timeout: 30_000 }, () => {
  it('is in the workspace’s bar, asks one bounded graph at the head shown, and asks nothing else of the items', async () => {
    const api = setup(() => ok(GRAPH_ANSWER));
    renderAt(`/w/${WS}`);
    fireEvent.click(await screen.findByRole('link', { name: 'Graph' }));
    const page = await screen.findByRole('region', { name: 'Graph' });
    await within(page).findByRole('table', { name: 'Items in the graph' });
    expect(api.callsTo(GRAPH)).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'revision', revision: 'a'.repeat(40) },
        max_nodes: GRAPH_MAX_NODES,
      },
    ]);
    // Never a walk of the workspace from the page: no read of an item, no item's links.
    expect(api.callsTo('/api/search/get-links')).toEqual([]);
    expect(api.callsTo('/api/reads/read-item')).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('lists every item it answers, with its links out and in, each opening at the revision the graph read', async () => {
    setup(() => ok(GRAPH_ANSWER));
    const page = await openGraph();
    const table = await within(page).findByRole('table', { name: 'Items in the graph' });
    const rows = within(table).getAllByRole('row').slice(1);
    expect(rows).toHaveLength(3);
    const note = within(table).getByRole('link', { name: 'Meeting notes' });
    expect(note.getAttribute('href')).toBe(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const noteRow = note.closest('tr');
    expect(noteRow?.textContent).toContain('notes/meeting.md');
    // Meeting notes links to two items, and none links to it.
    const cells = [...(noteRow?.querySelectorAll('td') ?? [])].map((cell) => cell.textContent);
    expect(cells.slice(-2)).toEqual(['2', '0']);
    const links = within(page).getByRole('list', { name: 'Links in the graph' });
    const items = within(links)
      .getAllByRole('listitem')
      .map((row) => row.textContent);
    expect(items).toContain('Meeting notes → Plan: “the plan”, line 4');
    expect(items.some((row) => row?.includes('old minutes') && row.includes('leads nowhere'))).toBe(
      true,
    );
    expect(
      items.some((row) => row?.includes('far away') && row.includes('an item outside this graph')),
    ).toBe(true);
  });

  it('says when the service cut the graph short', async () => {
    setup(() => ok({ ...GRAPH_ANSWER, truncated: true }));
    const page = await openGraph();
    await within(page).findByText(
      `The graph is cut short: it shows the ${GRAPH_ANSWER.nodes.length} items the service gave, at most ${GRAPH_MAX_NODES} at once. Items and links beyond them are not shown.`,
    );
  });

  it('says why it could not be read, never that there is nothing', async () => {
    setup(() => fail(501, 'not_implemented', 'get_graph is not built'));
    const page = await openGraph();
    const alert = await within(page).findByRole('alert');
    expect(alert.textContent).toContain('Reading the graph: Not available in this build.');
    expect(within(page).queryByText('No items to show.')).toBeNull();
  });

  it('says so when there are no items', async () => {
    setup(() => ok({ revision: OLD, nodes: [], edges: [], truncated: false }));
    const page = await openGraph();
    await within(page).findByText('No items to show.');
  });

  it('draws, or says why it cannot, and points to the list either way', async () => {
    setup(() => ok(GRAPH_ANSWER));
    const page = await openGraph();
    await within(page).findByRole('table', { name: 'Items in the graph' });
    const drawing = within(page).getByRole('img');
    expect(drawing.getAttribute('aria-label')).toContain('the table and the list below');
  });
});

describe('what the drawing holds', () => {
  it('draws every item answered, and only the links between items it shows', () => {
    const elements = graphElements(GRAPH_ANSWER);
    expect(elements.nodes.map((node) => node.data.id)).toEqual([NOTE, OTHER, SOURCE]);
    expect(elements.nodes.map((node) => node.data.label)).toEqual([
      'Meeting notes',
      'Plan',
      'Annual report',
    ]);
    expect(elements.edges.map((edge) => [edge.data.source, edge.data.target])).toEqual([
      [NOTE, OTHER],
      [NOTE, SOURCE],
    ]);
  });
});
