/**
 * An Office file's original, and an original with no viewer (UI M4a, ruling A2; SPEC section 7:
 * Office extraction with original access). An Office file is shown as its conversion, in the
 * reader's text view, and the words say so as the conversion stands: converted (and corrected by a
 * person), supplied by an agent, not converted yet, failed or not supported; no conversion is
 * claimed that the service did not record. An original with no viewer says so, with its media type
 * and size. Both give access to the original's bytes as a plain link to `download_object` at the
 * revision shown (`<a href download>`, the one exception to the transport rule here), named by the
 * file it was supplied as: the page itself reads nothing. Reading an original is a read: a reader
 * is offered it, and so is anyone in an archived workspace.
 */
import { QueryClientProvider } from '@tanstack/react-query';
import { cleanup, configure, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ConversionOutcome, SourceAppearance } from '../../src/api/generated/types.gen';
import { createQueryClient } from '../../src/app/query-client';
import { OfficeOriginal } from '../../src/features/viewers/office/OfficeOriginal';
import { OtherOriginal } from '../../src/features/viewers/office/OtherOriginal';
import { mockApi, ok } from './support/api';
import { appearanceOf, DOCX, ITEM, OBJECT_PATH, REVISION, WS } from './support/m4a-pdf';
import {
  explorerHandlers,
  itemDocument,
  renderAt,
  SOURCE,
  sourceSummary,
  workspace,
} from './support/workspace';

