/** Phase 0: ViewDocument Zod + prepareSpec + render equivalence for the six-component catalog. */

import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import viewDocumentFixture from '../../../tests/fixtures/views/view-document-six-component.json';
import { zViewDocument } from '../../src/api/generated/zod.gen';
import { BindingsContext, type ResolvedPresentation } from '../../src/features/views/Bindings';
import { Layout, prepareSpec } from '../../src/features/views/Layout';

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

describe('ViewDocument json-render round-trip', () => {
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
});
