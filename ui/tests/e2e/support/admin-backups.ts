/**
 * Ground for the Administration browser specs of M4c (rulings C6 and C9): the services of creating
 * a workspace, backing one up, uploading an archive and restoring it, and backing up the
 * installation, played through `page.route` beside the Explorer's own answers (`explorer.ts`), as
 * contract-shaped answers; and a small HTTP server of the specs' own that receives the bytes of
 * an upload.
 *
 * The services keep the rules of the real ones that the journeys rely on: a job-starting
 * operation and `create_workspace` answer the job (or workspace) first made under an idempotency
 * key, and make nothing more; a slot is unique on its key, bytes are appended, and the finish is
 * refused unless the bytes that arrived have the announced length and SHA-256, computed here; a
 * restore is refused unless its upload is complete with the SHA-256 it names. A job is reported
 * queued when the service first answers it and the first time it is asked again, and finished
 * from the next time on, so a journey sees it move.
 *
 * Why a server for the bytes: Playwright does not show a route the body of a request whose body is
 * a file, so the byte upload's route sends the browser's own request on to this server, which
 * reads what the browser really sent (as `explorer-import.spec.ts` does). It is another origin
 * than the page's, so it answers the browser's CORS questions.
 */

import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { type Page, test } from '@playwright/test';
import {
  type Answers,
  answers,
  HEAD,
  NOW,
  ok,
  type Reply,
  routeApi,
  WS,
  workspace,
} from './explorer';

type Body = Record<string, unknown>;

/** The Explorer's workspace, with `admin` in the person's grant there. */
export const adminWorkspace = {
  ...workspace,
  permissions: ['read', 'write', 'propose', 'approve', 'review', 'admin'],
};
/** A blank workspace to restore into. */
export const BLANK = '12121212-1212-4121-8121-121212121212';
export const BLANK_HEAD = 'b'.repeat(40);
export const blankWorkspace = {
  ...adminWorkspace,
  id: BLANK,
  name: 'Restored notes',
  description: 'A blank workspace for the restored notes',
  head: BLANK_HEAD,
};
/** The workspace `create_workspace` makes. */
export const CREATED = '13131313-1313-4131-8131-131313131313';

export const BACKUP_JOB = 'bbbbbbbb-7777-4777-8777-777777777777';
export const RESTORE_JOB = 'cccccccc-7777-4777-8777-777777777777';
export const INSTALLATION_JOB = 'dddddddd-7777-4777-8777-777777777777';
const BACKUP_ARTIFACT = '99999999-9999-4999-8999-999999999999';
const INSTALLATION_ARTIFACT = '98989898-9898-4989-8989-989898989898';
export const BACKUP_PATH = `/api/workspaces/${WS}/artifacts/${BACKUP_ARTIFACT}`;
export const INSTALLATION_PATH = `/api/artifacts/${INSTALLATION_ARTIFACT}`;

/** An empty zip archive (its end record alone) with a comment that makes it `size` bytes. */
export function emptyZip(size: number, fill = 0x20): Buffer {
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(size - 22, 20);
  return Buffer.concat([end, Buffer.alloc(size - 22, fill)]);
}

/** The workspace's backup archive, and the installation's. */
export const BACKUP_ARCHIVE = emptyZip(4096);
export const INSTALLATION_ARCHIVE = emptyZip(2048, 0x2e);
export const sha256Of = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');

/** What the restore did. */
export const REPORT = { items: 12, drafts_restored: 3, drafts_unassigned: 1, rows_left_out: 2 };

const refuse = (status: number, code: string, message: string, field?: string): Reply => ({
  status,
  body: { code, message, ...(field === undefined ? {} : { field }) },
});

const accepted = (job: Body): Reply => ({ status: 202, body: job });

