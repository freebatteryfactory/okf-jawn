/** A1: available representations and truthful source locations, from SPEC sections 5 and 7. */
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Locations } from '../../src/features/reader/Locations';
import { pageWindows } from '../../src/features/reader/ReaderViews';
import { viewsOf } from '../../src/features/reader/views';
import { mockApi, ok } from './support/api';
import { appearance, converter, sourceDocument, sourceRead } from './support/m4a-reader';
import { explorerHandlers, NOTE, renderAt, SOURCE, WS } from './support/workspace';

describe('the reader representations an item has', () => {
  it('offers all five for a converted paginated source with page renders', () => {
    expect(viewsOf({ kind: 'source', source: appearance })).toEqual({
      offered: ['text', 'outline', 'multimodal', 'pages', 'original'],
      refused: [],
    });
  });

  it.each(['note', 'view'] as const)('offers only text and outline for a %s', (kind) => {
    expect(viewsOf({ kind, source: null }).offered).toEqual(['text', 'outline']);
  });

  it.each(['pending', 'failed', 'unsupported'] as const)(
    'keeps the original readable when conversion is %s',
    (status) => {
      const outcome =
        status === 'failed'
          ? { status, reason: { kind: 'damaged' as const }, issues: [] }
          : { status };
      const source = {
        ...appearance,
        extraction: { ...appearance.extraction, digest: null, outcome },
      };
      expect(viewsOf({ kind: 'source', source }).offered).toEqual(['text', 'outline', 'original']);
    },
  );

  it('does not promise page renders a conversion did not retain', () => {
    const source = {
      ...appearance,
      extraction: {
        ...appearance.extraction,
        converter: { ...converter, settings: { ...converter.settings, page_images: false } },
      },
    };
    expect(viewsOf({ kind: 'source', source }).offered).toEqual([
      'text',
      'outline',
      'multimodal',
      'original',
    ]);
  });

  it('keeps document headings below the reader title without skipping levels', async () => {
    const api = mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    await within(article).findByText('Text of Meeting notes');
    expect(within(article).getByRole('heading', { name: 'Meeting notes', level: 2 })).toBeTruthy();
    expect(within(article).getByRole('heading', { name: 'Meeting notes', level: 3 })).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('waits for the outline entry selection before moving focus past a cached whole-item text read', async () => {
    // The old Text answer is cached before Outline is selected. Its first part is not the
    // selected section, so it cannot consume the pending focus request (A1 keyboard journey).
    const api = mockApi(
      explorerHandlers({
        '/api/items/get-item': () => ok(sourceDocument()),
        '/api/reads/read-item': (body) =>
          ok(
            sourceRead(body, {
              markdown:
                body.view === 'text'
                  ? body.selection &&
                    typeof body.selection === 'object' &&
                    'heading' in body.selection
                    ? 'Selected Budget section'
                    : 'Whole report before choosing its outline'
                  : '',
              outline: [
                {
                  kind: 'heading',
                  level: 1,
                  label: 'Budget',
                  selection: { kind: 'section', heading: 'Budget' },
                },
              ],
            }),
          ),
      }),
    );
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const article = await screen.findByRole('article', { name: 'Annual report' });
    await within(article).findByText('Whole report before choosing its outline');
    await userEvent.click(within(article).getByRole('radio', { name: 'Outline' }));
    const outline = await within(article).findByRole('navigation', {
      name: 'Outline of this item',
    });
    const budget = within(outline).getByRole('button', { name: 'Budget' });
    budget.focus();
    await userEvent.keyboard('{Enter}');
    await within(article).findByText('Selected Budget section');
    await waitFor(() => expect(document.activeElement).toBe(article.querySelector('[data-part]')));
    expect(api.violations).toEqual([]);
  });
});

describe('page windows and recorded location provenance', () => {
  it('offers the remaining page renders after the eight-image budget and a bounded way back', () => {
    expect(pageWindows({ kind: 'all' }, 12)).toEqual({
      before: null,
      after: { kind: 'pages', range: { start: 9, end: 12 } },
    });
    expect(pageWindows({ kind: 'pages', range: { start: 9, end: 12 } }, 12)).toEqual({
      before: { kind: 'pages', range: { start: 1, end: 8 } },
      after: null,
    });
  });

  it('labels direct, inferred and unresolved locations differently', () => {
    render(
      <Locations
        label="Source locations"
        locations={[
          { provenance: 'direct', locator: { kind: 'page', page_no: 2 } },
          { provenance: 'inferred', locator: { kind: 'page', page_no: 3 } },
          { provenance: 'unresolved', reason: { kind: 'no_match' } },
        ]}
      />,
    );
    const list = screen.getByRole('list', { name: 'Source locations' });
    expect(
      within(list)
        .getAllByRole('listitem')
        .map((item) => item.textContent),
    ).toEqual([
      'Direct location: page 2, recorded from the document itself',
      'Inferred location, not certain: page 3, found by matching text or layout',
      'Location unknown: the text was not found in the original',
    ]);
  });
});
