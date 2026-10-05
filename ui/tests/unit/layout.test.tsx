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
    // Invalid visibility shape must survive normalization so structural validation can judge it.
    if (result.ok) {
      expect(result.spec.elements.root?.visible).toEqual(visible);
    } else {
      expect(result.error).toMatch(/structural|visibility|visible|validation/i);
    }
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
});