/** A job as the service records it; a tenant job carries no workspace. */
function jobOf(id: string, kind: string, state: string, extra: Body = {}): Body {
  return {
    id,
    kind,
    state,
    attempt: 1,
    progress: state === 'succeeded' ? 100 : 0,
    item_ids: [],
    warnings: [],
    created_at: '2026-10-19T08:00:00.000Z',
    ...(kind === 'backup_installation'
      ? {}
      : { workspace_id: kind === 'restore_workspace' ? BLANK : WS }),
    ...(state === 'succeeded'
      ? { started_at: '2026-10-19T08:00:01.000Z', finished_at: '2026-10-19T08:00:04.000Z' }
      : {}),
    ...extra,
  };
}

const finishedBackup = () =>
  jobOf(BACKUP_JOB, 'backup_workspace', 'succeeded', {
    artifact: {
      artifact_id: BACKUP_ARTIFACT,
      kind: 'workspace_backup',
      sha256: sha256Of(BACKUP_ARCHIVE),
      size: String(BACKUP_ARCHIVE.length),
      download_path: BACKUP_PATH,
    },
    backup: { rows_left_out: 0 },
  });

const finishedRestore = () =>
  jobOf(RESTORE_JOB, 'restore_workspace', 'succeeded', {
    restore: REPORT,
    revision: 'e'.repeat(40),
  });

const finishedInstallationBackup = () =>
  jobOf(INSTALLATION_JOB, 'backup_installation', 'succeeded', {
    artifact: {
      artifact_id: INSTALLATION_ARTIFACT,
      kind: 'installation_backup',
      sha256: sha256Of(INSTALLATION_ARCHIVE),
      size: String(INSTALLATION_ARCHIVE.length),
      download_path: INSTALLATION_PATH,
    },
  });

/** One upload slot as the service holds it. */
interface Slot {
  id: string;
  key: string;
  expected: number;
  announced: string | null;
  parts: Buffer[];
  complete: boolean;
  filename: string;
  /** What `create_upload` first answered: a repeat under its key answers this. */
  first: Body | null;
}

export interface Service {
  /** Every request of the operations these journeys study, by operation path, in order. */
  sent: Record<string, Body[]>;
  slots: Slot[];
  /** The workspaces `create_workspace` made, by key. */
  created: Map<string, Body>;
  /** How many times each job was asked about since it was accepted. */
  asked: Map<string, number>;
}

const viewOf = (slot: Slot, workspaceId: string): Body => ({
  id: slot.id,
  workspace_id: workspaceId,
  upload_path: `/api/workspaces/${workspaceId}/uploads/${slot.id}/content`,
  received_bytes: String(slot.parts.reduce((sum, part) => sum + part.length, 0)),
  complete: slot.complete,
  filename: slot.filename,
  size: String(slot.expected),
});

/** The service the test now running plays; the upload sink appends to its slots. */
let playing: Service | null = null;

/**
 * Receive the bytes of each upload for the specs of the calling describe block: append them to
 * the slot the request names, and answer the slot as it then stands.
 */
export function useUploadSink(): { origin: () => string } {
  let origin = '';
  let close: () => void = () => {};
  test.beforeAll(async () => {
    const sink = createServer((request, response) => {
      // WebKit holds a request sent on to another address to the rules of a cross-origin one.
      const cors = {
        'access-control-allow-origin': request.headers.origin ?? '*',
        'access-control-allow-credentials': 'true',
        'access-control-allow-methods': 'PUT',
        'access-control-allow-headers':
          request.headers['access-control-request-headers'] ?? 'content-type, x-okf-csrf',
      };
      if (request.method === 'OPTIONS') {
        response.writeHead(204, cors);
        response.end();
        return;
      }
      const chunks: Buffer[] = [];
      request.on('data', (chunk: Buffer) => chunks.push(chunk));
      request.on('end', () => {
        const slot = playing?.slots.find((candidate) => request.url?.includes(candidate.id));
        const workspaceId = request.url?.split('/').at(3) ?? '';
        if (!slot || request.method !== 'PUT') {
          response.writeHead(404, { ...cors, 'content-type': 'application/json' });
          response.end('{"code":"not_found","message":"no such upload"}');
          return;
        }
        slot.parts.push(Buffer.concat(chunks));
        response.writeHead(200, { ...cors, 'content-type': 'application/json' });
        response.end(JSON.stringify(viewOf(slot, workspaceId)));
      });
    });
    await new Promise<void>((resolve) => sink.listen(0, '127.0.0.1', () => resolve()));
    const address = sink.address();
    if (!address || typeof address === 'string') throw new Error('sink address unavailable');
    origin = `http://127.0.0.1:${address.port}`;
    close = () => sink.close();
  });
  test.afterAll(() => close());
  return { origin: () => origin };
}

