/** Phase 0: json-render catalog Spec round-trip and empty-binding render. */

import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { BindingsContext, type ResolvedPresentation } from '../../src/features/views/Bindings';
import { Layout, prepareSpec } from '../../src/features/views/Layout';
import sixComponentSpec from '../fixtures/six-component-spec.json';

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

/** Canonical comparison shape: root + element type/props/children/slots only. */
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

describe('json-render catalog round-trip', () => {
  it('prepares, JSON round-trips, and renders all six catalog components', () => {
    const first = prepareSpec(sixComponentSpec);
    expect(first.ok).toBe(true);
    if (!first.ok) return;

    const types = Object.values(first.spec.elements).map((element) => element.type);
    for (const catalogType of CATALOG_TYPES) {
      expect(types).toContain(catalogType);
    }
    expect(first.spec.elements.excerpt?.props).toEqual({ binding: 'venue' });
    expect(first.spec.elements.table?.props).toEqual({ binding: 'metrics' });
    expect(first.spec.elements.chart?.props).toEqual({
      binding: 'metrics',
      title: 'Metrics chart',
    });
    expect(first.spec.elements.sources?.props).toEqual({
      bindings: ['venue', 'metrics'],
    });
    expect(first.spec.elements.root?.children).toEqual(['columns', 'chart', 'sources']);
    expect(first.spec.elements.root?.slots).toEqual({
      default: ['columns', 'chart', 'sources'],
    });
    expect(first.spec.elements.columns?.slots).toEqual({
      default: ['excerpt', 'table'],
    });
    expect(first.spec.elements.excerpt?.children).toEqual([]);
    expect(first.spec.elements.table?.children).toEqual([]);
    expect(first.spec.elements.chart?.children).toEqual([]);
    expect(first.spec.elements.sources?.children).toEqual([]);

    const roundTripped = prepareSpec(JSON.parse(JSON.stringify(first.spec)));
    expect(roundTripped.ok).toBe(true);
    if (!roundTripped.ok) return;
    expect(canonicalShape(roundTripped.spec)).toEqual(canonicalShape(first.spec));

    render(
      <BindingsContext.Provider value={emptyBindings}>
        <Layout spec={sixComponentSpec} />
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
    expect(
      alerts.some((node) =>
        /Resolved chart data or specification unavailable/.test(node.textContent ?? ''),
      ),
    ).toBe(true);

    const list = screen.getByRole('list');
    expect(within(list).getByText('Unresolved: venue')).toBeTruthy();
    expect(within(list).getByText('Unresolved: metrics')).toBeTruthy();
  });
});
