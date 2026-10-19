/**
 * Phase 0: ViewDocument Zod + prepareSpec + render equivalence for the six-component catalog.
 *
 * The fixture is rendered twice. With nothing resolved, each data component shows its
 * unavailable state. With the data each component needs (the committed source, dataset and
 * bindings fixtures, resolved as PresentView resolves them), each of the six draws what only a
 * real render of that data produces: a row's cells, a chart mark per row, the source's words.
 */

import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type { TopLevelSpec } from 'vega-lite';
import { describe, expect, it } from 'vitest';
import metricsDatasetFixture from '../../../tests/fixtures/views/present-metrics.dataset.json';
import presentResponseFixture from '../../../tests/fixtures/views/present-response.json';
import sourceReadItemFixture from '../../../tests/fixtures/views/source-read-item.json';
import viewDocumentFixture from '../../../tests/fixtures/views/view-document-six-component.json';
import { createClient } from '../../src/api/generated/client';
import {
  zDataset,
  zPresentResponse,
  zReadItemResponse,
  zViewDocument,
} from '../../src/api/generated/zod.gen';
import { BindingsContext, type ResolvedPresentation } from '../../src/features/views/Bindings';
import { Layout, prepareSpec } from '../../src/features/views/Layout';
import { datasetRecords, PresentView, parseDataset } from '../../src/features/views/PresentView';
import { SavedView, type SavedViewProps } from '../../src/features/views/SavedView';
import { parseViewFile } from '../../src/features/views/view-file';
import { serverToolData } from '../../src/mcp-apps/view-data';

/** The source the committed dataset cites, as the generated schema reads it. */
const fixtureSource = zDataset.parse(metricsDatasetFixture).source;

const emptyBindings: ResolvedPresentation = {
  charts: new Map(),
  sources: new Map(),
  bindings: new Map(),
  tables: new Map(),
};

const CATALOG_TYPES = [
  'Stack',
  'Columns',
  'SourceExcerpt',
  'DataTable',
  'Chart',
  'SourceList',
] as const;

function canonicalShape(spec: {
  root: string;
  elements: Record<
    string,
    {
      type: string;
      props: Record<string, unknown>;
      children?: string[];
      slots?: Record<string, string[]>;
    }
  >;
}) {
  const elements: Record<
    string,
    {
      type: string;
      props: Record<string, unknown>;
      children: string[];
      slots?: Record<string, string[]>;
    }
  > = {};
  for (const [key, element] of Object.entries(spec.elements)) {
    elements[key] = {
      type: element.type,
      props: element.props,
      children: element.children ?? [],
      ...(element.slots === undefined ? {} : { slots: element.slots }),
    };
  }
  return { root: spec.root, elements };
}

/** How long the chart may take to draw its marks before the poll says so. */
const CHART_DRAWS_WITHIN_MS = 15_000;
/** The timeout of every test here: longer than the poll plus the rest of a test, or vitest's default 5 s would end the test before the poll can fail. */
const TEST_TIMEOUT_MS = CHART_DRAWS_WITHIN_MS + 15_000;

