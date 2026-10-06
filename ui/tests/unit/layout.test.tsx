/** Layout normalize → validate pipeline and PresentView render path. */

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { z } from 'zod';
import type { zPresentResponse } from '../../src/api/generated/zod.gen';
import { BindingsContext, type ResolvedPresentation } from '../../src/features/views/Bindings';
import { Layout, materializeSlots, prepareSpec } from '../../src/features/views/Layout';
import { PresentView } from '../../src/features/views/PresentView';

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
      warnings: [],
      receipt_id: 'rcpt_test',
    } satisfies z.infer<typeof zPresentResponse>;
    render(<PresentView response={response} callTool={async () => ({ structuredContent: {} })} />);
    expect(await screen.findByRole('heading', { name: 'Workshop' })).toBeTruthy();
  });

  it('fills charts from view.charts and renders svg without unavailable alert', async () => {
    const rows = [{ category: 'a', value: 1 }];
    const payload = new TextEncoder().encode(JSON.stringify(rows));
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
    const { container } = render(<PresentView response={response} callTool={callTool} />);
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
      spec: { root: 'root', elements: { root: { type: 'Stack', props: {}, children: [] } } },
    },
    resolved_bindings: [
      { name: 'metrics', source, units: {}, transforms: [], materialized: digest },
    ],
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
    render(<PresentView response={response} callTool={callTool} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe('object is not retained by this source');
    expect(alert.textContent).not.toMatch(/invalid_type|expected|"path"/i);
  });

  it('shows a fixed plain sentence when the refusal carries no text', async () => {
    render(<PresentView response={response} callTool={refusing({ isError: true, content: [] })} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe('The host refused to read the dataset.');
  });

  it('bounds a long refusal text to 512 characters ending with an ellipsis', async () => {
    const callTool = refusing({
      isError: true,
      content: [{ type: 'text', text: 'x'.repeat(10_000) }],
    });
    render(<PresentView response={response} callTool={callTool} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(`${'x'.repeat(511)}…`);
  });

  it('cuts a long refusal on a character boundary, never inside a surrogate pair', async () => {
    const callTool = refusing({
      isError: true,
      content: [{ type: 'text', text: '\u{1F4A5}'.repeat(10_000) }],
    });
    render(<PresentView response={response} callTool={callTool} />);
    const shown = (await screen.findByRole('alert')).textContent ?? '';
    expect(shown.length).toBeLessThanOrEqual(512);
    expect(shown.endsWith('…')).toBe(true);
    expect(() => encodeURIComponent(shown)).not.toThrow();
  });
});
