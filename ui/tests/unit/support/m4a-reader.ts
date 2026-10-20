/**
 * The reader's own fixtures (UI M4a, ruling A1): a note read in parts, a Source with its
 * original, pictures and page renders, and an answer that can be held back while a test looks at
 * the page.
 */
import type {
  ConverterIdentity,
  ItemDocument,
  ItemSummary,
  MediaReference,
  ReadItemResponse,
  Selection,
  SourceAppearance,
} from '../../../src/api/generated/types.gen';
import type { Handler, Json, Reply } from './api';
import { ok } from './api';
import {
  DIGEST,
  HEAD,
  itemDocument,
  NOW,
  noteSummary,
  OBJECT,
  readResponse,
  SOURCE,
  sourceSummary,
  WS,
} from './workspace';

/** The selection a request asked for, as the answer cites it. */
function selectionOf(body: Json): Selection {
  const selection = body.selection;
  return selection !== null && typeof selection === 'object' && 'kind' in selection
    ? (selection as Selection)
    : { kind: 'all' };
}

/**
 * `read_item` answers of a note read in `count` parts: part n is "Part n of the text", and each
 * but the last carries the cursor `part-(n+1)`. At `revision`, which each answer cites.
 */
export function partsOf(count: number, revision: string = HEAD) {
  return (body: Json): Reply => {
    const cursor = typeof body.cursor === 'string' ? body.cursor : null;
    const number = cursor === null ? 1 : Number(cursor.replace('part-', ''));
    const more = number < count;
    return ok(
      readResponse(
        { ...noteSummary, revision },
        `Part ${number} of the text`,
        {
          receipt_id: `55555555-5555-4555-8555-${String(number).padStart(12, '0')}`,
          truncated: more,
          ...(more ? { next_cursor: `part-${number + 1}` } : {}),
        },
        selectionOf(body),
      ),
    );
  };
}

/** The note's two sections, each in parts: Budget in 3, Agenda in 2. */
export const SECTIONS = { Budget: 3, Agenda: 2 } as const;
export type SectionName = keyof typeof SECTIONS;

export const sectionOf = (heading: SectionName): Selection => ({ kind: 'section', heading });

/**
 * `read_item` answers of the note by section (`SECTIONS`): part n of a section is "<heading> part
 * n", each but the last with the cursor `<heading>-(n+1)`, every part with the outline of both
 * sections. The whole item is answered as its first section.
 */
export function sectionsOf(body: Json): Reply {
  const selection = selectionOf(body);
  const heading: SectionName =
    selection.kind === 'section' && selection.heading === 'Agenda' ? 'Agenda' : 'Budget';
  const cursor = typeof body.cursor === 'string' ? body.cursor : null;
  const number = cursor === null ? 1 : Number(cursor.split('-').at(-1));
  const more = number < SECTIONS[heading];
  const index = heading === 'Budget' ? 1 : 2;
  return ok(
    readResponse(
      noteSummary,
      `${heading} part ${number}`,
      {
        receipt_id: `55555555-5555-4555-8555-${String(index * 10 + number).padStart(12, '0')}`,
        truncated: more,
        ...(more ? { next_cursor: `${heading}-${number + 1}` } : {}),
        outline: (['Budget', 'Agenda'] as const).map((label) => ({
          kind: 'heading',
          label,
          level: 2,
          selection: sectionOf(label),
        })),
      },
      selection,
    ),
  );
}

/** An answer a test holds back until it releases it; requests that come meanwhile wait too. */
export function held(answer: Handler) {
  let holding = false;
  let waiting: Array<() => void> = [];
  return {
    handler: async (body: Json): Promise<Reply> => {
      if (holding) await new Promise<void>((resolve) => waiting.push(resolve));
      return answer(body);
    },
    hold: () => {
      holding = true;
    },
    release: () => {
      holding = false;
      for (const resume of waiting) resume();
      waiting = [];
    },
    waiting: () => waiting.length,
  };
}

export const PICTURE = 'e'.repeat(64);
export const PAGE_ONE = '1'.repeat(64);
export const PAGE_TWO = '2'.repeat(64);

export const converter: ConverterIdentity = {
  name: 'docling',
  version: '2.3.0',
  models: [],
  packages: [],
  page_window: 10,
  settings: {
    ocr: 'auto',
    page_image_dpi: 144,
    page_images: true,
    table_structure: true,
  },
};

/** A PDF source, converted, with its page renders kept. */
export const appearance: SourceAppearance = {
  object: OBJECT,
  media_type: 'application/pdf',
  size: '20480',
  metadata: {},
  names: [{ filename: 'report.pdf', folder: 'sources', observed_at: NOW, supplied_by: 'ada' }],
  extraction: {
    outcome: { status: 'completed' },
    text_origin: 'converter',
    corrected: false,
    digest: DIGEST,
    converter,
    page_count: 2,
    warnings: [],
  },
};

/** The source's `get_item` answer, with its appearance. */
export function sourceDocument(
  source: SourceAppearance = appearance,
  item: ItemSummary = sourceSummary,
): ItemDocument {
  return { ...itemDocument(item), source };
}

/** A page render of page `page`: direct on its page, with no caption. */
export function pageImage(page: number, object: string): MediaReference {
  return {
    object,
    media_type: 'image/png',
    role: 'page_image',
    width: 1190,
    height: 1684,
    source: {
      workspace_id: WS,
      item_id: SOURCE,
      path: sourceSummary.path,
      revision: HEAD,
      digest: object,
      selection: { kind: 'pages', range: { start: page, end: page } },
      locations: [{ provenance: 'direct', locator: { kind: 'page', page_no: page } }],
    },
  };
}

/** A picture with a caption, its place on page 1 inferred. */
export const picture: MediaReference = {
  object: PICTURE,
  media_type: 'image/png',
  role: 'picture',
  width: 640,
  height: 480,
  caption: 'Revenue by quarter, 2025',
  source: {
    workspace_id: WS,
    item_id: SOURCE,
    path: sourceSummary.path,
    revision: HEAD,
    digest: PICTURE,
    selection: { kind: 'pages', range: { start: 1, end: 1 } },
    locations: [{ provenance: 'inferred', locator: { kind: 'page', page_no: 1 } }],
  },
};

/** A `read_item` answer of the source in `view`, citing the request's selection. */
export function sourceRead(body: Json, extra: Partial<ReadItemResponse> = {}): ReadItemResponse {
  const view = body.view as ReadItemResponse['view'];
  const original = view === 'original';
  return readResponse(
    sourceSummary,
    view === 'text' || view === 'multimodal' ? '# Annual report\n\nText of the report' : '',
    {
      view,
      extraction: { status: 'completed', text_origin: 'converter', corrected: false },
      ...extra,
      source: {
        workspace_id: WS,
        item_id: SOURCE,
        path: sourceSummary.path,
        revision: HEAD,
        digest: original ? OBJECT : DIGEST,
        selection: selectionOf(body),
        locations: [],
        ...extra.source,
      },
    },
    selectionOf(body),
  );
}