describe('ViewDocument json-render round-trip', { timeout: TEST_TIMEOUT_MS }, () => {
  it('parses the fixture with Zod, prepareSpec, and renders catalog roles', () => {
    const document = zViewDocument.parse(viewDocumentFixture);
    expect(document.grammar).toBe('json_render');
    expect(document.mode).toBe('pinned');

    const prepared = prepareSpec(document.spec);
    expect(prepared.ok).toBe(true);
    if (!prepared.ok) return;

    const types = Object.values(prepared.spec.elements).map((element) => element.type);
    for (const catalogType of CATALOG_TYPES) {
      expect(types).toContain(catalogType);
    }
    const again = prepareSpec(JSON.parse(JSON.stringify(prepared.spec)));
    expect(again.ok).toBe(true);
    if (!again.ok) return;
    expect(canonicalShape(again.spec)).toEqual(canonicalShape(prepared.spec));
    expect(prepared.spec.elements.root?.props).toEqual({ title: 'Six-component catalog' });
    expect(prepared.spec.elements.excerpt?.props).toEqual({ binding: 'venue' });
    expect(prepared.spec.elements.sources?.props).toEqual({
      bindings: ['venue', 'metrics'],
    });

    render(
      <BindingsContext.Provider value={emptyBindings}>
        <Layout spec={document.spec} />
      </BindingsContext.Provider>,
    );

    expect(screen.getByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Metrics chart' })).toBeTruthy();
    const alerts = screen.getAllByRole('alert');
    expect(
      alerts.some((node) => /Source binding unavailable: venue/.test(node.textContent ?? '')),
    ).toBe(true);
    expect(alerts.some((node) => /Dataset unavailable: metrics/.test(node.textContent ?? ''))).toBe(
      true,
    );
    const list = screen.getByRole('list');
    expect(within(list).getByText('Unresolved: venue')).toBeTruthy();
    expect(within(list).getByText('Unresolved: metrics')).toBeTruthy();
  });

  it('renders each of the six catalog components with the data it needs', async () => {
    const document = zViewDocument.parse(viewDocumentFixture);
    const source = zReadItemResponse.parse(sourceReadItemFixture);
    const resolved = zPresentResponse.parse(presentResponseFixture).resolved_bindings;
    const dataset = zDataset.parse(metricsDatasetFixture);
    const rows = datasetRecords(dataset);
    // The fixtures are the ones the view names: its two bindings, its chart, and the dataset behind both.
    expect(resolved.map((binding) => binding.name)).toEqual(['venue', 'metrics']);
    expect(Object.keys(document.charts ?? {})).toEqual(['metrics_chart']);
    expect(dataset.rows).toHaveLength(5);
    expect(rows).toHaveLength(5);
    const presentation: ResolvedPresentation = {
      charts: new Map([['metrics_chart', document.charts?.metrics_chart as TopLevelSpec]]),
      sources: new Map([['venue', source]]),
      bindings: new Map(resolved.map((binding) => [binding.name, binding])),
      tables: new Map([['metrics', rows]]),
    };

    const { container } = render(
      <BindingsContext.Provider value={presentation}>
        <Layout spec={document.spec} />
      </BindingsContext.Provider>,
    );
    const only = (selector: string, within_: ParentNode = container) => {
      const found = within_.querySelectorAll(selector);
      expect(found, selector).toHaveLength(1);
      return found[0] as HTMLElement;
    };
    const texts = (nodes: Iterable<Element>) => Array.from(nodes, (node) => node.textContent);

    // Stack: the titled section that holds everything else.
    const stack = only('section.view-stack');
    expect(only('h2', stack).textContent).toBe('Six-component catalog');

    // Columns: one container, and the two components the spec puts in it are drawn inside it.
    const columns = only('.view-columns', stack);
    const excerpt = only('article.source-excerpt', columns);
    const table = only(':scope > table', columns);

    // SourceExcerpt: the path and revision of the source it was given, and its words as rendered Markdown.
    expect(only('header strong', excerpt).textContent).toBe('fixtures/qualification-source.md');
    expect(only('header code', excerpt).textContent).toBe(source.source.revision);
    expect(only('h1', excerpt).textContent).toBe('Qualification source');
    expect(only('p', excerpt).textContent).toBe(
      'Contract-valid ReadItemResponse fixture for the MCP Apps harness.',
    );
    expect(excerpt.textContent).not.toContain('# Qualification source');

    // DataTable: a column per field and a row per record of the dataset, cell for cell.
    expect(only('caption', table).textContent).toBe('metrics');
    expect(texts(table.querySelectorAll('thead th'))).toEqual(['category', 'value']);
    const body = Array.from(table.querySelectorAll('tbody tr'), (row) =>
      texts(row.querySelectorAll('td')),
    );
    expect(body).toEqual(rows.map((row) => [row.category, String(row.value)]));
    expect(body[0]).toEqual(['Ingested', '412']);

    // Chart: its title, one drawn mark per row of the dataset, and the same rows as its source table.
    const chart = only(':scope > section', stack);
    expect(only('h3', chart).textContent).toBe('Metrics chart');
    const marks = () => chart.querySelectorAll('svg g[class~="role-mark"] > *');
    // Drawing takes about 0.2 s here (the whole test runs in 0.16 s); the poll waits 75 times that for a loaded
    // machine, and the test timeout above is longer than the poll, so a chart that draws nothing fails
    // with this assertion's message and not with "Test timed out".
    await expect.poll(() => marks().length, { timeout: CHART_DRAWS_WITHIN_MS }).toBe(rows.length);
    for (const mark of marks()) expect(mark.tagName.toLowerCase()).toBe('path');
    const chartTable = only('details table', chart);
    expect(only('caption', chartTable).textContent).toBe('metrics');
    expect(chartTable.querySelectorAll('tbody tr')).toHaveLength(rows.length);

    // SourceList: each binding by the path and revision it resolved to.
    const list = only(':scope > ul', stack);
    expect(texts(list.querySelectorAll('li'))).toEqual([
      `fixtures/qualification-source.md @ ${source.source.revision}`,
      `fixtures/qualification-metrics.json @ ${source.source.revision}`,
    ]);

    // Nothing is unavailable and nothing is unresolved: every one of the six drew its data.
    expect(screen.queryByRole('alert')).toBeNull();
    expect(container.textContent).not.toMatch(/unavailable|Unresolved/);
  });
});

describe('the Dataset the views read', () => {
  it("is the converter's typed table, never a bare array of records", () => {
    expect(zDataset.safeParse(metricsDatasetFixture).success).toBe(true);
    const bare = [{ category: 'Ingested', value: 412 }];
    expect(zDataset.safeParse(bare).success).toBe(false);
  });

  it('turns rows into records by column name and refuses a row of the wrong width', () => {
    const dataset = zDataset.parse(metricsDatasetFixture);
    expect(datasetRecords(dataset).at(0)).toEqual({ category: 'Ingested', value: 412 });
    const ragged = { ...dataset, rows: [['Ingested']] };
    expect(() => datasetRecords(ragged)).toThrow(/row 0 has 1 values for 2 columns/);
  });

  it("shows the chart's own alert when its binding has no dataset", async () => {
    const response = zPresentResponse.parse(presentResponseFixture);
    const binding = response.resolved_bindings.find((entry) => entry.name === 'metrics');
    expect(binding).toBeTruthy();
    const { materialized: _dropped, ...unmaterialized } = binding as NonNullable<typeof binding>;
    const document = zViewDocument.parse(viewDocumentFixture);
    const message = "The source's shown text is not the converter's, so no dataset can be read.";
    const refused = zPresentResponse.parse({
      ...presentResponseFixture,
      resolved_bindings: [unmaterialized],
      view: { ...document, grammar: 'vega_lite', spec: document.charts?.metrics_chart },
      charts: [
        {
          chart: { kind: 'spec' },
          status: {
            status: 'failed',
            reason: 'dataset_unavailable',
            binding: 'metrics',
            message,
          },
        },
      ],
    });
    const data = serverToolData(async () => ({
      structuredContent: zReadItemResponse.parse(sourceReadItemFixture),
    }));
    render(<PresentView response={refused} data={data} />);
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toBe(`Dataset unavailable: ${message}`),
    );
    // The View's own title and description stay: only the chart is refused.
    expect(screen.getByRole('heading', { name: refused.view.title })).toBeTruthy();
  });
});

async function digestOf(bytes: Uint8Array<ArrayBuffer>) {
  return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
}

/**
 * PresentView resolving the fixture's View with these datasets behind the named bindings. A name
 * not in the fixture is added as a table and a chart over the metrics source. The bytes are exactly
 * the payloads, as the host serves them; `venue` has none.
 */
