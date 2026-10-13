/**
 * The conversion state of an imported item, in the tree, on its tab and in the reader, for every
 * state the generated types name: always in words, and an unsupported file never as converted or
 * as an empty document.
 */
import { cleanup, configure, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type {
  ConversionOutcome,
  ExtractionSummary,
  ItemSummary,
} from '../../src/api/generated/types.gen';
import { fail, fromNewerService, mockApi, ok } from './support/api';
import {
  explorerHandlers,
  itemDocument,
  listing,
  NOTE,
  OBJECT,
  readResponse,
  renderAt,
  SOURCE,
  sourceSummary,
  WS,
} from './support/workspace';

const ITEM = '/api/items/get-item';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** The workspace's one source in this state: as the tree lists it, and as `get_item` reads it. */
function sourceIn(
  outcome: ConversionOutcome,
  textOrigin: ExtractionSummary['text_origin'] = 'converter',
) {
  const extraction: ExtractionSummary = {
    status: outcome.status,
    text_origin: textOrigin,
    corrected: false,
  };
  const summary: ItemSummary = { ...sourceSummary, extraction };
  return {
    summary,
    extraction,
    handlers: {
      '/api/items/list-items': () => ok(listing([], [summary])),
      [ITEM]: () =>
        ok({
          ...itemDocument(summary),
          source: {
            extraction: { outcome, corrected: false, text_origin: textOrigin, warnings: [] },
            media_type: 'application/pdf',
            metadata: {},
            names: [],
            object: OBJECT,
            size: '3',
          },
        }),
      '/api/reads/read-item': () =>
        ok(
          readResponse(summary, textOrigin === 'none' ? '' : 'Text of the report', { extraction }),
        ),
    },
  };
}

async function open(outcome: ConversionOutcome, textOrigin?: ExtractionSummary['text_origin']) {
  const source = sourceIn(outcome, textOrigin);
  const api = mockApi(explorerHandlers(source.handlers));
  renderAt(`/w/${WS}/i/${SOURCE}`);
  const state = await screen.findByText('Conversion:');
  return { api, note: state.closest('[role="note"]') };
}

const treeRow = () => screen.getByRole('treeitem', { name: /^Annual report/ });
const tab = () => screen.getByRole('tab', { name: 'Annual report' });
/** What the tab says besides its name. */
const tabSays = () =>
  document.getElementById(tab().getAttribute('aria-describedby') ?? '')?.textContent;

describe('the state of an imported item', () => {
  it('says a pending conversion is pending, and not an empty document', async () => {
    const { note } = await open({ status: 'pending' }, 'none');
    expect(note?.textContent).toBe(
      'Conversion: Conversion is pending; the original bytes are kept. Its text is not ready yet. This is not an empty document.',
    );
    expect(treeRow().getAttribute('aria-label')).toBe(
      'Annual report, type report, conversion pending',
    );
    expect(tabSays()).toBe(' (conversion pending)');
    expect(tab().textContent).toContain('(conversion pending)');
  });

  it('says why a conversion failed, with what the converter recorded and where a retry is', async () => {
    const { note } = await open(
      {
        status: 'failed',
        reason: { kind: 'time_limit', limit_seconds: 120 },
        issues: [
          {
            component_type: 'document_backend',
            module_name: 'pipeline',
            error_message: 'gave up on page 3',
          },
        ],
      },
      'none',
    );
    expect(note?.textContent).toContain(
      'Conversion: Conversion failed because it took longer than 120 seconds; the original bytes are kept.',
    );
    expect(note?.textContent).toContain('The converter recorded: gave up on page 3 (pipeline)');
    const jobs = within(note as HTMLElement).getByRole('link', { name: 'Jobs' });
    expect(jobs.getAttribute('href')).toBe(`/w/${WS}/jobs`);
    expect(treeRow().getAttribute('aria-label')).toBe(
      'Annual report, type report, conversion failed',
    );
    expect(tabSays()).toBe(' (conversion failed)');
  });

  it('shows an unsupported file as unsupported: never as converted, never as an empty document', async () => {
    const { note } = await open({ status: 'unsupported' }, 'none');
    expect(note?.textContent).toBe(
      'Conversion: Bytes kept; text extraction is not supported for this format. There is no extracted text. This is not an empty document, and it has not been converted.',
    );
    expect(treeRow().getAttribute('aria-label')).toBe(
      'Annual report, type report, format not supported',
    );
    expect(tabSays()).toBe(' (format not supported)');
    // Nowhere is it called converted; and where its text would be, the page says there is none.
    expect(screen.queryByText(/^Converted\.$/)).toBeNull();
    expect(note?.textContent).not.toMatch(/Conversion: Converted/);
    expect(await screen.findByText('There is no text to show.')).toBeDefined();
  });

  it('names the pages a partial conversion left out', async () => {
    const { note } = await open({
      status: 'partial',
      issues: [],
      coverage: {
        page_count: 5,
        converted: [{ start: 1, end: 2 }],
        partly_extracted: [],
        not_converted: [{ start: 3, end: 5 }],
      },
    });
    expect(note?.textContent).toBe('Conversion: Partly converted. Not converted: pages 3–5.');
    expect(treeRow().getAttribute('aria-label')).toBe(
      'Annual report, type report, partly converted',
    );
    expect(tabSays()).toBe(' (partly converted)');
  });

  it('says a converted source is converted, and marks neither its row nor its tab', async () => {
    const { note, api } = await open({ status: 'completed' });
    expect(note?.textContent).toBe('Conversion: Converted.');
    expect(treeRow().getAttribute('aria-label')).toBe('Annual report, type report');
    expect(tabSays()).toBe('');
    expect(tab().textContent).toBe('Annual report');
    expect(api.violations).toEqual([]);
  });

  it('says nothing about conversion for an item that is not a source', async () => {
    mockApi(explorerHandlers());
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    expect(screen.queryByText('Conversion:')).toBeNull();
    const noteTab = screen.getByRole('tab', { name: 'Meeting notes' });
    expect(
      document.getElementById(noteTab.getAttribute('aria-describedby') ?? '')?.textContent,
    ).toBe('');
  });

  it('says that a conversion is in a state this page does not know, and never shows it as converted', async () => {
    // A newer service names a state this build's types do not.
    const { note } = await open({ status: fromNewerService<'unsupported'>('quarantined') });
    expect(note?.textContent).toBe(
      'Conversion: The conversion is in a state this page does not know (“quarantined”). It is not known to be converted.',
    );
    expect(note?.textContent).not.toContain('Converted.');
    // In the tree it carries a mark: no mark is how a converted source looks.
    expect(treeRow().getAttribute('aria-label')).toBe(
      'Annual report, type report, conversion state not known: “quarantined”',
    );
    expect(within(treeRow()).getByText('conversion state not known: “quarantined”')).toBeDefined();
    expect(tabSays()).toBe(' (conversion state not known: “quarantined”)');
    expect(tab().textContent).toContain('(conversion state not known: “quarantined”)');
    expect(document.body.textContent).not.toContain('undefined');
  });

  it('says the same where only the listing’s verdict is known', async () => {
    const extraction = {
      status: fromNewerService<ExtractionSummary['status']>('quarantined'),
      text_origin: 'converter' as const,
      corrected: false,
    };
    const summary: ItemSummary = { ...sourceSummary, extraction };
    mockApi(
      explorerHandlers({
        '/api/items/list-items': () => ok(listing([], [summary])),
        [ITEM]: () => ok(itemDocument(summary)),
        '/api/reads/read-item': () => ok(readResponse(summary, 'Text', { extraction })),
      }),
    );
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const state = await screen.findByText('Conversion:');
    expect(state.closest('[role="note"]')?.textContent).toBe(
      'Conversion: The conversion is in a state this page does not know (“quarantined”). It is not known to be converted.',
    );
  });

  it('gives the verdict alone when only the reading knows it', async () => {
    const source = sourceIn({ status: 'unsupported' }, 'none');
    mockApi(
      explorerHandlers({
        ...source.handlers,
        [ITEM]: () => fail(500, 'internal', 'the properties could not be read'),
      }),
    );
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const state = await screen.findByText('Conversion:');
    expect(state.closest('[role="note"]')?.textContent).toBe(
      'Conversion: Bytes kept; text extraction is not supported for this format. There is no extracted text. This is not an empty document, and it has not been converted.',
    );
  });
});
