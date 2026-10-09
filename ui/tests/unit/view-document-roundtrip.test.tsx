/**
 * Phase 0: ViewDocument Zod + prepareSpec + render equivalence for the six-component catalog.
 *
 * The fixture is rendered twice. With nothing resolved, each data component shows its
 * unavailable state. With the data each component needs (the committed source, dataset and
 * bindings fixtures, resolved as PresentView resolves them), each of the six draws what only a
 * real render of that data produces: a row's cells, a chart mark per row, the source's words.
 */

import { render, screen, waitFor, within } from '@testing-library/react';
import type { TopLevelSpec } from 'vega-lite';
import { describe, expect, it } from 'vitest';
import metricsDatasetFixture from '../../../tests/fixtures/views/present-metrics.dataset.json';
import presentResponseFixture from '../../../tests/fixtures/views/present-response.json';
import sourceReadItemFixture from '../../../tests/fixtures/views/source-read-item.json';
import viewDocumentFixture from '../../../tests/fixtures/views/view-document-six-component.json';
import {
  zDataset,
  zPresentResponse,
  zReadItemResponse,
  zViewDocument,
} from '../../src/api/generated/zod.gen';
import { BindingsContext, type ResolvedPresentation } from '../../src/features/views/Bindings';
import { Layout, prepareSpec } from '../../src/features/views/Layout';
import { datasetRecords, PresentView, parseDataset } from '../../src/features/views/PresentView';

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
    const callTool = async () => ({
      structuredContent: zReadItemResponse.parse(sourceReadItemFixture),
    });
    render(<PresentView response={refused} callTool={callTool} />);
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe(message));
  });
});

/** PresentView resolving one dataset whose bytes are exactly `payload`, as the host serves them. */
async function presentServing(payload: unknown) {
  const bytes = new TextEncoder().encode(JSON.stringify(payload));
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
  const fixture = zPresentResponse.parse(presentResponseFixture);
  const metrics = fixture.resolved_bindings.find((entry) => entry.name === 'metrics');
  const response = zPresentResponse.parse({
    ...presentResponseFixture,
    resolved_bindings: [{ ...metrics, materialized: digest }],
  });
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  const callTool = async (name: string) =>
    name === 'show'
      ? { structuredContent: zReadItemResponse.parse(sourceReadItemFixture) }
      : {
          structuredContent: {
            data_base64: btoa(binary),
            has_more: false,
            media_type: 'application/json',
            offset: '0',
            sha256: digest,
            total_size: String(bytes.byteLength),
          },
        };
  return render(<PresentView response={response} callTool={callTool} />);
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
      const alert = await screen.findByRole('alert');
      expect(alert.textContent).toMatch(message);
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
    const records = parseDataset(JSON.stringify(withNull));
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
