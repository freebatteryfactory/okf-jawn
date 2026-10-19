/** Layout normalize → validate pipeline and PresentView render path. */

import { render, screen, within } from '@testing-library/react';
import { compile, type TopLevelSpec } from 'vega-lite';
import { describe, expect, it } from 'vitest';
import type { z } from 'zod';
import type { GetObjectRequest, ReadItemRequest } from '../../src/api/generated/types.gen';
import type { zPresentResponse } from '../../src/api/generated/zod.gen';
import { BindingsContext, type ResolvedPresentation } from '../../src/features/views/Bindings';
import type { ViewData } from '../../src/features/views/data';
import { Layout, materializeSlots, prepareSpec } from '../../src/features/views/Layout';
import { PresentView } from '../../src/features/views/PresentView';
import { serverToolData } from '../../src/mcp-apps/view-data';

/** A View's reads over a host double that answers each server tool by its name. */
function tools(callTool: (name: string) => Promise<unknown>): ViewData {
  return serverToolData((call) => callTool(call.name));
}

const emptyBindings: ResolvedPresentation = {
  charts: new Map(),
  sources: new Map(),
  bindings: new Map(),
  tables: new Map(),
};

const validSpec = {
  root: 'root',
  elements: {
    root: {
      type: 'Stack',
      props: { title: 'Workshop' },
      children: ['excerpt', 'sources'],
      slots: { default: ['excerpt', 'sources'] },
    },
    excerpt: {
      type: 'SourceExcerpt',
      props: { binding: 'venue' },
    },
    sources: {
      type: 'SourceList',
      props: { bindings: ['venue'] },
    },
  },
};

/** The converter's `Dataset` for rows of (category, value), as the host serves it. */
function datasetBytes(
  rows: ReadonlyArray<{ category: string; value: number }>,
): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(
    JSON.stringify({
      schema_version: 1,
      source: {
        workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
        item_id: '11111111-2222-4333-8444-555555555555',
        path: 'fixtures/metrics.json',
        revision: '0123456789abcdef0123456789abcdef01234567',
        selection: { kind: 'all' },
      },
      text_origin: 'converter',
      columns: [
        { name: 'category', kind: 'string' },
        { name: 'value', kind: 'integer' },
      ],
      rows: rows.map((row) => [row.category, row.value]),
    }),
  );
}

describe('materializeSlots', () => {
  it('keeps a canonical record of string arrays', () => {
    expect(materializeSlots({ default: ['a', 'b'] }, undefined)).toEqual({
      default: ['a', 'b'],
    });
  });

  it('promotes a string array to the default slot', () => {
    expect(materializeSlots(['a', 'b'], undefined)).toEqual({ default: ['a', 'b'] });
  });

  it('derives default slots from children when slots are absent', () => {
    expect(materializeSlots(undefined, ['a', 'b'])).toEqual({ default: ['a', 'b'] });
  });
});