async function presentMany(datasets: Record<string, unknown>) {
  const fixture = JSON.parse(JSON.stringify(presentResponseFixture));
  const metrics = fixture.resolved_bindings.find(
    (entry: { name: string }) => entry.name === 'metrics',
  );
  const served = new Map<string, string>();
  const bindings = [fixture.resolved_bindings[0]];
  const elements = fixture.view.spec.elements;
  for (const [name, payload] of Object.entries(datasets)) {
    const bytes = new TextEncoder().encode(JSON.stringify(payload));
    const digest = await digestOf(bytes);
    let binary = '';
    for (const byte of bytes) binary += String.fromCharCode(byte);
    served.set(digest, binary);
    bindings.push({ ...metrics, name, materialized: digest });
    if (name !== 'metrics') {
      elements[`${name}_table`] = { type: 'DataTable', props: { binding: name }, children: [] };
      elements[`${name}_chart`] = {
        type: 'Chart',
        props: { binding: name, chart: 'metrics_chart', title: `${name} chart` },
        children: [],
      };
      elements.root.children.push(`${name}_table`, `${name}_chart`);
      elements.root.slots.default.push(`${name}_table`, `${name}_chart`);
      fixture.view.bindings.push({ ...metrics, name, materialized: digest });
    }
  }
  fixture.resolved_bindings = bindings;
  fixture.charts = [
    {
      chart: { kind: 'named', name: 'metrics_chart' },
      // What the server reports: the chart's spec reads `metrics` alone, whichever binding a
      // component showing it cites.
      status: { status: 'ready', bindings: ['metrics'] },
    },
  ];
  const response = zPresentResponse.parse(fixture);
  const data = serverToolData(async ({ name, arguments: input }) => {
    if (name === 'show')
      return { structuredContent: zReadItemResponse.parse(sourceReadItemFixture) };
    const binary = served.get(String(input.object)) ?? '';
    return {
      structuredContent: {
        data_base64: btoa(binary),
        has_more: false,
        media_type: 'application/json',
        offset: '0',
        sha256: String(input.object),
        total_size: String(binary.length),
      },
    };
  });
  return render(<PresentView response={response} data={data} />);
}

/** PresentView resolving one dataset behind `metrics`. */
function presentServing(payload: unknown) {
  return presentMany({ metrics: payload });
}

describe("PresentView refuses a dataset that is not the converter's typed table", () => {
  const good = JSON.parse(JSON.stringify(metricsDatasetFixture)) as Record<string, unknown> & {
    columns: unknown[];
    rows: unknown[][];
  };
  const refusals: Array<[string, unknown, RegExp]> = [
    ['a missing columns', { ...good, columns: undefined }, /malformed at columns/],
    [
      'a wrong column kind',
      { ...good, columns: [{ name: 'category', kind: 'text' }, good.columns[1]] },
      /malformed at columns.0.kind/,
    ],
    ['a row of the wrong width', { ...good, rows: [['Ingested']] }, /row 0 has 1 values for 2/],
    ['a bare records array', [{ category: 'Ingested', value: 412 }], /malformed/],
    [
      'text supplied by an agent',
      { ...good, text_origin: 'supplied_by_agent' },
      /not the converter's .text_origin is supplied_by_agent./,
    ],
    [
      'text with no origin',
      { ...good, text_origin: 'none' },
      /not the converter's .text_origin is none./,
    ],
    [
      'two columns of one name',
      { ...good, columns: [good.columns[0], good.columns[0]], rows: [['a', 'b']] },
      /two columns named "category"/,
    ],
  ];
  for (const [name, payload, message] of refusals) {
    it(`shows the chart's own alert for ${name}`, async () => {
      await presentServing(payload);
      const alerts = await screen.findAllByRole('alert');
      expect(alerts.some((alert) => message.test(alert.textContent ?? ''))).toBe(true);
      expect(document.querySelector('svg')).toBeNull();
    });
  }

  it('keeps a null cell null in the table and the chart data, never 0', async () => {
    const withNull = {
      ...good,
      rows: [
        ['Ingested', null],
        ['Converted', 397],
      ],
    };
    const records = parseDataset(JSON.stringify(withNull), fixtureSource);
    expect(records.at(0)).toEqual({ category: 'Ingested', value: null });
    const { container } = render(
      <BindingsContext.Provider
        value={{
          charts: new Map(),
          sources: new Map(),
          bindings: new Map(),
          tables: new Map([['metrics', records]]),
        }}
      >
        <Layout spec={zViewDocument.parse(viewDocumentFixture).spec} />
      </BindingsContext.Provider>,
    );
    const cells = Array.from(
      container.querySelectorAll('tbody tr:first-child td'),
      (n) => n.textContent,
    );
    expect(cells).not.toContain('0');
    expect(cells.at(0)).toBe('Ingested');
  });
});

