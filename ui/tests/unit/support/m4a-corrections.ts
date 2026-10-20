/**
 * Ground for the corrections tests (UI M4a, ruling A4): a source as `get_item` answers it, with its
 * whole extraction (`SourceAppearance.extraction`: the outcome, the converter and its settings, the
 * digest, whose text is shown and the supplied text), its text as `read_item` answers it, the
 * accepted proposal a supplied text came from, and jobs of a reconversion.
 *
 * The service it plays keeps the rules of the real one (`crates/core/src/application/imports.rs`):
 * `correct_digest` is a `conflict` with `stale_base` on `/base_revision` when its base is not the
 * head, `invalid_input` on `/digest` when the digest is not the item's at the base, and with
 * `adopts` a `conflict` on `/adopts` unless the text is the proposal's supplied text; a correction
 * commits a new head, at which the item shows the corrected text (`corrected`, the converter's
 * origin) or, adopted, the supplied text (the agent's origin, kept by the person). `redigest_item`
 * answers 202 with the job it queued.
 */
import type {
  ConversionOutcome,
  ConversionSettings,
  Extraction,
  ItemDocument,
  ItemSummary,
  Job,
  Proposal,
  SuppliedText,
} from '../../../src/api/generated/types.gen';
import { fail, type Handler, type Json, ok, type Reply } from './api';
import { JOB } from './imports';
import { CORRECT_DIGEST, REDIGEST_ITEM } from './reader';
import {
  DIGEST,
  explorerHandlers,
  HEAD,
  itemDocument,
  listing,
  NOW,
  OBJECT,
  RECEIPT,
  readResponse,
  SOURCE,
  sourceSummary,
  workspace,
  writableWorkspace,
} from './workspace';

export const GET_PROPOSAL = '/api/proposals/get-proposal';
export const OPEN = '/api/workspaces/open-workspace';
export const ITEM = '/api/items/get-item';
export const READ = '/api/reads/read-item';
export const LIST = '/api/items/list-items';

/** The revision a correction makes. */
export const CORRECTED_AT = 'e'.repeat(40);
/** A head someone else made meanwhile. */
export const ELSEWHERE = 'f'.repeat(40);
/** The digest of a later conversion. */
export const LATER_DIGEST = '9'.repeat(64);
export const PROPOSAL = '66666666-6666-4666-8666-666666666666';
export const REDIGEST_JOB = '77777777-7777-4777-8777-777777777777';
export const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** The text the converter made. */
export const CONVERTED = '# Annual report\n\nRevenue grew by 21% in 2O25.';
/** The text an agent supplied, as its proposal holds it. */
export const SUPPLIED_MARKDOWN = '# Annual report\n\nRevenue grew by 12% in 2025.\n';

export const SETTINGS: ConversionSettings = {
  ocr: 'auto',
  ocr_language: 'en',
  page_image_dpi: 144,
  page_images: true,
  table_structure: true,
};

export const CONVERTER = {
  name: 'docling',
  version: '2.3.0',
  models: [],
  packages: [{ name: 'docling-pdf', version: '2.3.0', source: 'registry+https://example.org' }],
  page_window: 10,
  settings: SETTINGS,
};

export const PARTIAL: ConversionOutcome = {
  status: 'partial',
  issues: [],
  coverage: {
    page_count: 6,
    converted: [{ start: 1, end: 2 }],
    not_converted: [{ start: 3, end: 4 }],
    partly_extracted: [],
  },
};

export const SUPPLIED: SuppliedText = {
  proposal_id: PROPOSAL,
  supplied_by: 'agent-7',
  client_id: 'desk-client',
  approved_by: 'ada',
  approved_at: '2026-01-02T09:15:00.000Z',
  content_digest: 'b'.repeat(64),
  pages: [{ start: 3, end: 4 }],
};

/** A converted source's extraction, with whatever a test changes. */
export function extractionOf(extra: Partial<Extraction> = {}): Extraction {
  return {
    outcome: { status: 'completed' },
    converter: CONVERTER,
    digest: DIGEST,
    page_count: 6,
    text_origin: 'converter',
    corrected: false,
    warnings: [],
    ...extra,
  };
}

/** The source's summary at `revision`, its listing's view of `extraction`. */
export function sourceAt(revision: string, extraction: Extraction): ItemSummary {
  return {
    ...sourceSummary,
    revision,
    extraction: {
      status: extraction.outcome.status,
      text_origin: extraction.text_origin,
      corrected: extraction.corrected,
    },
  };
}

/** The source as `get_item` answers it at `revision`. */
export function sourceDocument(
  revision: string,
  extraction: Extraction,
  body: string = CONVERTED,
): ItemDocument {
  return {
    ...itemDocument(sourceAt(revision, extraction)),
    body,
    source: {
      extraction,
      media_type: 'application/pdf',
      metadata: {},
      names: [],
      object: OBJECT,
      size: '2048',
    },
  };
}

/** The accepted proposal the supplied text came from. */
export function proposal(markdown: string = SUPPLIED_MARKDOWN): Proposal {
  return {
    id: PROPOSAL,
    workspace_id: workspace.id,
    title: 'Supply the text of the annual report',
    description: 'Read from the scan by the agent.',
    kind: 'supply_extraction',
    status: 'accepted',
    base_revision: HEAD,
    proposal_revision: 'c'.repeat(40),
    content_digest: 'd'.repeat(64),
    created_at: NOW,
    created_by: 'agent-7',
    created_via: 'mcp_delegation',
    changes: [{ kind: 'supply_extraction', item_id: SOURCE, markdown, based_on: DIGEST }],
  };
}

