/** Reading an item: sanitized Markdown, read-only properties, continuation, extraction state, failures. */

import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mockApi, notImplemented, ok } from './support/api';
import {
  explorerHandlers,
  NOTE,
  noteSummary,
  readResponse,
  renderAt,
  SOURCE,
  sourceSummary,
  WS,
} from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const READ = '/api/reads/read-item';
const hostile = [
  '# Hostile',
  '',
  '<script>window.okfHacked = true</script>',
  '',
  '<img src="x" onerror="window.okfHacked = true">',
  '',
  '<iframe src="https://evil.example/"></iframe>',
  '',
  '[click me](javascript:window.okfHacked=true)',
  '',
  'Plain words with a footnote[^1].',
  '',
  '[^1]: The footnote text.',
].join('\n');

async function openDocument(name = 'Meeting notes') {
  return screen.findByRole('article', { name });
}

describe('reading', () => {
  it('renders Markdown with no raw HTML, script, handler or page-id clobbering', async () => {
    mockApi(explorerHandlers({ [READ]: () => ok(readResponse(noteSummary, hostile)) }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await openDocument();
    const body = await within(article).findByText(/Plain words with a footnote/);
    expect(body).toBeTruthy();
    expect(within(article).getByText('The footnote text.', { exact: false })).toBeTruthy();
    expect(article.querySelector('script, iframe, img, object, embed')).toBeNull();
    expect(article.querySelector('[onerror], [onclick], [href^="javascript"]')).toBeNull();
    expect(article.textContent).not.toContain('okfHacked');
    for (const element of article.querySelectorAll('[id]')) {
      // React's own useId ids belong to the page; document content never sets an id unprefixed.
      if (element.id.startsWith('_r_') || element.id.startsWith(':r')) continue;
      expect(element.id.startsWith('user-content-')).toBe(true);
    }
    expect(Reflect.get(window, 'okfHacked')).toBeUndefined();
  });

  it('shows properties read-only, keeping unknown values and the managed header', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await openDocument();
    const properties = await within(article).findByText('Properties (3)');
    const panel = properties.closest('details');
    if (panel === null) throw new Error('properties are not a details element');
    expect(within(panel).getByText('owner')).toBeTruthy();
    expect(within(panel).getByText('Ada')).toBeTruthy();
    expect(within(panel).getByText('okf_jawn (kept by okf-jawn; not editable)')).toBeTruthy();
    expect(within(panel).queryAllByRole('textbox')).toEqual([]);
    expect(within(panel).queryAllByRole('checkbox')).toEqual([]);
    expect(within(panel).queryAllByRole('button')).toEqual([]);
  });

  it('marks a partial read and continues the same resolved read through its cursor', async () => {
    const api = mockApi(
      explorerHandlers({
        [READ]: (body) =>
          body.cursor === 'part-2'
            ? ok(readResponse(noteSummary, 'Part two of the text'))
            : ok(
                readResponse(noteSummary, 'Part one of the text', {
                  truncated: true,
                  next_cursor: 'part-2',
                }),
              ),
      }),
    );
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await openDocument();
    expect(await within(article).findByText('Part one of the text')).toBeTruthy();
    expect(
      within(article).getByText(/Partial reading: this is not all of the whole item./),
    ).toBeTruthy();
    await userEvent.click(within(article).getByRole('button', { name: 'Continue reading' }));
    expect(await within(article).findByText('Part two of the text')).toBeTruthy();
    expect(within(article).getByText('Part one of the text')).toBeTruthy();
    expect(within(article).queryByText(/Partial reading/)).toBeNull();
    const reads = api.callsTo(READ);
    expect(reads.map((body) => body.cursor)).toEqual([null, 'part-2']);
    expect(reads[1]?.at).toEqual(reads[0]?.at);
    expect(reads[1]?.selection).toEqual(reads[0]?.selection);
    expect(reads[1]?.view).toBe('text');
    expect(api.violations).toEqual([]);
  });

  it('says when a source is partly converted, agent-supplied or corrected', async () => {
    mockApi(
      explorerHandlers({
        [READ]: () =>
          ok(
            readResponse(sourceSummary, 'Converted text', {
              extraction: { status: 'partial', text_origin: 'supplied_by_agent', corrected: true },
            }),
          ),
      }),
    );
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const article = await openDocument('Annual report');
    expect(await within(article).findByText('Partly converted.')).toBeTruthy();
    expect(within(article).getByText(/supplied by an agent and accepted by a person/)).toBeTruthy();
    expect(within(article).getByText(/A person’s correction is shown/)).toBeTruthy();
  });

  it('shows an unbuilt read as not available, with no document text and no fake success', async () => {
    mockApi(explorerHandlers({ [READ]: () => notImplemented('read_item') }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await openDocument();
    const alert = await within(article).findByRole('alert');
    expect(alert.textContent).toContain('Reading this item: Not available in this build.');
    expect(alert.textContent).toContain('read_item is not implemented in this build');
    expect(within(article).queryByText(/Text of Meeting notes/)).toBeNull();
    expect(within(article).queryByText(/Partial reading/)).toBeNull();
  });

  it('keeps reading when only the properties operation is unbuilt', async () => {
    mockApi(explorerHandlers({ '/api/items/get-item': () => notImplemented('get_item') }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    const article = await screen.findByRole('article');
    expect(await within(article).findByText('Text of Meeting notes')).toBeTruthy();
    const alert = await within(article).findByRole('alert');
    expect(alert.textContent).toContain(
      'Reading this item’s properties: Not available in this build.',
    );
  });
});

describe('source panel', () => {
  it('shows each citation, where it lies in the original, and how that place was found', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    const panel = await screen.findByRole('region', { name: 'Source panel' });
    const sources = await within(panel).findByRole('region', { name: 'Sources' });
    expect(within(sources).getByText('Cites pages 2–3.')).toBeTruthy();
    const places = within(sources).getByRole('list', { name: 'Where this is in the original' });
    const items = within(places)
      .getAllByRole('listitem')
      .map((item) => item.textContent);
    expect(items).toEqual([
      'Direct location: page 2, recorded from the document itself',
      'Inferred location, not certain: page 3, found by matching text or layout',
      'Location unknown: the text appears 4 times in the original',
    ]);
    expect(within(sources).getByText('Cited as citation 1.')).toBeTruthy();
    expect(
      within(sources).getByText('Not a citation: a reference outside this workspace.'),
    ).toBeTruthy();
  });

  it('shows an unbuilt sources operation as not available, never as "no sources"', async () => {
    mockApi(explorerHandlers({ '/api/reads/get-sources': () => notImplemented('get_sources') }));
    renderAt(`/w/${WS}/i/${NOTE}`);
    const panel = await screen.findByRole('region', { name: 'Source panel' });
    const alert = await within(panel).findByRole('alert');
    expect(alert.textContent).toContain(
      'Listing this item’s sources: Not available in this build.',
    );
    await waitFor(() => expect(within(panel).queryByText(/cites no sources/)).toBeNull());
  });
});
