/**
 * Server text is never live HTML: titles, snippets, property values, error messages and
 * Markdown images and links all reach the DOM as text. These are the reviewer's probes P2,
 * adopted; each kills a mutant (R1-R6) that rendered one of them as HTML.
 */

import { render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SearchHit } from '../../src/api/generated/types.gen';
import { SafeMarkdown } from '../../src/features/documents/SafeMarkdown';
import { FailureNotice } from '../../src/features/errors/FailureNotice';
import { mockApi, ok } from './support/api';
import {
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  renderAt,
  WS,
} from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const SEARCH = '/api/search/search-items';
const HOSTILE = '<img src="x" onerror="window.okfProbe = true"><b>bold</b>';

function hit(extra: Partial<SearchHit> = {}): SearchHit {
  return {
    title: 'Meeting notes',
    snippet: '…matching words…',
    score: 1,
    source: {
      workspace_id: WS,
      item_id: NOTE,
      path: 'notes/meeting.md',
      revision: HEAD,
      selection: { kind: 'lines', range: { start: 4, end: 5 } },
      locations: [],
    },
    ...extra,
  };
}

describe('P2 safety: server text is never live HTML', () => {
  it('shows a search snippet and title that contain markup as text', async () => {
    mockApi(
      explorerHandlers({
        [SEARCH]: () => ok({ hits: [hit({ title: HOSTILE, snippet: HOSTILE })], revision: HEAD }),
      }),
    );
    renderAt(`/w/${WS}?q=budget`);
    const results = await screen.findByRole('region', { name: 'Results for “budget”' });
    const link = await within(results).findByRole('link');
    expect(link.querySelector('img, b')).toBeNull();
    expect(link.textContent).toContain('<b>bold</b>');
  });

  it('shows a property value that contains markup as text', async () => {
    mockApi(
      explorerHandlers({
        '/api/items/get-item': () => ok(itemDocument(noteSummary, { note: HOSTILE })),
      }),
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await within(article).findByText('Properties (3)');
    expect(article.querySelector('details img, details b')).toBeNull();
    expect(within(article).getByText(HOSTILE)).toBeTruthy();
  });

  it('shows a server error message that contains markup as text', () => {
    render(<FailureNotice context="Opening" error={{ code: 'internal', message: HOSTILE }} />);
    const alert = screen.getByRole('alert');
    expect(alert.querySelector('img, b')).toBeNull();
    expect(alert.textContent).toContain('<b>bold</b>');
  });

  it('never loads a Markdown image or follows a Markdown link from document text', () => {
    const { container } = render(
      <SafeMarkdown
        markdown={'![pixel](https://evil.example/p.png)\n\n[home](https://evil.example/)'}
      />,
    );
    expect(container.querySelector('img, a, [src], [href]')).toBeNull();
    expect(container.textContent).toContain('[Image: pixel]');
  });

  it('shows an item title that contains markup as text in the tree, tab and heading', async () => {
    const hostile = { ...noteSummary, title: HOSTILE };
    mockApi(
      explorerHandlers({
        '/api/items/list-items': (body) =>
          ok({
            folders: [],
            items: body.folder === '' ? [hostile] : [],
            revision: HEAD,
            next_cursor: null,
          }),
        '/api/items/get-item': () => ok(itemDocument(hostile)),
      }),
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('heading', { level: 2, name: HOSTILE });
    await screen.findByRole('tab', { name: HOSTILE });
    await screen.findByRole('treeitem', { name: /bold/ });
    expect(document.querySelector('.explorer img, .explorer b')).toBeNull();
    expect(Reflect.get(window, 'okfProbe')).toBeUndefined();
  });
});
