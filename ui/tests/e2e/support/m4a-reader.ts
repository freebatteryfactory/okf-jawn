/**
 * The reader's journeys' own ground (UI M4a, rulings A1 and A2): a PDF source converted with its
 * pictures and page renders kept, an image source, their `get_item` and `read_item` answers per
 * view and selection, and the bytes `download_object` answers for their objects, as real PNG
 * images drawn here (no fixture file).
 */
import { deflateSync } from 'node:zlib';
import type { Page, Route } from '@playwright/test';
import { type Answers, HEAD, NOW, ok, summary, WS } from './explorer';

export const REPORT = '66666666-6666-4666-8666-666666666666';
export const PHOTO = '77777777-7777-4777-8777-777777777777';
const RECORD = 'c'.repeat(64);
const REPORT_BYTES = 'd'.repeat(64);
const PHOTO_BYTES = 'e'.repeat(64);
const CHART = 'f'.repeat(64);
const LOGO = '9'.repeat(64);
/** Pages in the report. */
export const REPORT_PAGES = 12;

/** The digest of page `page`'s render. */
const pageDigest = (page: number) => `${page.toString(16).padStart(2, '0')}`.repeat(32);

export const report = {
  ...summary(REPORT, 'reports/annual-report.pdf.md', 'Annual report 2025', 'report', 'source'),
  extraction: { status: 'completed', text_origin: 'converter', corrected: false },
};
export const photo = {
  ...summary(PHOTO, 'photos/harbour.png.md', 'Harbour at dawn', 'photo', 'source'),
  extraction: { status: 'completed', text_origin: 'converter', corrected: false },
  media_type: 'image/png',
};

const converter = {
  name: 'docling',
  version: '2.3.0',
  models: [],
  packages: [],
  page_window: 10,
  settings: { ocr: 'auto', page_image_dpi: 144, page_images: true, table_structure: true },
};

function appearance(
  object: string,
  mediaType: string,
  filename: string,
  pages: number,
  size: number,
) {
  return {
    object,
    media_type: mediaType,
    size: String(size),
    metadata: {},
    names: [{ filename, folder: null, observed_at: NOW, supplied_by: 'ada' }],
    extraction: {
      outcome: { status: 'completed' },
      text_origin: 'converter',
      corrected: false,
      digest: RECORD,
      converter,
      page_count: pages,
      warnings: [],
    },
  };
}

