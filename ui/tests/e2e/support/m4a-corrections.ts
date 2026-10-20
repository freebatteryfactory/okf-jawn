/**
 * The service behind the corrections journeys (UI M4a, ruling A4), played through `page.route`:
 * one partly converted PDF source in a workspace the person may change, read as the real service
 * reads it (`get_item` with its whole extraction, `read_item` with its text, at the revision
 * asked), and its corrections, as `crates/core/src/application/imports.rs` takes them:
 * - `correct_digest` on a base that is not the head is `conflict` with `stale_base`; with the
 *   item's digest at the head it commits a new revision, at which the source shows the corrected
 *   text, marked corrected;
 * - `redigest_item` answers 202 with the job it queued; the job is then reported running, and at
 *   the next reading finished with the revision it made, at which the source holds a complete
 *   conversion with a new digest and the converter's new text.
 * The original's bytes (`download_object`) are a one-page PDF, so a viewer that reads them has
 * real bytes to show.
 */
import type { Page, Route } from '@playwright/test';
import {
  type Answers,
  HEAD,
  ok,
  type Reply,
  routeApi,
  SOURCE,
  WS,
  writableWorkspace,
} from './explorer';

type Body = Record<string, unknown>;

export const CORRECTED = 'e'.repeat(40);
export const RECONVERTED = '5'.repeat(40);
export const DIGEST = 'c'.repeat(64);
export const NEW_DIGEST = '9'.repeat(64);
export const OBJECT = 'd'.repeat(64);
export const JOB = '77777777-7777-4777-8777-777777777777';
const RECEIPT = '55555555-5555-4555-8555-555555555555';

/**
 * The converter's text, with a misread year. It opens without a `#` heading: the reader draws a
 * text's `#` as a level-one heading, and the reader's "Original" (h3) then follows it, which axe
 * reports as `heading-order` (the reader's, ruling A1; said in the M4a report).
 */
export const CONVERTED = 'Annual report 2025\n\nRevenue grew by 21% in 2O25.';
/** The person's correction of it. */
export const FIXED = 'Annual report 2025\n\nRevenue grew by 21% in 2025.';
/** The converter's text of the pages converted again. */
export const RECONVERTED_TEXT =
  'Annual report 2025\n\nRevenue grew by 21% in 2025.\n\n## Outlook\n\nThe pages that were not converted are converted now.';

const SETTINGS = {
  ocr: 'auto',
  ocr_language: 'en',
  page_image_dpi: 144,
  page_images: true,
  table_structure: true,
};
const CONVERTER = {
  name: 'docling',
  version: '2.3.0',
  models: [],
  packages: [],
  page_window: 10,
  settings: SETTINGS,
};
const PARTIAL = {
  status: 'partial',
  issues: [
    {
      component_type: 'document_backend',
      module_name: 'pipeline',
      error_message: 'pages 3 and 4 hold no text layer',
    },
  ],
  coverage: {
    page_count: 4,
    converted: [{ start: 1, end: 2 }],
    not_converted: [{ start: 3, end: 4 }],
    partly_extracted: [],
  },
};

interface Version {
  body: string;
  extraction: Body;
}

const status = (extraction: Body) => {
  const outcome = extraction.outcome as { status: string };
  return {
    status: outcome.status,
    text_origin: extraction.text_origin,
    corrected: extraction.corrected,
  };
};

function summaryAt(revision: string, version: Version) {
  return {
    id: SOURCE,
    path: 'annual-report.pdf.md',
    title: 'Annual report 2025',
    type_name: 'report',
    kind: 'source',
    description: 'Annual report 2025.',
    archived: false,
    revision,
    status: 'stable',
    media_type: 'application/pdf',
    extraction: status(version.extraction),
  };
}

function documentAt(revision: string, version: Version) {
  return {
    summary: summaryAt(revision, version),
    body: version.body,
    content_digest: 'b'.repeat(64),
    properties: { type: 'report', title: 'Annual report 2025', okf_jawn: { id: SOURCE } },
    source: {
      extraction: version.extraction,
      media_type: 'application/pdf',
      metadata: {},
      names: [],
      object: OBJECT,
      size: String(PDF.length),
    },
  };
}

/** A one-page PDF with the report's title on it, its cross-reference table counted here. */
function onePagePdf(): Buffer {
  const text = 'BT /F1 18 Tf 24 60 Td (Annual report 2025) Tj ET';
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 120] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>',
    `<< /Length ${text.length} >>\nstream\n${text}\nendstream`,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ];
  let pdf = '%PDF-1.4\n';
  const offsets: number[] = [];
  objects.forEach((object, index) => {
    offsets.push(pdf.length);
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = pdf.length;
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) pdf += `${String(offset).padStart(10, '0')} 00000 n \n`;
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(pdf, 'latin1');
}

export const PDF = onePagePdf();

export interface CorrectionsService {
  /** Every `correct_digest` request, in order. */
  corrections: Body[];
  /** Every `redigest_item` request, in order. */
  reconversions: Body[];
  head: () => string;
}