/** Queued when first answered and when first asked again; finished from then on. */
function moving(service: Service, id: string, queued: Body, finished: () => Body): Body {
  const times = (service.asked.get(id) ?? 0) + 1;
  service.asked.set(id, times);
  return times <= 1 ? queued : finished();
}

/**
 * Play the services for a journey: the person is the local owner unless `route` says otherwise,
 * an administrator of the Explorer's workspace and of a blank one. `sink` receives the bytes of
 * an upload (`useUploadSink`), when the journey uploads one.
 */
export async function play(
  page: Page,
  options: { sink?: string; route?: string; extra?: Answers } = {},
): Promise<Service> {
  const service: Service = { sent: {}, slots: [], created: new Map(), asked: new Map() };
  playing = service;
  const note = (path: string, body: Body) => {
    service.sent[path] = [...(service.sent[path] ?? []), body];
  };
  const route = options.route ?? 'local_owner';
  const jobs: Map<string, Body> = new Map();
  await routeApi(page, {
    '/api/session/get-session': () =>
      ok({
        csrf_required: false,
        limits: {
          item_text_bytes: 1_048_576,
          request_body_bytes: 4_194_304,
          upload_bytes: '1073741824',
        },
        principal: { route, subject: 'local-owner', tenant_id: 'tenant_01' },
      }),
    '/api/workspaces/list-workspaces': () =>
      ok({ items: [adminWorkspace, blankWorkspace], next_cursor: null }),
    '/api/workspaces/open-workspace': (body) => {
      if (body.workspace_id === BLANK) return ok(blankWorkspace);
      const made = [...service.created.values()].find((one) => one.id === body.workspace_id);
      return ok(made ?? adminWorkspace);
    },
    // The Explorer's workspace holds its notes; the blank one, and one just made, hold nothing.
    '/api/items/list-items': (body) =>
      body.workspace_id === WS
        ? (answers()['/api/items/list-items']?.(body) ?? ok({}))
        : ok({
            folders: [],
            items: [],
            revision: body.workspace_id === BLANK ? BLANK_HEAD : 'f'.repeat(40),
          }),
    '/api/workspaces/create-workspace': (body) => {
      note('create_workspace', body);
      const key = String(body.idempotency_key);
      const made = service.created.get(key) ?? {
        id: CREATED,
        name: String(body.name),
        description: String(body.description),
        head: 'f'.repeat(40),
        created_at: NOW,
        permissions: ['read', 'write', 'propose', 'approve', 'review', 'admin'],
      };
      service.created.set(key, made);
      return ok(made);
    },
    '/api/workspaces/backup-workspace': (body) => {
      note('backup_workspace', body);
      const job = jobOf(BACKUP_JOB, 'backup_workspace', 'queued');
      jobs.set(BACKUP_JOB, job);
      return accepted(job);
    },
    '/api/workspaces/restore-workspace': (body) => {
      note('restore_workspace', body);
      const slot = service.slots.find((candidate) => candidate.id === body.upload_id);
      if (!slot?.complete)
        return refuse(409, 'conflict', 'the upload is not complete', '/upload_id');
      if (sha256Of(Buffer.concat(slot.parts)) !== body.sha256) {
        return refuse(
          409,
          'conflict',
          'the uploaded archive does not have the SHA-256 the request names',
          '/sha256',
        );
      }
      const job = jobOf(RESTORE_JOB, 'restore_workspace', 'queued');
      jobs.set(RESTORE_JOB, job);
      return accepted(job);
    },
    '/api/workspaces/backup-installation': (body) => {
      note('backup_installation', body);
      const job = jobOf(INSTALLATION_JOB, 'backup_installation', 'queued');
      jobs.set(INSTALLATION_JOB, job);
      return accepted(job);
    },
    '/api/imports/get-job': (body) => {
      const id = String(body.job_id);
      const job = jobs.get(id);
      if (job === undefined) return refuse(404, 'not_found', 'no such job');
      return ok(moving(service, id, job, id === BACKUP_JOB ? finishedBackup : finishedRestore));
    },
    '/api/imports/list-jobs': (body) =>
      ok({
        items: [...jobs.values()].filter((job) => job.workspace_id === body.workspace_id),
        next_cursor: null,
      }),
    '/api/imports/list-tenant-jobs': () => {
      const job = jobs.get(INSTALLATION_JOB);
      return ok({
        items:
          job === undefined
            ? []
            : [moving(service, INSTALLATION_JOB, job, finishedInstallationBackup)],
        next_cursor: null,
      });
    },
    '/api/imports/create-upload': (body) => {
      note('create_upload', body);
      const workspaceId = String(body.workspace_id);
      const known = service.slots.find((slot) => slot.key === body.idempotency_key);
      if (known) return ok(known.first ?? viewOf(known, workspaceId));
      const slot: Slot = {
        id: `77777777-7777-4777-8777-${String(service.slots.length + 1).padStart(12, '0')}`,
        key: String(body.idempotency_key),
        expected: Number(body.size),
        announced: typeof body.sha256 === 'string' ? body.sha256 : null,
        parts: [],
        complete: false,
        filename: String(body.filename),
        first: null,
      };
      slot.first = viewOf(slot, workspaceId);
      service.slots.push(slot);
      return ok(slot.first);
    },
    '/api/imports/get-upload': (body) => {
      const slot = service.slots.find((candidate) => candidate.id === body.upload_id);
      return slot
        ? ok(viewOf(slot, String(body.workspace_id)))
        : refuse(404, 'not_found', 'no such upload');
    },
    '/api/imports/complete-upload': (body) => {
      note('complete_upload', body);
      const slot = service.slots.find((candidate) => candidate.id === body.upload_id);
      if (!slot) return refuse(404, 'not_found', 'no such upload');
      const content = Buffer.concat(slot.parts);
      if (content.length !== slot.expected) {
        return refuse(409, 'conflict', `received ${content.length} bytes of ${slot.expected}`);
      }
      const digest = sha256Of(content);
      if (digest !== body.sha256 || (slot.announced !== null && slot.announced !== digest)) {
        return refuse(409, 'conflict', 'the digest differs from the one announced');
      }
      slot.complete = true;
      return ok(viewOf(slot, String(body.workspace_id)));
    },
    // The other sections of the Administration pages, answered empty.
    '/api/events/list-events': () => ok({ events: [], next_cursor: null }),
    '/api/events/list-tenant-events': () => ok({ events: [], next_cursor: null }),
    '/api/session/list-connectors': () => ok({ connectors: [] }),
    ...options.extra,
  });
  const sink = options.sink;
  if (sink !== undefined) {
    // The byte upload: registered after the operations, so it is the route that answers it.
    await page.route('**/api/workspaces/*/uploads/*/content', async (routed) => {
      await routed.continue({ url: `${sink}${new URL(routed.request().url()).pathname}` });
    });
  }
  return service;
}

export { HEAD, WS };