// ---- PNG images, drawn here -------------------------------------------------------------------

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes: Buffer): number {
  let c = 0xffffffff;
  for (const byte of bytes) c = (CRC_TABLE[(c ^ byte) & 0xff] ?? 0) ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Buffer): Buffer {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

/**
 * A PNG of `width` × `height`: a band of colour per `seed` over a pale ground, with a dark frame,
 * so each image is told apart on a screenshot.
 */
export function png(width: number, height: number, seed: number): Buffer {
  const hue = [
    [196, 64, 48],
    [40, 120, 200],
    [60, 150, 80],
    [150, 90, 170],
    [210, 150, 40],
  ][seed % 5] ?? [128, 128, 128];
  const rows: Buffer[] = [];
  for (let y = 0; y < height; y += 1) {
    const row = Buffer.alloc(1 + width * 3);
    for (let x = 0; x < width; x += 1) {
      const frame = x < 3 || y < 3 || x >= width - 3 || y >= height - 3;
      const band = y > height * 0.3 && y < height * 0.55 && x > width * 0.1 && x < width * 0.9;
      const [r, g, b] = frame ? [40, 40, 40] : band ? hue : [244, 240, 232];
      row.writeUInt8(r ?? 0, 1 + x * 3);
      row.writeUInt8(g ?? 0, 2 + x * 3);
      row.writeUInt8(b ?? 0, 3 + x * 3);
    }
    rows.push(row);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.writeUInt8(8, 8); // bit depth
  header.writeUInt8(2, 9); // truecolour
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ---- What the service answers ------------------------------------------------------------------

type Body = Record<string, unknown>;
type Selection = { kind: string; range?: { start: number; end: number }; heading?: string };

const selectionOf = (body: Body): Selection =>
  (body.selection as Selection | undefined) ?? { kind: 'all' };

function citation(item: { id: string; path: string }, body: Body, digest: string, extra = {}) {
  return {
    workspace_id: WS,
    item_id: item.id,
    path: item.path,
    revision: HEAD,
    digest,
    selection: selectionOf(body),
    locations: [],
    ...extra,
  };
}

function pageRender(page: number) {
  return {
    object: pageDigest(page),
    media_type: 'image/png',
    role: 'page_image',
    width: 420,
    height: 594,
    source: {
      workspace_id: WS,
      item_id: REPORT,
      path: report.path,
      revision: HEAD,
      digest: pageDigest(page),
      selection: { kind: 'pages', range: { start: page, end: page } },
      locations: [{ provenance: 'direct', locator: { kind: 'page', page_no: page } }],
    },
  };
}

const pictures = [
  {
    object: CHART,
    media_type: 'image/png',
    role: 'picture',
    width: 480,
    height: 300,
    caption: 'Revenue by quarter, 2025',
    source: {
      workspace_id: WS,
      item_id: REPORT,
      path: report.path,
      revision: HEAD,
      digest: CHART,
      selection: { kind: 'pages', range: { start: 2, end: 2 } },
      locations: [{ provenance: 'direct', locator: { kind: 'page', page_no: 2 } }],
    },
  },
  {
    object: LOGO,
    media_type: 'image/png',
    role: 'picture',
    width: 240,
    height: 160,
    source: {
      workspace_id: WS,
      item_id: REPORT,
      path: report.path,
      revision: HEAD,
      digest: LOGO,
      selection: { kind: 'pages', range: { start: 3, end: 3 } },
      locations: [{ provenance: 'inferred', locator: { kind: 'page', page_no: 3 } }],
    },
  },
];

const REPORT_TEXT = [
  '# Annual report 2025',
  '',
  'Revenue grew by 12% in 2025.',
  '',
  '## Budget',
  '',
  'The budget is published after review.',
  '',
  '## Outlook',
  '',
  'The outlook for 2026 is steady.',
].join('\n');

const OUTLINE = [
  {
    kind: 'heading',
    label: 'Annual report 2025',
    level: 1,
    selection: { kind: 'lines', range: { start: 1, end: 4 } },
  },
  { kind: 'heading', label: 'Budget', level: 2, selection: { kind: 'section', heading: 'Budget' } },
  {
    kind: 'heading',
    label: 'Outlook',
    level: 2,
    selection: { kind: 'section', heading: 'Outlook' },
  },
  {
    kind: 'figure',
    label: 'Revenue by quarter, 2025',
    level: 0,
    selection: { kind: 'pages', range: { start: 2, end: 2 } },
  },
];

/** The text a selection of the report reads as. */
function reportText(selection: Selection): string {
  if (selection.kind === 'section' && selection.heading === 'Budget') {
    return '## Budget\n\nThe budget is published after review.';
  }
  if (selection.kind === 'section' && selection.heading === 'Outlook') {
    return '## Outlook\n\nThe outlook for 2026 is steady.';
  }
  return REPORT_TEXT;
}

/** The pages a selection of the report takes, as `read_item` scopes its images. */
function pagesOf(selection: Selection): number[] {
  const all = Array.from({ length: REPORT_PAGES }, (_, index) => index + 1);
  if (selection.kind === 'pages' && selection.range) {
    const { start, end } = selection.range;
    return all.filter((page) => page >= start && page <= end);
  }
  if (selection.kind === 'section') return [selection.heading === 'Budget' ? 4 : 5];
  return all;
}

/** Every `read_item` request the journey made, in order. */
export interface Reads {
  bodies: Body[];
}

/** The report's `read_item` answer, by view and selection, within the request's image budget. */
export function readReport(body: Body) {
  const view = String(body.view);
  const selection = selectionOf(body);
  const budget = Number(body.max_images);
  const warnings: Array<{ code: string; message: string }> = [];
  let media: unknown[] = [];
  if (view === 'pages') {
    const pages = pagesOf(selection);
    media = pages.slice(0, budget).map(pageRender);
    if (pages.length > budget) {
      warnings.push({
        code: 'images_omitted',
        message: `${pages.length - budget} more images lie in the selection; raise max_images or narrow it`,
      });
    }
  }
  if (view === 'multimodal') {
    const pages = pagesOf(selection);
    media = pictures.filter((picture) => pages.includes(picture.source.selection.range.start));
  }
  const markdown = view === 'text' || view === 'multimodal' ? reportText(selection) : '';
  const located =
    view === 'original' && selection.kind === 'section'
      ? [
          { provenance: 'direct', locator: { kind: 'page', page_no: 4 } },
          { provenance: 'inferred', locator: { kind: 'page', page_no: 5 } },
        ]
      : [];
  return ok({
    markdown,
    media,
    outline: selection.kind === 'all' ? OUTLINE : [],
    receipt_id: '55555555-5555-4555-8555-555555555555',
    source: citation(report, body, view === 'original' ? REPORT_BYTES : RECORD, {
      locations: located,
    }),
    truncated: false,
    view,
    warnings,
    extraction: report.extraction,
  });
}

function readPhoto(body: Body) {
  const view = String(body.view);
  return ok({
    markdown: view === 'text' || view === 'multimodal' ? 'A harbour at dawn, boats moored.' : '',
    media: [],
    outline: [],
    receipt_id: '55555555-5555-4555-8555-555555555556',
    source: citation(photo, body, view === 'original' ? PHOTO_BYTES : RECORD),
    truncated: false,
    view,
    warnings: [],
    extraction: photo.extraction,
  });
}

/** `get_item` and `read_item` of the report and the photo; other items as `explorer.ts` answers. */
export function readerAnswers(reads: Reads, others: Answers['/api/items/get-item']): Answers {
  return {
    '/api/items/list-items': (body) =>
      body.folder === 'reports'
        ? ok({ folders: [], items: [report], revision: HEAD })
        : body.folder === 'photos'
          ? ok({ folders: [], items: [photo], revision: HEAD })
          : ok({ folders: ['photos', 'reports'], items: [], revision: HEAD }),
    '/api/items/get-item': (body) => {
      if (body.item_id === REPORT) {
        return ok({
          summary: report,
          body: REPORT_TEXT,
          content_digest: 'c'.repeat(64),
          properties: { type: report.type_name, okf_jawn: { id: REPORT } },
          source: appearance(
            REPORT_BYTES,
            'application/pdf',
            'annual-report.pdf',
            REPORT_PAGES,
            812_034,
          ),
        });
      }
      if (body.item_id === PHOTO) {
        return ok({
          summary: photo,
          body: 'A harbour at dawn, boats moored.',
          content_digest: 'c'.repeat(64),
          properties: { type: photo.type_name, okf_jawn: { id: PHOTO } },
          source: appearance(PHOTO_BYTES, 'image/png', 'harbour.png', 1, 52_100),
        });
      }
      return others(body);
    },
    '/api/reads/read-item': (body) => {
      reads.bodies.push(body);
      return body.item_id === PHOTO ? readPhoto(body) : readReport(body);
    },
  };
}

/** The image each object digest is. */
function imageOf(digest: string): Buffer | null {
  if (digest === PHOTO_BYTES) return png(640, 400, 1);
  if (digest === CHART) return png(480, 300, 2);
  if (digest === LOGO) return png(240, 160, 3);
  for (let page = 1; page <= REPORT_PAGES; page += 1) {
    if (digest === pageDigest(page)) return png(420, 594, page);
  }
  return null;
}

/** Every object path asked, in order. */
export interface Downloads {
  paths: string[];
}

/** Answer `download_object` with the image of the digest it names, as the service sends one. */
export async function routeObjects(page: Page, downloads: Downloads) {
  // Registered after the operations, so it is the route that answers the byte transport.
  await page.route('**/api/workspaces/*/items/*/revisions/*/objects/*', async (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    downloads.paths.push(path);
    const digest = path.split('/').at(-1) ?? '';
    const body = imageOf(digest);
    if (body === null) {
      await route.fulfill({
        status: 404,
        contentType: 'application/json',
        body: JSON.stringify({ code: 'not_found', message: 'no such object' }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      body,
      headers: {
        'content-type': 'image/png',
        'content-disposition': 'attachment',
        'content-security-policy': "sandbox; default-src 'none'",
        'x-content-type-options': 'nosniff',
      },
    });
  });
}