// Below the test's own time limit, so a wait that fails says what it waited for.
configure({ asyncUtilTimeout: 4_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const PPTX = 'application/vnd.openxmlformats-officedocument.presentationml.presentation';
const ZIP = 'application/zip';

const named = (filename: string) => ({
  names: [
    {
      filename,
      folder: 'reports',
      observed_at: '2026-01-01T09:00:00.000Z',
      supplied_by: 'ada',
    },
  ],
});

/** Render `Viewer` for an original of `appearance`; every request it would make is recorded. */
function renderOriginal(
  Viewer: typeof OfficeOriginal | typeof OtherOriginal,
  appearance: SourceAppearance,
) {
  const requests = vi.fn(async () => new Response('{}', { status: 500 }));
  vi.stubGlobal('fetch', requests);
  render(
    <QueryClientProvider client={createQueryClient()}>
      <Viewer
        workspaceId={WS}
        itemId={ITEM}
        revision={REVISION}
        path="reports/quarterly.docx.md"
        appearance={appearance}
        selection={{ kind: 'all' }}
      />
    </QueryClientProvider>,
  );
  return requests;
}

/** The text of the whole render, its spaces as a reader sees them. */
const said = () => (document.body.textContent ?? '').replace(/\s+/g, ' ');

describe('an Office file’s original', () => {
  it('is said to be shown as its conversion, in the reader’s text view, with a plain link to its bytes', () => {
    const requests = renderOriginal(
      OfficeOriginal,
      appearanceOf(DOCX, 12_345, named('quarterly.docx')),
    );
    expect(said()).toContain(
      'This Word document is shown as its conversion: the reader’s text view has the text the converter read from it. The file itself is not drawn here.',
    );
    const link = screen.getByRole('link', { name: 'Download the original, quarterly.docx' });
    expect(link.getAttribute('href')).toBe(OBJECT_PATH);
    expect(link.getAttribute('download')).toBe('quarterly.docx');
    expect(said()).toContain(`(12,345 bytes (12 KB), ${DOCX})`);
    // The page itself reads nothing: the browser follows the link.
    expect(requests).not.toHaveBeenCalled();
  });

  it('says a person’s corrections over the converted text', () => {
    const appearance = appearanceOf(DOCX, 2048, named('quarterly.docx'));
    renderOriginal(OfficeOriginal, {
      ...appearance,
      extraction: { ...appearance.extraction, corrected: true },
    });
    expect(said()).toContain(
      'the reader’s text view has the text the converter read from it, with a person’s corrections over it.',
    );
  });

  it('says a conversion that read only part of it', () => {
    const appearance = appearanceOf(DOCX, 2048, named('quarterly.docx'));
    renderOriginal(OfficeOriginal, {
      ...appearance,
      extraction: { ...appearance.extraction, outcome: { status: 'partial', issues: [] } },
    });
    expect(said()).toContain(
      'the reader’s text view has the text the converter read from it, which is not all of it.',
    );
  });

  it('says text an agent supplied, and that it is not the converter’s', () => {
    const appearance = appearanceOf(DOCX, 2048, named('quarterly.docx'));
    renderOriginal(OfficeOriginal, {
      ...appearance,
      extraction: {
        ...appearance.extraction,
        outcome: { status: 'failed', reason: { kind: 'damaged' }, issues: [] },
        text_origin: 'supplied_by_agent',
      },
    });
    expect(said()).toContain(
      'This Word document is shown as text an agent supplied for it, which a person accepted: the reader’s text view has it. It is not the converter’s reading of the file, and the file itself is not drawn here.',
    );
  });

  it.each<[string, ConversionOutcome, string]>([
    [
      'pending',
      { status: 'pending' },
      'This Word document has no conversion yet: the reader’s text view will show it once the converter has read the file. The file itself is not drawn here.',
    ],
    [
      'failed',
      { status: 'failed', reason: { kind: 'converter_crashed' }, issues: [] },
      'This Word document has no conversion to show: the converter could not read it. The file itself is not drawn here.',
    ],
    [
      'unsupported',
      { status: 'unsupported' },
      'This Word document has no conversion to show: its format is not converted by this service. The file itself is not drawn here.',
    ],
  ])('claims no conversion when it is %s', (_state, outcome, words) => {
    const appearance = appearanceOf(DOCX, 2048, named('quarterly.docx'));
    renderOriginal(OfficeOriginal, {
      ...appearance,
      extraction: { ...appearance.extraction, outcome, text_origin: 'none' },
    });
    expect(said()).toContain(words);
    expect(said()).not.toContain('is shown as its conversion');
    expect(
      screen.getByRole('link', { name: 'Download the original, quarterly.docx' }),
    ).toBeTruthy();
  });

  it('names a presentation as one, and a file whose name was not recorded by its path', () => {
    renderOriginal(OfficeOriginal, appearanceOf(PPTX, 4096, { names: [] }));
    expect(said()).toContain('This PowerPoint presentation is shown as its conversion');
    const link = screen.getByRole('link', { name: 'Download the original, quarterly.docx.md' });
    expect(link.getAttribute('download')).toBe('quarterly.docx.md');
  });

  it('is named by the file it was last supplied as', () => {
    renderOriginal(
      OfficeOriginal,
      appearanceOf(DOCX, 4096, {
        names: [
          {
            filename: 'draft.docx',
            folder: 'reports',
            observed_at: '2026-01-01T09:00:00.000Z',
            supplied_by: 'ada',
          },
          {
            filename: 'final.docx',
            folder: 'reports',
            observed_at: '2026-02-01T09:00:00.000Z',
            supplied_by: 'ada',
          },
        ],
      }),
    );
    expect(screen.getByRole('link', { name: 'Download the original, final.docx' })).toBeTruthy();
  });
});

describe('an original with no viewer', () => {
  it('says so in words, with its media type and size, and the same plain link to its bytes', () => {
    const requests = renderOriginal(OtherOriginal, appearanceOf(ZIP, 3072, named('photos.zip')));
    expect(said()).toContain(
      'There is no viewer here for files of this type (application/zip). The original, 3,072 bytes (3.0 KB), is kept: download it to open it with another program.',
    );
    const link = screen.getByRole('link', { name: 'Download the original, photos.zip' });
    expect(link.getAttribute('href')).toBe(OBJECT_PATH);
    expect(link.getAttribute('download')).toBe('photos.zip');
    expect(requests).not.toHaveBeenCalled();
  });
});

describe('reading an original', () => {
  it.each([
    ['a reader', { permissions: ['read' as const] }],
    ['a writer', { permissions: ['read' as const, 'write' as const] }],
    [
      'anyone in an archived workspace',
      { permissions: ['read' as const, 'write' as const], archived_at: '2026-01-02T08:00:00.000Z' },
    ],
  ])('is offered to %s, in the reader', async (_who, grant) => {
    const api = mockApi(
      explorerHandlers({
        '/api/workspaces/open-workspace': () => ok({ ...workspace, ...grant }),
        '/api/items/get-item': () =>
          ok({
            ...itemDocument(sourceSummary),
            source: appearanceOf(DOCX, 12_345, named('quarterly.docx')),
          }),
      }),
    );
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const link = await screen.findByRole('link', { name: 'Download the original, quarterly.docx' });
    expect(link.getAttribute('href')).toBe(
      `/api/workspaces/${WS}/items/${SOURCE}/revisions/${sourceSummary.revision}/objects/${'d'.repeat(64)}`,
    );
    expect(api.violations).toEqual([]);
  });
});