describe('prepareSpec', () => {
  it('accepts a valid six-component-capable composition', () => {
    const result = prepareSpec(validSpec);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.spec.root).toBe('root');
      expect(result.spec.elements.root?.type).toBe('Stack');
    }
  });

  it('rejects a dangling child reference after normalization', () => {
    const result = prepareSpec({
      root: 'root',
      elements: {
        root: {
          type: 'Stack',
          props: {},
          children: ['missing'],
          slots: { default: ['missing'] },
        },
      },
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toMatch(/structural validation|missing_child/i);
  });

  it('rejects an unknown catalog component', () => {
    const result = prepareSpec({
      root: 'root',
      elements: {
        root: { type: 'NotInCatalog', props: {} },
      },
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.error).toMatch(/catalog/i);
  });
});

describe('Layout', () => {
  it('renders a validated Stack title', () => {
    render(
      <BindingsContext.Provider value={emptyBindings}>
        <Layout spec={validSpec} />
      </BindingsContext.Provider>,
    );
    expect(screen.getByRole('heading', { name: 'Workshop' })).toBeTruthy();
  });

  it('surfaces an explicit error for a dangling slot', () => {
    render(
      <BindingsContext.Provider value={emptyBindings}>
        <Layout
          spec={{
            root: 'root',
            elements: {
              root: {
                type: 'Stack',
                props: {},
                children: [],
                slots: ['ghost'],
              },
            },
          }}
        />
      </BindingsContext.Provider>,
    );
    expect(screen.getByRole('alert').textContent).toMatch(/structural validation|missing_child/i);
  });

  it('rejects on and watch because the catalog has no actions', () => {
    const withOn = prepareSpec({
      root: 'root',
      elements: {
        root: { type: 'Stack', props: {}, on: { click: 'noop' } },
      },
    });
    expect(withOn.ok).toBe(false);
    const withWatch = prepareSpec({
      root: 'root',
      elements: {
        root: { type: 'Stack', props: {}, watch: ['state.x'] },
      },
    });
    expect(withWatch.ok).toBe(false);
  });

  it('passes visible through unchanged for validateSpec', () => {
    const visible = { $state: '/show' };
    const result = prepareSpec({
      root: 'root',
      elements: {
        root: {
          type: 'Stack',
          props: { title: 'Conditional' },
          visible,
        },
      },
    });
    expect(result.ok).toBe(true);
    if (!result.ok) {
      throw new Error(`expected prepareSpec ok; got ${result.error}`);
    }
    expect(result.spec.elements.root?.visible).toEqual(visible);
  });
});

describe('PresentView', () => {
  it('renders a json_render presentation through Layout', async () => {
    const response = {
      view: {
        schema_version: 1,
        title: 'Venue board',
        description: 'Approved layout',
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        spec: validSpec,
      },
      resolved_bindings: [],
      charts: [],
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'rcpt_test',
    } satisfies z.infer<typeof zPresentResponse>;
    render(
      <PresentView response={response} data={tools(async () => ({ structuredContent: {} }))} />,
    );
    expect(await screen.findByRole('heading', { name: 'Workshop' })).toBeTruthy();
  });

  it('fills charts from view.charts and renders svg without unavailable alert', async () => {
    const rows = [{ category: 'a', value: 1 }];
    const payload = datasetBytes(rows);
    const digestBytes = new Uint8Array(await crypto.subtle.digest('SHA-256', payload));
    const digest = Array.from(digestBytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
    const chartSpec = {
      $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
      data: { name: 'metrics' },
      mark: 'bar',
      encoding: {
        x: { field: 'category', type: 'nominal' },
        y: { field: 'value', type: 'quantitative' },
      },
    };
    const source = {
      item_id: '11111111-2222-4333-8444-555555555555',
      path: 'fixtures/metrics.json',
      revision: '0123456789abcdef0123456789abcdef01234567',
      selection: { kind: 'all' as const },
      workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
    };
    const response = {
      view: {
        schema_version: 1,
        title: 'Chart board',
        description: 'Chart with one named Vega-Lite entry',
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        charts: { metrics_chart: chartSpec },
        spec: {
          root: 'root',
          elements: {
            root: {
              type: 'Chart',
              props: {
                binding: 'metrics',
                chart: 'metrics_chart',
                title: 'Metrics chart',
              },
              children: [],
            },
          },
        },
      },
      resolved_bindings: [
        {
          name: 'metrics',
          source,
          units: {},
          transforms: [],
          materialized: digest,
        },
      ],
      charts: [
        {
          chart: { kind: 'named', name: 'metrics_chart' },
          status: { status: 'ready', bindings: ['metrics'] },
        },
      ],
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as z.infer<typeof zPresentResponse>;
    const callTool = async (name: string) => {
      if (name === 'show') {
        return {
          structuredContent: {
            markdown: 'metrics',
            media: [],
            outline: [],
            receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
            source,
            truncated: false,
            view: 'text',
            warnings: [],
          },
        };
      }
      if (name === 'read_object') {
        let binary = '';
        for (const byte of payload) binary += String.fromCharCode(byte);
        return {
          structuredContent: {
            data_base64: btoa(binary),
            has_more: false,
            media_type: 'application/json',
            offset: '0',
            sha256: digest,
            total_size: String(payload.byteLength),
          },
        };
      }
      throw new Error(`unexpected tool ${name}`);
    };
    const { container } = render(<PresentView response={response} data={tools(callTool)} />);
    expect(await screen.findByRole('heading', { name: 'Metrics chart' })).toBeTruthy();
    await expect
      .poll(() => container.querySelector('svg') !== null, { timeout: 15_000 })
      .toBe(true);
    expect(screen.queryByText(/Resolved chart data or specification unavailable/i)).toBeNull();
  });
});

describe('PresentView refused dataset read', () => {
  const digest = 'a'.repeat(64);
  const source = {
    item_id: '11111111-2222-4333-8444-555555555555',
    path: 'fixtures/metrics.json',
    revision: '0123456789abcdef0123456789abcdef01234567',
    selection: { kind: 'all' as const },
    workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
  };
  const response = {
    view: {
      schema_version: 1,
      title: 'Chart board',
      description: 'A binding whose dataset the source refuses to read',
      mode: 'pinned',
      grammar: 'json_render',
      bindings: [],
      spec: {
        root: 'root',
        elements: {
          root: { type: 'DataTable', props: { binding: 'metrics' }, children: [] },
        },
      },
    },
    resolved_bindings: [
      { name: 'metrics', source, units: {}, transforms: [], materialized: digest },
    ],
    charts: [],
    as_of: '2026-10-08T14:03:07.250Z',
    warnings: [],
    receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
  } as z.infer<typeof zPresentResponse>;

  /** `show` succeeds; `read_object` resolves to the given refusal. */
  function refusing(refusal: unknown) {
    return async (name: string) => {
      if (name === 'show') {
        return {
          structuredContent: {
            markdown: 'metrics',
            media: [],
            outline: [],
            receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
            source,
            truncated: false,
            view: 'text',
            warnings: [],
          },
        };
      }
      if (name === 'read_object') return refusal;
      throw new Error(`unexpected tool ${name}`);
    };
  }

  it("shows the tool's own message, not a schema issue dump", async () => {
    const callTool = refusing({
      isError: true,
      content: [{ type: 'text', text: 'object is not retained by this source' }],
    });
    render(<PresentView response={response} data={tools(callTool)} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(
      'Dataset unavailable: metrics: object is not retained by this source',
    );
    expect(alert.textContent).not.toMatch(/invalid_type|expected|"path"/i);
  });

  it('shows a fixed plain sentence when the refusal carries no text', async () => {
    render(
      <PresentView response={response} data={tools(refusing({ isError: true, content: [] }))} />,
    );
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(
      'Dataset unavailable: metrics: The host refused to read the dataset.',
    );
  });

  it('bounds a long refusal text to 512 characters ending with an ellipsis', async () => {
    const callTool = refusing({
      isError: true,
      content: [{ type: 'text', text: 'x'.repeat(10_000) }],
    });
    render(<PresentView response={response} data={tools(callTool)} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(`Dataset unavailable: metrics: ${'x'.repeat(511)}…`);
  });

  it('cuts a long refusal on a character boundary, never inside a surrogate pair', async () => {
    const callTool = refusing({
      isError: true,
      content: [{ type: 'text', text: '\u{1F4A5}'.repeat(10_000) }],
    });
    render(<PresentView response={response} data={tools(callTool)} />);
    const shown = (await screen.findByRole('alert')).textContent ?? '';
    expect(shown.startsWith('Dataset unavailable: metrics: ')).toBe(true);
    expect(shown.length).toBeLessThanOrEqual(512 + 'Dataset unavailable: metrics: '.length);
    expect(shown.endsWith('…')).toBe(true);
    expect(() => encodeURIComponent(shown)).not.toThrow();
  });
  /** Refuses `show` as given; the dataset is refused plainly, to keep the two alerts apart. */
  function showRefused(refusal: unknown) {
    return async (name: string) => {
      if (name === 'show') return refusal;
      return { isError: true, content: [{ type: 'text', text: 'dataset refused' }] };
    };
  }
  const withSource = {
    ...response,
    view: {
      ...response.view,
      spec: {
        root: 'root',
        elements: {
          root: { type: 'Stack', props: {}, children: ['excerpt', 'table'] },
          excerpt: { type: 'SourceExcerpt', props: { binding: 'metrics' }, children: [] },
          table: { type: 'DataTable', props: { binding: 'metrics' }, children: [] },
        },
      },
    },
  } as z.infer<typeof zPresentResponse>;
  async function sourceAlert() {
    const alerts = await screen.findAllByRole('alert');
    await expect.poll(() => screen.getAllByRole('alert').length).toBe(2);
    return alerts.map((alert) => alert.textContent ?? '').find((text) => text.startsWith('Source'));
  }

  it("shows the tool's own message when `show` is refused, beside the table's own alert", async () => {
    const callTool = showRefused({
      isError: true,
      content: [{ type: 'text', text: 'revision is not retained by this source' }],
    });
    render(<PresentView response={withSource} data={tools(callTool)} />);
    expect(await sourceAlert()).toBe(
      'Source binding unavailable: metrics: revision is not retained by this source',
    );
  });

  it('bounds a long `show` refusal to 512 characters ending with an ellipsis', async () => {
    const callTool = showRefused({
      isError: true,
      content: [{ type: 'text', text: 'y'.repeat(10_000) }],
    });
    render(<PresentView response={withSource} data={tools(callTool)} />);
    expect(await sourceAlert()).toBe(`Source binding unavailable: metrics: ${'y'.repeat(511)}…`);
  });

  it('shows a fixed plain sentence when a `show` refusal carries no text', async () => {
    render(
      <PresentView
        response={withSource}
        data={tools(showRefused({ isError: true, content: [] }))}
      />,
    );
    expect(await sourceAlert()).toBe(
      'Source binding unavailable: metrics: The host refused to read the source.',
    );
  });
});

describe('PresentView dataset integrity checks', () => {
  const source = {
    item_id: '11111111-2222-4333-8444-555555555555',
    path: 'fixtures/metrics.json',
    revision: '0123456789abcdef0123456789abcdef01234567',
    selection: { kind: 'all' as const },
    workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
  };
  const rows = [
    { category: 'a', value: 1 },
    { category: 'b', value: 2 },
  ];
  const payload = datasetBytes(rows);
  const chartSpec = {
    $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
    data: { name: 'metrics' },
    mark: 'bar',
    encoding: {
      x: { field: 'category', type: 'nominal' },
      y: { field: 'value', type: 'quantitative' },
    },
  };

  interface Block {
    bytes: Uint8Array;
    offset: number;
    sha256?: string;
    hasMore: boolean;
  }

  async function sha256Hex(bytes: BufferSource) {
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
    return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  }

  function base64(bytes: Uint8Array) {
    let binary = '';
    for (let index = 0; index < bytes.length; index += 8192)
      binary += String.fromCharCode(...bytes.subarray(index, index + 8192));
    return btoa(binary);
  }

  function responseFor(materialized: string) {
    return {
      view: {
        schema_version: 1,
        title: 'Chart board',
        description: 'Dataset integrity',
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        charts: { metrics_chart: chartSpec },
        spec: {
          root: 'root',
          elements: {
            root: {
              type: 'Chart',
              props: { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics chart' },
              children: [],
            },
          },
        },
      },
      resolved_bindings: [{ name: 'metrics', source, units: {}, transforms: [], materialized }],
      charts: [
        {
          chart: { kind: 'named', name: 'metrics_chart' },
          status: { status: 'ready', bindings: ['metrics'] },
        },
      ],
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as z.infer<typeof zPresentResponse>;
  }

  /** `show` succeeds; each `read_object` call returns the next scripted block. */
  function serving(blocks: Block[], materialized: string) {
    let next = 0;
    return async (name: string) => {
      if (name === 'show') {
        return {
          structuredContent: {
            markdown: 'metrics',
            media: [],
            outline: [],
            receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
            source,
            truncated: false,
            view: 'text',
            warnings: [],
          },
        };
      }
      const block = blocks[next];
      next += 1;
      if (!block) throw new Error('the host was asked for more blocks than scripted');
      return {
        structuredContent: {
          data_base64: base64(block.bytes),
          has_more: block.hasMore,
          media_type: 'application/json',
          offset: String(block.offset),
          sha256: block.sha256 ?? materialized,
          total_size: String(payload.byteLength),
        },
      };
    };
  }

  async function renderServing(blocks: Block[], materialized?: string) {
    const digest = materialized ?? (await sha256Hex(payload));
    const view = render(
      <PresentView response={responseFor(digest)} data={tools(serving(blocks, digest))} />,
    );
    return view.container;
  }

  async function expectRefusal(container: HTMLElement, message: string) {
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(message);
    expect(container.querySelector('svg')).toBeNull();
    expect(container.querySelector('table')).toBeNull();
  }

  it('accepts honest blocks, split across two reads', async () => {
    const cut = 7;
    const container = await renderServing([
      { bytes: payload.subarray(0, cut), offset: 0, hasMore: true },
      { bytes: payload.subarray(cut), offset: cut, hasMore: false },
    ]);
    await expect
      .poll(() => container.querySelector('svg') !== null, { timeout: 15_000 })
      .toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('refuses bytes that do not hash to the materialized digest', async () => {
    const tampered = payload.slice();
    tampered[tampered.length - 3] = (tampered[tampered.length - 3] ?? 0) === 49 ? 50 : 49;
    const container = await renderServing([{ bytes: tampered, offset: 0, hasMore: false }]);
    await expectRefusal(container, 'Dataset digest verification failed');
  });

  it('refuses a block that reports a different sha256 than the materialized digest', async () => {
    const container = await renderServing([
      { bytes: payload, offset: 0, hasMore: false, sha256: 'b'.repeat(64) },
    ]);
    await expectRefusal(container, 'Dataset identity or range changed');
  });

  it('refuses a block whose offset is not where the previous block ended', async () => {
    const cut = 7;
    const container = await renderServing([
      { bytes: payload.subarray(0, cut), offset: 0, hasMore: true },
      { bytes: payload.subarray(cut), offset: cut + 1, hasMore: false },
    ]);
    await expectRefusal(container, 'Dataset identity or range changed');
  });

  it('refuses an empty block that claims more data follows', async () => {
    const container = await renderServing([
      { bytes: new Uint8Array(0), offset: 0, hasMore: true },
      { bytes: payload, offset: 0, hasMore: false },
    ]);
    await expectRefusal(container, 'Dataset read made no progress');
  });

  it('refuses more than 4 MiB in total', async () => {
    const mebibyte = 1_048_576;
    const big = new Uint8Array(mebibyte);
    // Four full blocks reach the budget exactly; one more byte exceeds it.
    const blocks: Block[] = [0, 1, 2, 3].map((index) => ({
      bytes: big,
      offset: index * mebibyte,
      hasMore: true,
    }));
    blocks.push({ bytes: new Uint8Array(1), offset: 4 * mebibyte, hasMore: false });
    const container = await renderServing(blocks, 'c'.repeat(64));
    await expectRefusal(container, 'Dataset exceeds the inline display budget');
  });
});

describe('PresentView chart specification validation', () => {
  const source = {
    item_id: '11111111-2222-4333-8444-555555555555',
    path: 'fixtures/metrics.json',
    revision: '0123456789abcdef0123456789abcdef01234567',
    selection: { kind: 'all' as const },
    workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
  };
  const rows = [{ category: 'a', value: 1 }];
  const payload = datasetBytes(rows);
  const validChart = {
    $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
    data: { name: 'metrics' },
    mark: 'bar',
    encoding: {
      x: { field: 'category', type: 'nominal' },
      y: { field: 'value', type: 'quantitative' },
    },
  };
  /** A JSON object with no mark, layer or composition: the wire schema takes it, `compile` throws. */
  const rejectedChart = {
    $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
    data: { name: 'metrics' },
    encoding: {
      x: { field: 'category', type: 'nominal' },
      y: { field: 'value', type: 'quantitative' },
    },
  };

  /** A json_render view that shows the chart named `metrics_chart` of `charts`. */
  async function responseWith(charts: Record<string, unknown>) {
    const digestBytes = new Uint8Array(await crypto.subtle.digest('SHA-256', payload));
    const digest = Array.from(digestBytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
    const response = {
      view: {
        schema_version: 1,
        title: 'Chart board',
        description: 'One named Vega-Lite entry',
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        charts,
        spec: {
          root: 'root',
          elements: {
            root: {
              type: 'Chart',
              props: { binding: 'metrics', chart: 'metrics_chart', title: 'Metrics chart' },
              children: [],
            },
          },
        },
      },
      resolved_bindings: [
        { name: 'metrics', source, units: {}, transforms: [], materialized: digest },
      ],
      charts: Object.keys(charts).map((name) => ({
        chart: { kind: 'named' as const, name },
        status: { status: 'ready' as const, bindings: ['metrics'] },
      })),
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as z.infer<typeof zPresentResponse>;
    return { response, digest };
  }

  /** A host that serves the source and the dataset, and remembers which tools were called. */
  function host(digest: string) {
    const called: string[] = [];
    const callTool = async (name: string) => {
      called.push(name);
      if (name === 'show') {
        return {
          structuredContent: {
            markdown: 'metrics',
            media: [],
            outline: [],
            receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
            source,
            truncated: false,
            view: 'text',
            warnings: [],
          },
        };
      }
      if (name === 'read_object') {
        let binary = '';
        for (const byte of payload) binary += String.fromCharCode(byte);
        return {
          structuredContent: {
            data_base64: btoa(binary),
            has_more: false,
            media_type: 'application/json',
            offset: '0',
            sha256: digest,
            total_size: String(payload.byteLength),
          },
        };
      }
      throw new Error(`unexpected tool ${name}`);
    };
    return { called, callTool };
  }

  it('uses a specification that compile itself rejects, beside one it accepts', () => {
    expect(() => compile(rejectedChart as TopLevelSpec)).toThrow(/^Invalid specification /);
    expect(() => compile(validChart as TopLevelSpec)).not.toThrow();
  });

  /**
   * A board of two charts over one dataset, with text and sources around them: a titled stack, an
   * excerpt, the two charts (`bad_chart`, `good_chart`) and the source list.
   */
  async function board(
    charts: Record<string, unknown>,
    statuses: z.infer<typeof zPresentResponse>['charts'] = [],
    cited: { bad: string; good: string } = { bad: 'metrics', good: 'metrics' },
  ) {
    const { response, digest } = await responseWith(charts);
    const view = {
      ...response.view,
      title: 'Quarterly board View',
      spec: {
        root: 'root',
        elements: {
          root: {
            type: 'Stack',
            props: { title: 'Quarterly board' },
            children: ['excerpt', 'bad', 'good', 'sources'],
          },
          excerpt: { type: 'SourceExcerpt', props: { binding: 'metrics' }, children: [] },
          bad: {
            type: 'Chart',
            props: { binding: cited.bad, chart: 'bad_chart', title: 'Bad chart' },
            children: [],
          },
          good: {
            type: 'Chart',
            props: { binding: cited.good, chart: 'good_chart', title: 'Good chart' },
            children: [],
          },
          sources: { type: 'SourceList', props: { bindings: ['metrics'] }, children: [] },
        },
      },
    };
    return { response: { ...response, view, charts: statuses }, digest };
  }

  /** What must survive any one chart's failure: the text, the source components, the other chart. */
  async function expectEverythingElseRendered(container: HTMLElement) {
    expect(await screen.findByRole('heading', { name: 'Quarterly board' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Bad chart' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Good chart' })).toBeTruthy();
    expect(
      screen.getByText('fixtures/metrics.json @ 0123456789abcdef0123456789abcdef01234567'),
    ).toBeTruthy();
    expect(container.querySelector('article.source-excerpt')).not.toBeNull();
    await expect
      .poll(() => container.querySelector('svg') !== null, { timeout: 15_000 })
      .toBe(true);
  }

  async function renderBoard(
    charts: Record<string, unknown>,
    statuses: z.infer<typeof zPresentResponse>['charts'] = [],
    cited?: { bad: string; good: string },
  ) {
    const { response, digest } = await board(charts, statuses, cited);
    const { callTool } = host(digest);
    return render(<PresentView response={response} data={tools(callTool)} />).container;
  }

  it('isolates a chart specification that compile rejects: its own alert names the chart, the text and the other chart still render', async () => {
    const container = await renderBoard({ bad_chart: rejectedChart, good_chart: validChart });
    await expectEverythingElseRendered(container);
    const alerts = screen.getAllByRole('alert');
    expect(alerts).toHaveLength(1);
    expect(alerts[0]?.textContent).toMatch(
      /^Chart "bad_chart" failed validation: Invalid specification /,
    );
    // The alert sits inside the bad chart's own section, not beside the View.
    expect(alerts[0]?.closest('section')?.querySelector('h3')?.textContent).toBe('Bad chart');
    // The good chart drew exactly once.
    expect(container.querySelectorAll('svg')).toHaveLength(1);
  });

  it('isolates a chart that names a binding the View does not resolve', async () => {
    const container = await renderBoard({ bad_chart: validChart, good_chart: validChart }, [], {
      bad: 'missing',
      good: 'metrics',
    });
    await expectEverythingElseRendered(container);
    const alerts = screen.getAllByRole('alert');
    expect(alerts).toHaveLength(1);
    expect(alerts[0]?.textContent).toBe(
      'Unknown binding "missing": the chart cites a binding this View does not resolve.',
    );
    expect(alerts[0]?.closest('section')?.querySelector('h3')?.textContent).toBe('Bad chart');
  });

  it.each([
    ['dataset_unavailable', 'Dataset unavailable', 'the blob is not retained'],
    ['too_large', 'Dataset too large', 'the dataset has 9000000 rows'],
    ['invalidated', 'Invalidated', 'the source revision was purged'],
    ['invalid_spec', 'Invalid specification', 'mark is missing'],
    ['unknown_binding', 'Unknown binding', 'binding "elsewhere" is not in this View'],
  ] as const)(
    'isolates a chart the server reports as %s: its own alert says why',
    async (reasonCode, label, message) => {
      const container = await renderBoard({ bad_chart: validChart, good_chart: validChart }, [
        {
          chart: { kind: 'named', name: 'bad_chart' },
          status: { status: 'failed', reason: reasonCode, message },
        },
        {
          chart: { kind: 'named', name: 'good_chart' },
          status: { status: 'ready', bindings: ['metrics'] },
        },
      ]);
      await expectEverythingElseRendered(container);
      const alerts = screen.getAllByRole('alert');
      expect(alerts).toHaveLength(1);
      expect(alerts[0]?.textContent).toBe(`${label}: ${message}`);
      expect(alerts[0]?.closest('section')?.querySelector('h3')?.textContent).toBe('Bad chart');
    },
  );

  it('keeps the title and description of a vega_lite View whose one chart the server reports failed', async () => {
    const { response, digest } = await responseWith({});
    const failed = {
      ...response,
      view: {
        ...response.view,
        grammar: 'vega_lite' as const,
        spec: validChart,
        charts: undefined,
      },
      charts: [
        {
          chart: { kind: 'spec' as const },
          status: {
            status: 'failed' as const,
            reason: 'dataset_unavailable' as const,
            message: 'the blob is not retained',
          },
        },
      ],
    } as z.infer<typeof zPresentResponse>;
    const { callTool } = host(digest);
    const { container } = render(<PresentView response={failed} data={tools(callTool)} />);
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Dataset unavailable: the blob is not retained',
    );
    expect(screen.getByRole('heading', { name: 'Chart board' })).toBeTruthy();
    expect(screen.getByText('One named Vega-Lite entry')).toBeTruthy();
    expect(container.querySelector('svg')).toBeNull();
  });

  it('draws the chart of the same view when its specification is one compile accepts', async () => {
    const { response, digest } = await responseWith({ metrics_chart: validChart });
    const { called, callTool } = host(digest);
    const { container } = render(<PresentView response={response} data={tools(callTool)} />);
    expect(await screen.findByRole('heading', { name: 'Metrics chart' })).toBeTruthy();
    await expect
      .poll(() => container.querySelector('svg') !== null, { timeout: 15_000 })
      .toBe(true);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(called).toEqual(['show', 'read_object']);
  });
});

describe('PresentView unexpected failure', () => {
  it('shows an alert instead of resolving forever when something outside a binding throws', async () => {
    const response = {
      view: {
        schema_version: 1,
        title: 'Board',
        description: 'd',
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        spec: { root: 'root', elements: { root: { type: 'Stack', props: {}, children: [] } } },
      },
      get resolved_bindings(): never {
        throw new Error('the response could not be walked');
      },
      charts: [],
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as unknown as z.infer<typeof zPresentResponse>;
    render(<PresentView response={response} data={tools(async () => ({}))} />);
    expect((await screen.findByRole('alert')).textContent).toBe('the response could not be walked');
    expect(screen.queryByRole('status')).toBeNull();
  });
});

describe('PresentView reads through the ViewData it is given', () => {
  const metricsSource = {
    item_id: '11111111-2222-4333-8444-555555555555',
    path: 'fixtures/metrics.json',
    revision: '0123456789abcdef0123456789abcdef01234567',
    selection: { kind: 'all' as const },
    workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
  };
  const reportSource = {
    item_id: '33333333-2222-4333-8444-555555555555',
    path: 'reports/pipeline.pdf',
    revision: '89abcdef0123456789abcdef0123456789abcdef',
    selection: { kind: 'pages' as const, range: { start: 3, end: 3 } },
    workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
  };
  const payload = datasetBytes([
    { category: 'a', value: 1 },
    { category: 'b', value: 2 },
  ]);
  const cut = 7;

  async function digestOf(bytes: BufferSource) {
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
    return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  }
  function base64(bytes: Uint8Array) {
    let binary = '';
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary);
  }
  /** A View citing a page of a report (an excerpt) and a dataset (a table). */
  function viewOver(materialized: string) {
    return {
      view: {
        schema_version: 1,
        title: 'Board',
        description: 'A source and a table',
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        spec: {
          root: 'root',
          elements: {
            root: { type: 'Stack', props: { title: 'Board' }, children: ['excerpt', 'table'] },
            excerpt: { type: 'SourceExcerpt', props: { binding: 'report' }, children: [] },
            table: { type: 'DataTable', props: { binding: 'metrics' }, children: [] },
          },
        },
      },
      resolved_bindings: [
        { name: 'report', source: reportSource, units: {}, transforms: [] },
        { name: 'metrics', source: metricsSource, units: {}, transforms: [], materialized },
      ],
      charts: [],
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as z.infer<typeof zPresentResponse>;
  }
  /** The source each binding cites, read as the read answers it, with `markdown` as its text. */
  function answer(cited: typeof reportSource | typeof metricsSource, markdown: string) {
    return {
      markdown,
      media: [],
      outline: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
      source: cited,
      truncated: false,
      view: 'text' as const,
      warnings: [],
    };
  }
  /** A ViewData that answers each read and records what it was asked. */
  function recording(digest: string) {
    const asked = { readItem: [] as ReadItemRequest[], getObject: [] as GetObjectRequest[] };
    const data: ViewData = {
      readItem: async (request) => {
        asked.readItem.push(request);
        return request.item_id === reportSource.item_id
          ? answer(reportSource, 'Page three of the report.')
          : answer(metricsSource, 'metrics');
      },
      getObject: async (request) => {
        asked.getObject.push(request);
        const start = Number(request.offset);
        const bytes = start === 0 ? payload.subarray(0, cut) : payload.subarray(cut);
        return {
          data_base64: base64(bytes),
          has_more: start === 0,
          media_type: 'application/json',
          offset: String(start),
          sha256: digest,
          total_size: String(payload.byteLength),
        };
      },
    };
    return { asked, data };
  }

  it('asks for each cited source at its revision with its selection, and a dataset in blocks of 1 MiB from offset 0', async () => {
    const digest = await digestOf(payload);
    const { asked, data } = recording(digest);
    render(<PresentView response={viewOver(digest)} data={data} />);
    expect(await screen.findByText('Page three of the report.')).toBeTruthy();
    expect(await screen.findByRole('table', { name: 'metrics' })).toBeTruthy();
    expect(asked.readItem).toStrictEqual(
      [reportSource, metricsSource].map((cited) => ({
        workspace_id: cited.workspace_id,
        item_id: cited.item_id,
        at: { kind: 'revision', revision: cited.revision },
        view: 'text',
        selection: cited.selection,
        max_bytes: 65536,
        max_images: 0,
      })),
    );
    expect(asked.getObject).toStrictEqual([
      { source: metricsSource, object: digest, offset: '0', length: 1048576 },
      { source: metricsSource, object: digest, offset: String(cut), length: 1048576 },
    ]);
  });

  it('refuses a source a provider answers at another revision: that excerpt says so, the table still draws', async () => {
    const digest = await digestOf(payload);
    const { data } = recording(digest);
    const elsewhere: ViewData = {
      ...data,
      readItem: async (request) =>
        request.item_id === reportSource.item_id
          ? answer({ ...reportSource, revision: 'f'.repeat(40) }, 'Another revision.')
          : data.readItem(request),
    };
    render(<PresentView response={viewOver(digest)} data={elsewhere} />);
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Source binding unavailable: report: Returned source revision differs from the binding',
    );
    expect(await screen.findByRole('table', { name: 'metrics' })).toBeTruthy();
    expect(screen.queryByText('Another revision.')).toBeNull();
  });

  it('refuses an answer the generated schema does not read, whatever the provider', async () => {
    const digest = await digestOf(payload);
    const { data } = recording(digest);
    // Each answer is right in everything the later checks read (the revision, the object, the
    // range), and wrong in one field only the schema reads.
    const malformed: ViewData = {
      readItem: async (request) =>
        request.item_id === reportSource.item_id
          ? JSON.parse(JSON.stringify({ ...answer(reportSource, 'text'), markdown: 7 }))
          : data.readItem(request),
      getObject: async (request) =>
        JSON.parse(JSON.stringify({ ...(await data.getObject(request)), data_base64: 7 })),
    };
    render(<PresentView response={viewOver(digest)} data={malformed} />);
    await expect.poll(() => screen.queryAllByRole('alert').length).toBe(2);
    const [source, table] = screen.getAllByRole('alert').map((alert) => alert.textContent ?? '');
    expect(source).toMatch(/^Source binding unavailable: report: .*"markdown"/s);
    expect(table).toMatch(/^Dataset unavailable: metrics: .*"data_base64"/s);
    expect(screen.queryByRole('table')).toBeNull();
  });
});

describe('a chart draws every binding its ready status names', () => {
  const workspace = 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff';
  const revision = '0123456789abcdef0123456789abcdef01234567';
  const cite = (item: string, path: string) => ({
    item_id: item,
    path,
    revision,
    selection: { kind: 'all' as const },
    workspace_id: workspace,
  });
  const cited = {
    metrics: cite('11111111-2222-4333-8444-555555555555', 'fixtures/metrics.json'),
    targets: cite('22222222-2222-4333-8444-555555555555', 'fixtures/targets.json'),
    venues: cite('33333333-2222-4333-8444-555555555555', 'fixtures/venues.json'),
  };
  const rows = {
    metrics: [
      ['a', 1],
      ['b', 2],
    ],
    targets: [
      ['a', 3],
      ['b', 4],
      ['c', 5],
    ],
    venues: [
      ['a', 1],
      ['b', 2],
    ],
  };
  /** The converter's Dataset of one binding, as its bytes. */
  const bytesOf = (name: keyof typeof cited) =>
    new TextEncoder().encode(
      JSON.stringify({
        schema_version: 1,
        source: cited[name],
        text_origin: 'converter',
        columns: [
          { name: 'category', kind: 'string' },
          { name: 'value', kind: 'integer' },
        ],
        rows: rows[name],
      }),
    );
  async function digestOf(bytes: BufferSource) {
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
    return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  }
  const layer = (name: string, mark: string) => ({
    data: { name },
    mark,
    encoding: {
      x: { field: 'category', type: 'nominal' },
      y: { field: 'value', type: 'quantitative' },
    },
  });
  const layered = {
    $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
    layer: [layer('metrics', 'bar'), layer('targets', 'point')],
  };

  /**
   * A board of one chart whose server status reads both bindings (or the ones named): a
   * json_render Chart citing `metrics`, or a vega_lite View whose spec is the layered chart.
   */
  async function board(
    grammar: 'json_render' | 'vega_lite' = 'json_render',
    {
      chart = layered,
      transforms = {},
      names = ['metrics', 'targets'],
    }: {
      chart?: unknown;
      transforms?: Record<string, unknown[]>;
      names?: ReadonlyArray<keyof typeof cited>;
    } = {},
  ) {
    const served = new Map<string, Uint8Array>();
    const digests: Record<string, string> = {};
    for (const name of names) {
      const bytes = bytesOf(name);
      digests[name] = await digestOf(bytes);
      served.set(digests[name], bytes);
    }
    const response = {
      view: {
        schema_version: 1,
        title: 'Layered board',
        description: 'One chart over two bindings',
        mode: 'pinned',
        grammar,
        bindings: [],
        ...(grammar === 'vega_lite'
          ? { spec: chart }
          : {
              charts: { both: chart },
              spec: {
                root: 'root',
                elements: {
                  root: {
                    type: 'Chart',
                    props: { binding: 'metrics', chart: 'both', title: 'Both bindings' },
                    children: [],
                  },
                },
              },
            }),
      },
      resolved_bindings: names.map((name) => ({
        name,
        source: cited[name],
        units: {},
        transforms: transforms[name] ?? [],
        materialized: digests[name],
      })),
      charts: [
        {
          chart: grammar === 'vega_lite' ? { kind: 'spec' } : { kind: 'named', name: 'both' },
          status: { status: 'ready', bindings: [...names] },
        },
      ],
      as_of: '2026-10-08T14:03:07.250Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as z.infer<typeof zPresentResponse>;
    const data: ViewData = {
      readItem: async (request) => ({
        markdown: 'source',
        media: [],
        outline: [],
        receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
        source:
          Object.values(cited).find((source) => source.item_id === request.item_id) ??
          cited.metrics,
        truncated: false,
        view: 'text',
        warnings: [],
      }),
      getObject: async (request) => {
        const bytes = served.get(request.object) ?? new Uint8Array();
        let binary = '';
        for (const byte of bytes) binary += String.fromCharCode(byte);
        return {
          data_base64: btoa(binary),
          has_more: false,
          media_type: 'application/json',
          offset: '0',
          sha256: request.object,
          total_size: String(bytes.byteLength),
        };
      },
    };
    return render(<PresentView response={response} data={data} />).container;
  }

  it('draws a layered chart over two bindings with both, and tables both', {
    timeout: 30_000,
  }, async () => {
    const container = await board();
    const marks = () =>
      Array.from(
        container.querySelectorAll('svg g[class~="role-mark"]'),
        (group) => group.children.length,
      );
    await expect.poll(marks, { timeout: 15_000 }).toEqual([2, 3]);
    expect(screen.queryByRole('alert')).toBeNull();
    const chart = screen.getByRole('heading', { name: 'Both bindings' }).closest('section');
    if (chart === null) throw new Error('the chart has no section');
    expect(within(chart).getByRole('table', { name: 'metrics' })).toBeTruthy();
    expect(within(chart).getByRole('table', { name: 'targets' })).toBeTruthy();
  });

  /** The number of marks each mark group of the drawn chart holds, once it is drawn. */
  const drawn = (container: HTMLElement) => () =>
    Array.from(
      container.querySelectorAll('svg g[class~="role-mark"]'),
      (group) => group.children.length,
    );

  it("puts a binding's transforms before the chart's own, in the layer that reads it only", {
    timeout: 30_000,
  }, async () => {
    // `metrics` is (a, 1), (b, 2). Its own filter keeps one row; had the layer's calculate run
    // first (10, 20), it would keep both. `targets` has no transforms: its three points stay.
    const ordered = {
      ...layered,
      layer: [
        { ...layer('metrics', 'bar'), transform: [{ calculate: 'datum.value * 10', as: 'value' }] },
        layer('targets', 'point'),
      ],
    };
    const container = await board('json_render', {
      chart: ordered,
      transforms: { metrics: [{ filter: 'datum.value > 1' }] },
    });
    await expect.poll(drawn(container), { timeout: 15_000 }).toEqual([1, 3]);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it("puts a binding's transforms before a vega_lite View's own", { timeout: 30_000 }, async () => {
    const single = {
      $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
      ...layer('metrics', 'bar'),
      transform: [{ calculate: 'datum.value * 10', as: 'value' }],
    };
    const container = await board('vega_lite', {
      chart: single,
      transforms: { metrics: [{ filter: 'datum.value > 1' }] },
    });
    await expect.poll(drawn(container), { timeout: 15_000 }).toEqual([1]);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  /** The text each text mark of the drawn chart shows, in order, once there is any. */
  const texts = (container: HTMLElement) => () =>
    Array.from(
      container.querySelectorAll('svg g[class~="role-mark"] text'),
      (text) => text.textContent,
    );
  /** One text mark per `metrics` row, showing the field its transforms looked up. */
  const showing = (field: string) => ({
    $schema: 'https://vega.github.io/schema/vega-lite/v6.json',
    data: { name: 'metrics' },
    mark: 'text',
    encoding: {
      y: { field: 'category', type: 'nominal' },
      text: { field, type: 'quantitative' },
    },
  });
  const lookUp = (binding: string, field: string) => ({
    lookup: 'category',
    from: { data: { name: binding }, key: 'category', fields: [field] },
  });

  it("looks up a binding after that binding's own transforms", { timeout: 30_000 }, async () => {
    // `targets` is (a, 3), (b, 4), (c, 5); its own calculate makes `scaled` 300, 400, 500. A
    // lookup of the raw dataset would find no `scaled` at all.
    const container = await board('vega_lite', {
      chart: showing('scaled'),
      transforms: {
        metrics: [lookUp('targets', 'scaled')],
        targets: [{ calculate: 'datum.value * 100', as: 'scaled' }],
      },
    });
    await expect.poll(texts(container), { timeout: 15_000 }).toEqual(['300', '400']);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('runs looked-up transforms in dependency order, through a chain of lookups', {
    timeout: 30_000,
  }, async () => {
    // `venues` (a, 1), (b, 2) makes `factor` 10, 20; `targets` looks that up and makes
    // `scaled` 3 * 10 = 30 and 4 * 20 = 80; `metrics` looks `scaled` up. Had `targets`'
    // transforms run on the raw `venues`, `factor` would be missing and `scaled` not a number.
    const container = await board('json_render', {
      chart: showing('scaled'),
      names: ['metrics', 'targets', 'venues'],
      transforms: {
        metrics: [lookUp('targets', 'scaled')],
        targets: [
          lookUp('venues', 'factor'),
          { calculate: 'datum.value * datum.factor', as: 'scaled' },
        ],
        venues: [{ calculate: 'datum.value * 10', as: 'factor' }],
      },
    });
    await expect.poll(texts(container), { timeout: 15_000 }).toEqual(['30', '80']);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it("refuses, in that chart's alert, bindings whose transforms look each other up", {
    timeout: 30_000,
  }, async () => {
    await board('json_render', {
      chart: showing('scaled'),
      transforms: {
        metrics: [lookUp('targets', 'scaled')],
        targets: [lookUp('metrics', 'value'), { calculate: 'datum.value * 100', as: 'scaled' }],
      },
    });
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/metrics → targets → metrics/);
    expect(alert.textContent).toMatch(/look each other up/);
    // The chart's own alert, in its section, not the View's.
    const chart = screen.getByRole('heading', { name: 'Both bindings' }).closest('section');
    if (chart === null) throw new Error('the chart has no section');
    expect(within(chart).getByRole('alert')).toBe(alert);
  });

  it('draws a vega_lite View layered over two bindings with both, and tables both', {
    timeout: 30_000,
  }, async () => {
    const container = await board('vega_lite');
    const marks = () =>
      Array.from(
        container.querySelectorAll('svg g[class~="role-mark"]'),
        (group) => group.children.length,
      );
    await expect.poll(marks, { timeout: 15_000 }).toEqual([2, 3]);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('table', { name: 'metrics' })).toBeTruthy();
    expect(screen.getByRole('table', { name: 'targets' })).toBeTruthy();
  });
});

describe('PresentView shows only what was read for the response and the reads it is given', () => {
  const workspace = 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff';
  const citeOf = (item: string, revision: string) => ({
    item_id: item,
    path: `fixtures/${item.slice(0, 4)}.json`,
    revision,
    selection: { kind: 'all' as const },
    workspace_id: workspace,
  });
  const cited = {
    a: citeOf('11111111-2222-4333-8444-555555555555', 'a'.repeat(40)),
    b: citeOf('33333333-2222-4333-8444-555555555555', 'b'.repeat(40)),
  };
  /** The converter's Dataset of one source: one row whose category names the source. */
  const bytesOf = (name: keyof typeof cited) =>
    new TextEncoder().encode(
      JSON.stringify({
        schema_version: 1,
        source: cited[name],
        text_origin: 'converter',
        columns: [
          { name: 'category', kind: 'string' },
          { name: 'value', kind: 'integer' },
        ],
        rows: [[`row of ${name.toUpperCase()}`, 1]],
      }),
    );
  async function digestOf(bytes: BufferSource) {
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
    return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
  }
  /** A View of one source excerpt and one table over the source named, at its own as-of time. */
  function viewOf(name: keyof typeof cited, digest: string) {
    return {
      view: {
        schema_version: 1,
        title: `Board ${name.toUpperCase()}`,
        description: `The View of ${name.toUpperCase()}`,
        mode: 'pinned',
        grammar: 'json_render',
        bindings: [],
        spec: {
          root: 'root',
          elements: {
            root: { type: 'Stack', props: {}, children: ['excerpt', 'table'] },
            excerpt: { type: 'SourceExcerpt', props: { binding: 'metrics' }, children: [] },
            table: { type: 'DataTable', props: { binding: 'metrics' }, children: [] },
          },
        },
      },
      resolved_bindings: [
        { name: 'metrics', source: cited[name], units: {}, transforms: [], materialized: digest },
      ],
      charts: [],
      as_of: name === 'a' ? '2026-10-08T14:03:07.250Z' : '2026-10-09T09:00:00.000Z',
      warnings: [],
      receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    } as z.infer<typeof zPresentResponse>;
  }
  /**
   * A ViewData serving both sources. A read of a source in `held` waits until `release` is called,
   * so a test can look at the page while it is pending.
   */
  function serving(digests: Record<keyof typeof cited, string>, held: Set<string> = new Set()) {
    let release: () => void = () => {};
    const gate = new Promise<void>((done) => {
      release = done;
    });
    const nameOf = (item: string) => (item === cited.b.item_id ? 'b' : 'a');
    const data: ViewData = {
      readItem: async (request) => {
        const name = nameOf(request.item_id);
        if (held.has(name)) await gate;
        return {
          markdown: `Words of source ${name.toUpperCase()}.`,
          media: [],
          outline: [],
          receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
          source: cited[name],
          truncated: false,
          view: 'text',
          warnings: [],
        };
      },
      getObject: async (request) => {
        const name = request.object === digests.b ? 'b' : 'a';
        if (held.has(name)) await gate;
        const bytes = bytesOf(name);
        let binary = '';
        for (const byte of bytes) binary += String.fromCharCode(byte);
        return {
          data_base64: btoa(binary),
          has_more: false,
          media_type: 'application/json',
          offset: '0',
          sha256: request.object,
          total_size: String(bytes.byteLength),
        };
      },
    };
    return { data, release };
  }
  async function digests() {
    return { a: await digestOf(bytesOf('a')), b: await digestOf(bytesOf('b')) };
  }
  /** Nothing read for A is on the page. */
  function expectNothingOfA() {
    expect(screen.queryByText('Words of source A.')).toBeNull();
    expect(screen.queryByText('row of A')).toBeNull();
  }

  it("never shows A's sources and table under B's title and as-of time while B's reads are pending", async () => {
    const both = await digests();
    const { data, release } = serving(both, new Set(['b']));
    const view = render(<PresentView response={viewOf('a', both.a)} data={data} />);
    expect(await screen.findByText('Words of source A.')).toBeTruthy();
    expect(screen.getByText('row of A')).toBeTruthy();

    view.rerender(<PresentView response={viewOf('b', both.b)} data={data} />);
    // B's reads are held: what was read for A is gone at once, and nothing of B's is claimed.
    expectNothingOfA();
    expect(screen.getByRole('status').textContent).toMatch(/Resolving/);
    expect(screen.queryByRole('heading', { name: 'Board B' })).toBeNull();
    expect(screen.queryByRole('heading', { name: 'Board A' })).toBeNull();

    release();
    expect(await screen.findByText('Words of source B.')).toBeTruthy();
    expect(screen.getByText('row of B')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Board B' })).toBeTruthy();
    expectNothingOfA();
  });

  it('shows the next response once its reads answer, after a failure of the previous one', async () => {
    const both = await digests();
    const { data } = serving(both);
    const broken = {
      ...viewOf('a', both.a),
      get resolved_bindings(): never {
        throw new Error('the response could not be walked');
      },
    } as unknown as z.infer<typeof zPresentResponse>;
    const view = render(<PresentView response={broken} data={data} />);
    expect((await screen.findByRole('alert')).textContent).toBe('the response could not be walked');

    view.rerender(<PresentView response={viewOf('b', both.b)} data={data} />);
    expect(await screen.findByText('Words of source B.')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Board B' })).toBeTruthy();
    expect(screen.queryByText('the response could not be walked')).toBeNull();
  });

  it('reads the same response again through new reads, showing nothing of the old reads meanwhile', async () => {
    const both = await digests();
    const first = serving(both);
    const response = viewOf('a', both.a);
    const view = render(<PresentView response={response} data={first.data} />);
    expect(await screen.findByText('Words of source A.')).toBeTruthy();

    const second = serving(both, new Set(['a']));
    view.rerender(<PresentView response={response} data={second.data} />);
    expectNothingOfA();
    expect(screen.getByRole('status').textContent).toMatch(/Resolving/);

    second.release();
    expect(await screen.findByText('Words of source A.')).toBeTruthy();
    expect(screen.getByText('row of A')).toBeTruthy();
  });
});