const revisionAsked = (body: Body, head: string): string => {
  const at = body.at as { kind?: string; revision?: string } | undefined;
  return at?.kind === 'revision' && typeof at.revision === 'string' ? at.revision : head;
};

/** Play the service on `page`. */
export async function playCorrections(
  page: Page,
  extra: Answers = {},
): Promise<CorrectionsService> {
  const versions = new Map<string, Version>([
    [
      HEAD,
      {
        body: CONVERTED,
        extraction: {
          outcome: PARTIAL,
          converter: CONVERTER,
          digest: DIGEST,
          page_count: 4,
          text_origin: 'converter',
          corrected: false,
          warnings: [],
        },
      },
    ],
  ]);
  const state = { head: HEAD, jobReads: 0, jobState: 'queued' };
  const service: CorrectionsService = {
    corrections: [],
    reconversions: [],
    head: () => state.head,
  };
  const versionAt = (body: Body) => versions.get(revisionAsked(body, state.head));
  const missing: Reply = {
    status: 404,
    body: { code: 'not_found', message: 'No item with that identity here' },
  };
  const job = () => ({
    id: JOB,
    kind: 'redigest',
    state: state.jobState,
    attempt: 1,
    progress: state.jobState === 'succeeded' ? 100 : state.jobState === 'running' ? 40 : 0,
    item_ids: state.jobState === 'succeeded' ? [SOURCE] : [],
    warnings: [],
    workspace_id: WS,
    created_at: '2026-10-20T09:00:00.000Z',
    ...(state.jobState === 'queued' ? {} : { started_at: '2026-10-20T09:00:01.000Z' }),
    ...(state.jobState === 'succeeded'
      ? { finished_at: '2026-10-20T09:00:06.000Z', revision: RECONVERTED }
      : {}),
  });
  await routeApi(page, {
    ...extra,
    '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: state.head }),
    '/api/items/list-items': (body) => {
      const version = versions.get(state.head);
      return ok({
        folders: [],
        items: body.folder ? [] : version === undefined ? [] : [summaryAt(state.head, version)],
        revision: state.head,
      });
    },
    '/api/items/get-item': (body) => {
      const version = versionAt(body);
      return version === undefined
        ? missing
        : ok(documentAt(revisionAsked(body, state.head), version));
    },
    '/api/reads/read-item': (body) => {
      const version = versionAt(body);
      if (version === undefined) return missing;
      const revision = revisionAsked(body, state.head);
      return ok({
        markdown: version.body,
        media: [],
        outline: [],
        receipt_id: RECEIPT,
        source: {
          workspace_id: WS,
          item_id: SOURCE,
          path: 'annual-report.pdf.md',
          revision,
          digest: version.extraction.digest,
          selection: body.selection,
          locations: [],
        },
        truncated: false,
        view: 'text',
        warnings: [],
        extraction: status(version.extraction),
      });
    },
    '/api/imports/correct-digest': (body) => {
      service.corrections.push(body);
      if (body.base_revision !== state.head) {
        return {
          status: 409,
          body: {
            code: 'conflict',
            message: 'the workspace head moved since the base revision',
            field: '/base_revision',
            detail: { kind: 'stale_base', head: state.head },
          },
        };
      }
      const current = versions.get(state.head);
      if (current === undefined || current.extraction.digest !== body.digest) {
        return {
          status: 400,
          body: {
            code: 'invalid_input',
            message: 'the digest is not this item’s at the base revision',
            field: '/digest',
          },
        };
      }
      versions.set(CORRECTED, {
        body: String(body.corrected_markdown),
        extraction: { ...current.extraction, corrected: true, text_origin: 'converter' },
      });
      state.head = CORRECTED;
      return ok({ revision: CORRECTED, receipt_id: RECEIPT, warnings: [] });
    },
    '/api/imports/redigest-item': (body) => {
      service.reconversions.push(body);
      state.jobState = 'queued';
      return { status: 202, body: job() };
    },
    // Running when first asked; finished, with the revision it made, when asked again.
    '/api/imports/get-job': () => {
      state.jobReads += 1;
      if (state.jobReads === 1) state.jobState = 'running';
      else if (state.jobState !== 'succeeded') {
        state.jobState = 'succeeded';
        versions.set(RECONVERTED, {
          body: RECONVERTED_TEXT,
          extraction: {
            outcome: { status: 'completed' },
            converter: CONVERTER,
            digest: NEW_DIGEST,
            page_count: 4,
            text_origin: 'converter',
            corrected: false,
            warnings: [],
          },
        });
        state.head = RECONVERTED;
      }
      return ok(job());
    },
    '/api/imports/list-jobs': () =>
      ok({ items: service.reconversions.length === 0 ? [] : [job()], next_cursor: null }),
  });
  // The original's bytes, through the byte transport (registered last, so it is asked first).
  await page.route(
    `**/api/workspaces/${WS}/items/${SOURCE}/revisions/*/objects/*`,
    (route: Route) => route.fulfill({ status: 200, contentType: 'application/pdf', body: PDF }),
  );
  return service;
}