describe("a bad dataset is its own binding's alert, not the View's", () => {
  it('renders the prose, the source and the valid chart around one malformed dataset', async () => {
    const { container } = await presentMany({
      metrics: metricsDatasetFixture,
      broken: { ...metricsDatasetFixture, columns: undefined },
    });
    await waitFor(() =>
      expect(
        screen
          .getAllByRole('alert')
          .some((node) => /malformed at columns/.test(node.textContent ?? '')),
      ).toBe(true),
    );
    // The valid binding still draws its table cells and its chart marks.
    expect(container.textContent).toContain('Ingested');
    await waitFor(() => expect(container.querySelector('svg')).not.toBeNull(), { timeout: 15_000 });
    // The View, its title and the source components are all still there.
    expect(screen.getByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
    expect(container.querySelector('article.source-excerpt, .view-columns')).not.toBeNull();
    // The broken binding's table and chart each say why; nothing else was replaced.
    const reasons = screen
      .getAllByRole('alert')
      .filter((node) => /malformed at columns/.test(node.textContent ?? ''));
    expect(reasons.length).toBe(2);
  });
});

describe("each column's declared kind is enforced as the contract's DatasetValue::fits", () => {
  const one = (kind: string, value: unknown) => ({
    ...metricsDatasetFixture,
    columns: [{ name: 'c', kind }],
    rows: [[value]],
  });
  const bad: Array<[string, string, unknown]> = [
    ['integer', 'unknown text', 'unknown'],
    ['integer', 'a fraction', 1.5],
    ['integer', 'beyond 64 bits', 1e30],
    ['number', 'text', '1'],
    ['boolean', 'a number', 1],
    ['string', 'a number', 5],
    ['date_time', 'a date without a time', '2026-01-01'],
    ['date_time', 'an instant that does not exist', '2026-02-30T00:00:00.000Z'],
    ['date_time', 'an offset spelling', '2026-01-01T00:00:00.000+01:00'],
    ['date_time', 'no milliseconds', '2026-01-01T00:00:00Z'],
  ];
  for (const [kind, what, value] of bad) {
    it(`refuses ${what} in a ${kind} column, with its path`, async () => {
      expect(() => parseDataset(JSON.stringify(one(kind, value)), fixtureSource)).toThrow(
        /malformed at rows.0.0: a value of column c is not of its kind/,
      );
      await presentServing(one(kind, value));
      const alerts = await screen.findAllByRole('alert');
      expect(alerts.some((node) => /malformed at rows.0.0/.test(node.textContent ?? ''))).toBe(
        true,
      );
    });
  }

  it('accepts a value of its kind, and null in every kind', () => {
    const good: Array<[string, unknown]> = [
      ['integer', 3],
      ['integer', -7],
      ['number', 1.5],
      ['boolean', false],
      ['string', 'text'],
      ['date_time', '2026-01-01T00:00:00.000Z'],
    ];
    for (const [kind, value] of good) {
      expect(parseDataset(JSON.stringify(one(kind, value)), fixtureSource).at(0)).toEqual({
        c: value,
      });
      expect(parseDataset(JSON.stringify(one(kind, null)), fixtureSource).at(0)).toEqual({
        c: null,
      });
    }
  });
});

describe('a dataset read from another source than its binding cites is stale', () => {
  const cited = zPresentResponse
    .parse(presentResponseFixture)
    .resolved_bindings.find((entry) => entry.name === 'metrics')?.source;
  if (!cited) throw new Error('the fixture has no metrics binding');
  const from = (change: Record<string, unknown>) => ({
    ...metricsDatasetFixture,
    source: { ...metricsDatasetFixture.source, ...change },
  });

  it('is refused for a different revision, path, item, workspace or selection', async () => {
    const other: Array<Record<string, unknown>> = [
      { revision: 'f'.repeat(40) },
      { path: 'fixtures/another.json' },
      { item_id: '99999999-3333-4444-8555-666666666666' },
      { workspace_id: 'bbbbbbbb-bbbb-4ccc-8ddd-ffffffffffff' },
      { selection: { kind: 'lines', range: { start: 1, end: 2 } } },
      { digest: 'a'.repeat(64) },
    ];
    for (const change of other) {
      expect(
        () => parseDataset(JSON.stringify(from(change)), cited),
        JSON.stringify(change),
      ).toThrow(/stale/);
    }
    await presentServing(from({ revision: 'f'.repeat(40) }));
    const alerts = await screen.findAllByRole('alert');
    expect(alerts.some((node) => /stale/.test(node.textContent ?? ''))).toBe(true);
  });

  it('is accepted when only the derived locations or the key order differ', () => {
    const same = from({ locations: [] });
    expect(parseDataset(JSON.stringify(same), cited)).toHaveLength(5);
    const reordered = {
      ...same,
      source: Object.fromEntries(Object.entries(same.source).reverse()),
    };
    expect(parseDataset(JSON.stringify(reordered), cited)).toHaveLength(5);
  });
});

/** A failure notice's way home is a router link, so a refusal that offers one renders in a router. */
async function routed(element: ReactNode) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => element }),
    history: createMemoryHistory({ initialEntries: ['/somewhere'] }),
  });
  render(<RouterProvider router={router} />);
  await screen.findByRole('alert');
}