/** A reconversion job of the source in `state`. */
export function redigestJob(state: Job['state'], extra: Partial<Job> = {}): Job {
  return {
    id: REDIGEST_JOB,
    kind: 'redigest',
    state,
    attempt: 1,
    progress: state === 'succeeded' ? 100 : 0,
    item_ids: state === 'succeeded' ? [SOURCE] : [],
    warnings: [],
    workspace_id: workspace.id,
    created_at: '2026-01-02T10:00:00.000Z',
    ...extra,
  };
}

const text = (body: Json, key: string): string => {
  const value = body[key];
  return typeof value === 'string' ? value : '';
};
const revisionAsked = (body: Json, head: string): string => {
  const at = body.at;
  return at !== null &&
    typeof at === 'object' &&
    'revision' in at &&
    typeof at.revision === 'string'
    ? at.revision
    : head;
};

export interface CorrectionsService {
  handlers: Record<string, Handler>;
  /** The head the service holds now. */
  head: () => string;
  /** Move the head, as someone else's change does. */
  moveHead: (to: string) => void;
}

export interface ServiceOptions {
  extraction?: Extraction;
  body?: string;
  permissions?: string[];
  archivedAt?: string | null;
  /** Answer each `correct_digest` with this in place of the service's own answer. */
  correct?: Handler;
  overrides?: Record<string, Handler>;
}

/**
 * The service: the source at the head with `extraction`, and a correction committed at
 * `CORRECTED_AT` (then the next revisions), each revision holding what it was given.
 */
export function correctionsService(options: ServiceOptions = {}): CorrectionsService {
  const first = options.extraction ?? extractionOf();
  const revisions = new Map<string, ItemDocument>([
    [HEAD, sourceDocument(HEAD, first, options.body ?? CONVERTED)],
  ]);
  const made = [CORRECTED_AT, 'd'.repeat(40), '8'.repeat(40)];
  const state = { head: HEAD, made: 0 };
  const at = (revision: string): ItemDocument | undefined => revisions.get(revision);
  const commit = (document: ItemDocument): string => {
    const next = made.at(state.made) ?? '7'.repeat(40);
    state.made += 1;
    revisions.set(next, { ...document, summary: { ...document.summary, revision: next } });
    state.head = next;
    return next;
  };
  const correct: Handler = (body) => {
    if (text(body, 'base_revision') !== state.head) {
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
    const current = at(state.head);
    const extraction = current?.source?.extraction;
    if (current === undefined || extraction === undefined) {
      return fail(404, 'not_found', 'No item with that identity here');
    }
    if (extraction.digest !== text(body, 'digest')) {
      return {
        status: 400,
        body: {
          code: 'invalid_input',
          message: 'the digest is not this item’s at the base revision',
          field: '/digest',
        },
      };
    }
    const corrected = text(body, 'corrected_markdown');
    const adopts = body.adopts;
    if (typeof adopts === 'string' && corrected !== SUPPLIED_MARKDOWN) {
      return {
        status: 409,
        body: {
          code: 'conflict',
          message: 'the corrected text is not the text the proposal supplied',
          field: '/adopts',
        },
      };
    }
    const next: Extraction =
      typeof adopts === 'string'
        ? {
            ...extraction,
            corrected: false,
            text_origin: 'supplied_by_agent',
            supplied: { ...SUPPLIED, kept_by: 'ada' },
          }
        : { ...extraction, corrected: true, text_origin: 'converter', supplied: null };
    const revision = commit(sourceDocument(state.head, next, corrected.replace(/\n$/, '')));
    return ok({ revision, receipt_id: RECEIPT, warnings: [] });
  };
  const opened = {
    ...writableWorkspace,
    permissions: options.permissions ?? writableWorkspace.permissions,
    ...(options.archivedAt ? { archived_at: options.archivedAt } : {}),
  };
  const handlers: Record<string, Handler> = explorerHandlers({
    [OPEN]: () => ok({ ...opened, head: state.head }),
    [LIST]: (body) => {
      const document = at(revisionAsked(body, state.head)) ?? at(HEAD);
      return ok({
        ...listing([], document === undefined ? [] : [document.summary]),
        revision: revisionAsked(body, state.head),
      });
    },
    [ITEM]: (body): Reply => {
      const document = at(revisionAsked(body, state.head));
      return document === undefined
        ? fail(404, 'not_found', 'No item with that identity here')
        : ok(document);
    },
    [READ]: (body): Reply => {
      const revision = revisionAsked(body, state.head);
      const document = at(revision);
      if (document === undefined) return fail(404, 'not_found', 'No item with that identity here');
      const extraction = document.source?.extraction;
      return ok(
        readResponse(document.summary, document.body, {
          ...(extraction === undefined
            ? {}
            : {
                extraction: {
                  status: extraction.outcome.status,
                  text_origin: extraction.text_origin,
                  corrected: extraction.corrected,
                },
              }),
        }),
      );
    },
    [GET_PROPOSAL]: () => ok(proposal()),
    [CORRECT_DIGEST]: options.correct ?? correct,
    [REDIGEST_ITEM]: () => ({ status: 202, body: redigestJob('queued') }),
    [JOB]: () => ok(redigestJob('running')),
    ...options.overrides,
  });
  return {
    handlers,
    head: () => state.head,
    moveHead: (to: string) => {
      const current = at(state.head);
      if (current !== undefined) {
        revisions.set(to, { ...current, summary: { ...current.summary, revision: to } });
      }
      state.head = to;
    },
  };
}