describe('a saved View reopens with the server as the source of truth', () => {
  const saved = zPresentResponse.parse(presentResponseFixture);
  const noTools = serverToolData(async () => ({
    isError: true,
    content: [{ type: 'text', text: 'no host' }],
  }));

  /** The generated client, with only its transport replaced: what resolve-view answers, and what it was asked. */
  function server(status: number, body: unknown) {
    const asked: { url: string; body: unknown }[] = [];
    const client = createClient({
      baseUrl: 'http://okf.test',
      // Bun's `typeof fetch` also carries `preconnect`; the client never calls it.
      fetch: Object.assign(
        async (input: URL | RequestInfo, init?: RequestInit) => {
          const request = new Request(input, init);
          asked.push({ url: request.url, body: await request.json() });
          return new Response(JSON.stringify(body), {
            status,
            headers: { 'Content-Type': 'application/json' },
          });
        },
        { preconnect: () => {} },
      ),
    });
    return { client, asked };
  }
  const ids = {
    workspaceId: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
    itemId: '11111111-2222-4333-8444-555555555555',
    at: { kind: 'revision', revision: '0123456789abcdef0123456789abcdef01234567' },
  } satisfies Partial<SavedViewProps>;

  it('asks the server to resolve the View at the revision its caller pins, never to refresh it', async () => {
    const { client, asked } = server(200, { ...saved, resolved_bindings: [] });
    render(<SavedView {...ids} data={noTools} client={client} />);
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
    expect(asked).toHaveLength(1);
    expect(asked[0]?.url).toBe('http://okf.test/api/views/resolve-view');
    expect(asked[0]?.body).toEqual({
      workspace_id: ids.workspaceId,
      item_id: ids.itemId,
      at: ids.at,
      refresh_live: false,
    });
  });

  it('asks again at the revision its caller pins next, and never shows the earlier answer', async () => {
    const later = {
      kind: 'revision',
      revision: '89abcdef0123456789abcdef0123456789abcdef',
    } as const;
    const asked: unknown[] = [];
    const client = createClient({
      baseUrl: 'http://okf.test',
      fetch: Object.assign(
        async (input: URL | RequestInfo, init?: RequestInit) => {
          const body = (await new Request(input, init).json()) as { at: { revision: string } };
          asked.push(body.at);
          const title = `Board at ${body.at.revision.slice(0, 7)}`;
          const spec = {
            root: 'root',
            elements: { root: { type: 'Stack', props: { title }, children: [] } },
          };
          return new Response(
            JSON.stringify({ ...saved, view: { ...saved.view, spec }, resolved_bindings: [] }),
            { status: 200, headers: { 'Content-Type': 'application/json' } },
          );
        },
        { preconnect: () => {} },
      ),
    });
    const view = render(<SavedView {...ids} data={noTools} client={client} />);
    expect(await screen.findByRole('heading', { name: 'Board at 0123456' })).toBeTruthy();
    view.rerender(<SavedView {...ids} at={later} data={noTools} client={client} />);
    expect(screen.queryByRole('heading', { name: 'Board at 0123456' })).toBeNull();
    expect(await screen.findByRole('heading', { name: 'Board at 89abcde' })).toBeTruthy();
    expect(asked).toEqual([ids.at, later]);
  });

  it('asks once for a selector its caller gives again as a new object of the same value', async () => {
    const { client, asked } = server(200, { ...saved, resolved_bindings: [] });
    const view = render(<SavedView {...ids} at={{ ...ids.at }} data={noTools} client={client} />);
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
    view.rerender(<SavedView {...ids} at={{ ...ids.at }} data={noTools} client={client} />);
    view.rerender(<SavedView {...ids} at={{ ...ids.at }} data={noTools} client={client} />);
    await new Promise((done) => setTimeout(done, 50));
    expect(asked).toHaveLength(1);
    expect(screen.getByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
  });

  it("shows the server's failed chart status as that chart's alert, beside the View's text", async () => {
    const diagnosed = {
      ...saved,
      resolved_bindings: [],
      charts: [
        {
          chart: { kind: 'named', name: 'metrics_chart' },
          status: { status: 'failed', reason: 'invalidated', message: 'the source was purged' },
        },
      ],
    };
    const { client } = server(200, diagnosed);
    render(<SavedView {...ids} data={noTools} client={client} />);
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
    const chart = screen.getByRole('heading', { name: 'Metrics chart' }).closest('section');
    expect(chart?.querySelector('[role="alert"]')?.textContent).toBe(
      'Invalidated: the source was purged',
    );
  });

  it("shows the server's refusal in the standing failure notice, with its own message", async () => {
    const { client } = server(404, {
      code: 'not_found',
      message: 'no saved View at that item',
    });
    await routed(<SavedView {...ids} data={noTools} client={client} />);
    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText('Reopening this View: This could not be found.')).toBeTruthy();
    expect(alert.textContent).toContain('The service said: no saved View at that item');
    // A View that is not there offers the way back to its workspace, not a retry.
    expect(within(alert).getByRole('link', { name: 'Go to this workspace' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  });

  it('refuses a body the generated schema does not read, rather than drawing it', async () => {
    const { client } = server(200, { view: 'not a view' });
    render(<SavedView {...ids} data={noTools} client={client} />);
    const alert = await screen.findByRole('alert');
    expect(
      within(alert).getByText(
        'Reopening this View: The server returned a View this client cannot read.',
      ),
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
    expect(screen.queryByRole('heading', { name: 'Six-component catalog' })).toBeNull();
  });

  it('offers a retry that keeps the notice until the View answers, then moves focus to it', async () => {
    const asked: unknown[] = [];
    const answers = [
      { status: 500, body: { code: 'internal', message: 'the store was busy' } },
      { status: 200, body: { ...saved, resolved_bindings: [] } },
    ];
    const client = createClient({
      baseUrl: 'http://okf.test',
      fetch: Object.assign(
        async (input: URL | RequestInfo, init?: RequestInit) => {
          asked.push(await new Request(input, init).json());
          const { status, body } = answers[Math.min(asked.length, answers.length) - 1] ?? {
            status: 500,
            body: {},
          };
          return new Response(JSON.stringify(body), {
            status,
            headers: { 'Content-Type': 'application/json' },
          });
        },
        { preconnect: () => {} },
      ),
    });
    render(<SavedView {...ids} data={noTools} client={client} />);
    const alert = await screen.findByRole('alert');
    expect(
      within(alert).getByText('Reopening this View: Something went wrong in the service.'),
    ).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    const title = await screen.findByRole('heading', { name: 'Six-component catalog view' });
    expect(asked).toHaveLength(2);
    expect(asked.at(1)).toEqual(asked.at(0));
    // The notice is gone (the View's own alerts for its unread sources are another matter).
    expect(screen.queryByText(/^Reopening this View:/)).toBeNull();
    // Keyboard focus was on the retry button; it is now on the View that took the notice's place.
    const focused = document.activeElement;
    expect(focused).not.toBe(document.body);
    expect(focused?.contains(title)).toBe(true);
  });
});

describe('a saved View follows the ids it was asked for', () => {
  const saved = zPresentResponse.parse(presentResponseFixture);
  const noTools = serverToolData(async () => ({
    isError: true,
    content: [{ type: 'text', text: 'no host' }],
  }));
  const first = {
    workspaceId: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
    itemId: '11111111-2222-4333-8444-555555555555',
    at: { kind: 'latest' },
  } satisfies Partial<SavedViewProps>;
  const second = { ...first, itemId: '99999999-2222-4333-8444-555555555555' };

  /** A client whose answer for each item is released by the test, so a request can stay pending. */
  function gated() {
    const release = new Map<string, (title: string) => void>();
    const client = createClient({
      baseUrl: 'http://okf.test',
      fetch: Object.assign(
        async (input: URL | RequestInfo, init?: RequestInit) => {
          const asked = (await new Request(input, init).json()) as { item_id: string };
          const title = await new Promise<string>((done) => release.set(asked.item_id, done));
          const spec = {
            root: 'root',
            elements: { root: { type: 'Stack', props: { title }, children: [] } },
          };
          const view = { ...saved.view, title: `View: ${title}`, spec };
          return new Response(JSON.stringify({ ...saved, view, resolved_bindings: [] }), {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          });
        },
        { preconnect: () => {} },
      ),
    });
    return { client, release };
  }
  async function released(release: Map<string, (title: string) => void>, item: string) {
    await waitFor(() => expect(release.has(item)).toBe(true));
    return release.get(item) as (title: string) => void;
  }

  it('stops showing the previous View at once when the ids change, while the new one is pending', async () => {
    const { client, release } = gated();
    const view = render(<SavedView {...first} data={noTools} client={client} />);
    (await released(release, first.itemId))('Board of the first');
    expect(await screen.findByRole('heading', { name: 'Board of the first' })).toBeTruthy();
    view.rerender(<SavedView {...second} data={noTools} client={client} />);
    expect(screen.queryByRole('heading', { name: 'Board of the first' })).toBeNull();
    expect(screen.getByRole('status').textContent).toMatch(/Reopening/);
  });

  it('never renders a late answer for ids that are no longer asked for', async () => {
    const { client, release } = gated();
    const view = render(<SavedView {...first} data={noTools} client={client} />);
    const lateFirst = await released(release, first.itemId);
    view.rerender(<SavedView {...second} data={noTools} client={client} />);
    (await released(release, second.itemId))('Board of the second');
    expect(await screen.findByRole('heading', { name: 'Board of the second' })).toBeTruthy();
    lateFirst('Board of the first');
    await new Promise((done) => setTimeout(done, 50));
    expect(screen.queryByRole('heading', { name: 'Board of the first' })).toBeNull();
    expect(screen.getByRole('heading', { name: 'Board of the second' })).toBeTruthy();
  });

  /** A client whose every answer is a 404 with this body. */
  const refusingWith = (body: unknown) =>
    createClient({
      baseUrl: 'http://okf.test',
      fetch: Object.assign(
        async () =>
          new Response(JSON.stringify(body), {
            status: 404,
            headers: { 'Content-Type': 'application/json' },
          }),
        { preconnect: () => {} },
      ),
    });

  it('shows a fixed sentence when the refusal is not an error the service describes', async () => {
    const view = render(
      <SavedView {...first} data={noTools} client={refusingWith({ code: 'not_found' })} />,
    );
    const alert = await screen.findByRole('alert');
    expect(
      within(alert).getByText('Reopening this View: The server refused to open the View.'),
    ).toBeTruthy();
    expect(alert.textContent).not.toContain('The service said');
    view.unmount();
  });

  it("shows the code's own words, and no empty quotation, when the service's message is blank", async () => {
    await routed(
      <SavedView
        {...first}
        data={noTools}
        client={refusingWith({ code: 'not_found', message: '  ' })}
      />,
    );
    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText('Reopening this View: This could not be found.')).toBeTruthy();
    expect(alert.textContent).not.toContain('The service said');
  });
});

describe('a View says how its data was read, under it', () => {
  const presented = zPresentResponse.parse(presentResponseFixture);
  const refusing = serverToolData(async () => ({
    isError: true,
    content: [{ type: 'text', text: 'not read here' }],
  }));
  const revision = presented.resolved_bindings[0]?.source.revision ?? '';

  async function regionOf(response: typeof presented) {
    render(<PresentView response={response} data={refusing} />);
    return screen.findByRole('region', { name: "Where this View's data comes from" });
  }
  const texts = (nodes: HTMLElement[]) => nodes.map((node) => node.textContent);

  it('names each source and the revision it is shown at, for a pinned View', async () => {
    const region = await regionOf(presented);
    expect(within(region).getByText(/^Pinned:/).textContent).toBe(
      'Pinned: each source is shown at the revision this View cites, and does not refresh.',
    );
    const read = within(region).getByRole('list', { name: 'Sources read' });
    expect(texts(within(read).getAllByRole('listitem'))).toEqual([
      `venue: fixtures/qualification-source.md at revision ${revision}`,
      `metrics: fixtures/qualification-metrics.json at revision ${revision}`,
    ]);
    // A pinned View names no as-of time: its inputs are the cited revisions.
    expect(region.querySelector('time')).toBeNull();
    expect(within(region).queryByRole('list', { name: 'Warnings' })).toBeNull();
  });

  it('names the as-of time and each resolved revision, for a live View', async () => {
    const later = '89abcdef0123456789abcdef0123456789abcdef';
    const live = {
      ...presented,
      view: { ...presented.view, mode: 'live' as const },
      resolved_bindings: presented.resolved_bindings.map((binding) => ({
        ...binding,
        source: { ...binding.source, revision: later },
      })),
      as_of: '2026-10-05T12:00:00.000Z',
    };
    const region = await regionOf(live);
    const said = within(region).getByText(/^Live:/);
    expect(said.textContent).toMatch(
      /^Live: each source is shown at the revision it resolved to as of .+\.$/,
    );
    expect(said.querySelector('time')?.getAttribute('datetime')).toBe('2026-10-05T12:00:00.000Z');
    const read = within(region).getByRole('list', { name: 'Sources read' });
    expect(texts(within(read).getAllByRole('listitem'))).toEqual([
      `venue: fixtures/qualification-source.md at revision ${later}`,
      `metrics: fixtures/qualification-metrics.json at revision ${later}`,
    ]);
  });

  it('shows every warning in plain words with the server message as sent, naming its binding by location', async () => {
    const warned = {
      ...presented,
      warnings: [
        {
          code: 'dataset_unavailable',
          message: 'binding metrics: no text',
          location: '/bindings/1',
        },
        { code: 'newer_source', message: 'the head differs', location: '/bindings/0' },
        { code: 'source_removed', message: 'the item is gone', location: '/bindings/1' },
        { code: 'live_resolution', message: 'resolved at the current head' },
        { code: 'unknown_unit', message: 'EUR names no column', location: '/bindings/1' },
        { code: 'some_future_code', message: 'a new note', location: '/bindings/9' },
      ],
    };
    const region = await regionOf(warned);
    const warnings = within(region).getByRole('list', { name: 'Warnings' });
    expect(texts(within(warnings).getAllByRole('listitem'))).toEqual([
      'metrics: its data could not be read, so what draws from it is not shown. The server said: “binding metrics: no text”',
      'venue: its source has changed since the revision this View cites. The View still shows that revision: newer data is an observation, not proof that the View is wrong. The server said: “the head differs”',
      'metrics: its source is no longer in the workspace. The View still shows the revision it cites. The server said: “the item is gone”',
      'This View: its sources were read as the workspace stands now, and a review never covers live data. The server said: “resolved at the current head”',
      'metrics: a unit it declares names no column of its data. The server said: “EUR names no column”',
      'This View (/bindings/9): the server noted “some_future_code”. The server said: “a new note”',
    ]);
  });
});

/**
 * The one parser of a View file, against the examples of core M1.6's own reader
 * (crates/core/src/views/layout.rs, PR #48 at 8ce63d81): the same files, the same refusals, the
 * same JSON Pointers.
 */
describe('a View file reads as its title, its description and its one block', () => {
  const workspace = 'aaaaaaaa-bbbb-4ccc-8ddd-000000000001';
  const titled = { title: 'Pipeline throughput', type: 'Dashboard' };
  const stored = () => ({
    schema_version: 1,
    mode: 'pinned',
    grammar: 'json_render',
    bindings: [
      {
        name: 'report',
        source: {
          item_id: '22222222-3333-4444-8555-666666666666',
          path: 'reports/pipeline-pdf.md',
          revision: '0123456789abcdef0123456789abcdef01234567',
          digest: 'b'.repeat(64),
          selection: { kind: 'pages', range: { start: 3, end: 3 } },
        },
        units: { Documents: 'count' },
        transforms: [],
      },
      {
        name: 'venue',
        source: {
          item_id: '11111111-2222-4333-8444-555555555555',
          path: 'notes/venue.md',
          revision: '0123456789abcdef0123456789abcdef01234567',
          selection: { kind: 'all' },
        },
        units: {},
        transforms: [],
      },
    ],
    spec: { root: 'root', elements: {} },
    charts: { stages: { data: { name: 'report' }, mark: 'bar' } },
  });
  const block = (value: unknown = stored()) => JSON.stringify(value, null, 2);
  const body = (prose: string, text = block()) =>
    `${prose}\`\`\`json okf-jawn-view\n${text}\n\`\`\`\n`;
  const read = (text: string, properties: Record<string, unknown> = titled) =>
    parseViewFile({ properties, body: text }, workspace);
  /** The refusal of a file that must not read, or a failed expectation. */
  function refusal(text: string, properties?: Record<string, unknown>) {
    const result = read(text, properties);
    if (result.ok) throw new Error('the file read as a View');
    return result;
  }
  /** The stored block with its first binding (or that binding's source) edited. */
  function edited(
    change: (binding: Record<string, unknown>, source: Record<string, unknown>) => void,
  ) {
    const value = stored();
    const binding = value.bindings[0] as unknown as Record<string, unknown>;
    change(binding, binding.source as Record<string, unknown>);
    return body('', block(value));
  }

  it('reads the title property, the prose before the block and the block, in the given workspace', () => {
    const result = read(body('Two lines\nof prose\n\n\n'));
    if (!result.ok) throw new Error(result.message);
    expect(zViewDocument.parse(result.document)).toStrictEqual(result.document);
    expect(result.document.title).toBe('Pipeline throughput');
    expect(result.document.description).toBe('Two lines\nof prose');
    expect(result.document.bindings.map((binding) => binding.source.workspace_id)).toEqual([
      workspace,
      workspace,
    ]);
    for (const binding of result.document.bindings) {
      expect(Object.hasOwn(binding, 'materialized')).toBe(false);
      expect(Object.hasOwn(binding.source, 'locations')).toBe(false);
    }
    expect(result.document.bindings[0]?.source.digest).toBe('b'.repeat(64));
    expect(Object.keys(result.document.charts ?? {})).toEqual(['stages']);
    const bare = read(body(''));
    expect(bare.ok && bare.document.description).toBe('');
    // The same file in another workspace binds there and differs in nothing else.
    const other = 'aaaaaaaa-bbbb-4ccc-8ddd-000000000002';
    const elsewhere = parseViewFile(
      { properties: titled, body: body('Two lines\nof prose\n\n\n') },
      other,
    );
    if (!elsewhere.ok) throw new Error(elsewhere.message);
    expect(
      elsewhere.document.bindings.every((binding) => binding.source.workspace_id === other),
    ).toBe(true);
    expect(elsewhere.document.spec).toStrictEqual(result.document.spec);
  });

  it('reads a CRLF file as its LF spelling', () => {
    const lf = body('Prose\n\n');
    expect(read(lf.replaceAll('\n', '\r\n'))).toStrictEqual(read(lf));
  });

  it('keeps a fence of another info string in the prose as prose', () => {
    const prose = 'An example:\n\n```js\nconsole.log(1);\n```\n\n';
    const result = read(body(prose));
    if (!result.ok) throw new Error(result.message);
    expect(result.document.description).toBe('An example:\n\n```js\nconsole.log(1);\n```');
  });

  it('refuses a body without exactly one block that ends it', () => {
    const one = body('');
    expect(refusal('Only prose.\n')).toMatchObject({ problem: 'no_block', pointer: null });
    expect(refusal('Only prose.\n').message).toMatch(/ends with one/);
    const twice = refusal(`${one}\n${one}`);
    expect(twice.problem).toBe('two_blocks');
    expect(twice.message).toMatch(/2 okf-jawn-view blocks/);
    const trailing = refusal(`${one}More prose.\n`);
    expect(trailing.problem).toBe('not_last');
    expect(trailing.message).toMatch(/must end the body/);
    const open = refusal(`\`\`\`json okf-jawn-view\n${block()}\n`);
    expect(open.problem).toBe('not_closed');
    expect(open.message).toMatch(/not closed/);
  });

  it('reads only the exact opening line: a block of another info string is no View block', () => {
    for (const marker of [
      '```json',
      '```JSON okf-jawn-view',
      ' ```json okf-jawn-view',
      '````json okf-jawn-view',
      '~~~json okf-jawn-view',
      '```json okf-jawn-view extra',
    ]) {
      const refused = refusal(`${marker}\n${block()}\n\`\`\`\n`);
      expect(refused.problem, marker).toBe('no_block');
    }
  });

  it('counts the marker quoted inside a fence of the prose as a second block', () => {
    const quoted = refusal(body('~~~\n```json okf-jawn-view\n~~~\n\n'));
    expect(quoted.problem).toBe('two_blocks');
    expect(quoted.message).toMatch(/2 okf-jawn-view blocks/);
  });

  it('refuses a block that repeats a key, where it repeats', () => {
    const top = block().replace('"mode": "pinned",', '"mode": "pinned",\n  "mode": "live",');
    const repeated = refusal(body('', top));
    expect(repeated).toMatchObject({ problem: 'repeated_key', pointer: null });
    expect(repeated.message).toMatch(/`mode` is repeated/);
    const inner = block().replace('"units": {}', '"units": { "a": "x", "a": "y" }');
    expect(refusal(body('', inner))).toMatchObject({
      problem: 'repeated_key',
      pointer: '/bindings/1/units',
    });
  });

  it("refuses JSON nested deeper than core reads as not JSON, at core's own limit, never throwing", () => {
    // Core reads the block with serde_json, which refuses the 128th array or object nested in one
    // another, the block's own object the first: core's `view_from` reads 127 and refuses 128 with
    // "the View block is not JSON: recursion limit exceeded" and no field.
    const nested = (depth: number) => {
      // `spec` is the second level; the arrays in it are the rest.
      const arrays = depth - 1;
      const text = block({ ...stored(), spec: 'NESTED' }).replace(
        '"NESTED"',
        `${'['.repeat(arrays)}${']'.repeat(arrays)}`,
      );
      return body('', text);
    };
    expect(read(nested(127)).ok).toBe(true);
    const refused = refusal(nested(128));
    expect(refused).toMatchObject({ problem: 'not_json', pointer: null });
    expect(refused.message).toMatch(/not JSON.*127/);
    expect(refusal(nested(100_000))).toMatchObject({ problem: 'not_json', pointer: null });
  });

  it('refuses a derived field by name, and an unknown field, at its pointer', () => {
    const named: Array<
      [
        string,
        string,
        RegExp,
        (binding: Record<string, unknown>, source: Record<string, unknown>) => void,
      ]
    > = [
      [
        'workspace_id',
        '/bindings/0/source/workspace_id',
        /own workspace/,
        (_, source) => {
          source.workspace_id = [];
        },
      ],
      [
        'locations',
        '/bindings/0/source/locations',
        /derived/,
        (_, source) => {
          source.locations = [];
        },
      ],
      [
        'materialized',
        '/bindings/0/materialized',
        /resolved/,
        (binding) => {
          binding.materialized = [];
        },
      ],
    ];
    for (const [key, pointer, gist, change] of named) {
      const refused = refusal(edited(change));
      expect(refused, key).toMatchObject({ problem: 'schema', pointer });
      expect(refused.message, key).toMatch(gist);
    }
    expect(refusal(body('', block({ ...stored(), theme: 'dark' })))).toMatchObject({
      problem: 'schema',
      pointer: '/theme',
    });
  });

  it('refuses a title that is absent or not a string', () => {
    expect(refusal(body(''), {}).pointer).toBe('/title');
    expect(refusal(body(''), { title: 2024 }).pointer).toBe('/title');
  });

  it('refuses a later version for its version, before its fields', () => {
    const refused = refusal(body('', block({ ...stored(), schema_version: 2, theme: 'dark' })));
    expect(refused).toMatchObject({ problem: 'version', pointer: '/schema_version' });
    expect(refused.message).toMatch(/version 2/);
  });

  it('refuses a missing or mistyped field at its pointer, and a block that is not JSON', () => {
    const { mode: _mode, ...modeless } = stored();
    expect(refusal(body('', block(modeless)))).toMatchObject({
      problem: 'schema',
      pointer: '/mode',
    });
    const path = edited((_, source) => {
      source.path = 'notes/<venue>.md';
    });
    expect(refusal(path)).toMatchObject({ problem: 'schema', pointer: '/bindings/0/source/path' });
    const notJson = refusal(body('', '{ "schema_version": 1,'));
    expect(notJson.problem).toBe('not_json');
    expect(notJson.message).toMatch(/not JSON/);
  });
});

describe('a json_render View shows its own title and description, as a vega_lite View does', () => {
  it('names the View above its composition', async () => {
    const presented = zPresentResponse.parse(presentResponseFixture);
    const refusing = serverToolData(async () => ({ isError: true, content: [] }));
    render(<PresentView response={presented} data={refusing} />);
    expect(
      await screen.findByRole('heading', { name: 'Six-component catalog view', level: 2 }),
    ).toBeTruthy();
    expect(screen.getByText(presented.view.description)).toBeTruthy();
    // The composition's own Stack title is still there, under the View's.
    expect(screen.getByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
  });
});

describe('a dataset the server could not produce says why under its table', () => {
  it("shows the dataset_unavailable warning's message, as sent, in the table's own alert", async () => {
    const presented = zPresentResponse.parse(presentResponseFixture);
    const message = "binding metrics: the source's shown text is not the converter's";
    const unproduced = {
      ...presented,
      resolved_bindings: presented.resolved_bindings.map(
        ({ materialized: _dropped, ...binding }) => binding,
      ),
      charts: [
        {
          chart: { kind: 'named' as const, name: 'metrics_chart' },
          status: {
            status: 'failed' as const,
            reason: 'dataset_unavailable' as const,
            binding: 'metrics',
            message: "the source's shown text is not the converter's",
          },
        },
      ],
      warnings: [{ code: 'dataset_unavailable', message, location: '/bindings/1' }],
    };
    const data = serverToolData(async () => ({
      structuredContent: zReadItemResponse.parse(sourceReadItemFixture),
    }));
    const { container } = render(<PresentView response={unproduced} data={data} />);
    const columns = await waitFor(() => {
      const found = container.querySelector('.view-columns');
      if (found === null) throw new Error('the columns are not drawn yet');
      return found as HTMLElement;
    });
    expect(within(columns).getByRole('alert').textContent).toBe(
      `Dataset unavailable: metrics: ${message}`,
    );
  });
});
