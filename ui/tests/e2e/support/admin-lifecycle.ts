/**
 * The service a workspace's lifecycle and its purge are played against (UI M4c, rulings C3, C4 and
 * C9): an administrator's workspace whose archive the requests change, and a purge accepted 202
 * whose record (`get_purge`) is running when first read and completed, with every count of its
 * report, when read again. Each request is recorded as the page sent it.
 *
 * The operations are played by `page.route` through `routeApi` (no server exists in this lane);
 * every answer is the shape the generated schemas give it.
 */
import type { Page } from '@playwright/test';
import { type Answers, HEAD, ok, routeApi, WS, workspace } from './explorer';

export const PURGE_ID = '66666666-6666-4666-8666-666666666666';
export const PURGE_JOB = '77777777-7777-4777-8777-777777777777';
export const ARCHIVED_AT = '2026-01-02T08:00:00.000Z';
export const REQUESTED_AT = '2026-01-03T09:00:00.000Z';
export const COMPLETED_AT = '2026-01-03T09:05:00.000Z';

/** The workspace for a person whose grant there includes `admin`. */
export const adminWorkspace = {
  ...workspace,
  permissions: ['read', 'write', 'propose', 'approve', 'review', 'admin'],
};

type Body = Record<string, unknown>;

/** A purge's record in `state`; a completed one has its report. */
export function purgeOf(state: string): Body {
  return {
    id: PURGE_ID,
    job_id: PURGE_JOB,
    requested_at: REQUESTED_AT,
    requested_by: 'user_01',
    state,
    target: { kind: 'workspace', workspace_id: WS },
    ...(state === 'completed'
      ? {
          completed_at: COMPLETED_AT,
          report: {
            backups_removed: 2,
            backups_rewritten: 1,
            citations_invalidated: 14,
            citations_remapped: 0,
            drafts_removed: 3,
            exports_removed: 4,
            objects_kept_shared: '12',
            objects_removed: '1048576',
            proposals_removed: 2,
            revisions_invalidated: 37,
            views_invalidated: 1,
          },
        }
      : {}),
  };
}

const mutationAt = (revision: string) => ({
  revision,
  receipt_id: '55555555-5555-4555-8555-555555555555',
  warnings: [],
});

export interface LifecycleService {
  archives: Body[];
  unarchives: Body[];
  purges: Body[];
  /** `get_purge` requests answered. */
  reads: number;
}

/**
 * The service: the workspace as `open_workspace` answers it, archived or not as the last request
 * left it; a purge accepted as requested, read running once and completed after.
 */
export async function playLifecycle(page: Page, extra: Answers = {}): Promise<LifecycleService> {
  const service: LifecycleService = { archives: [], unarchives: [], purges: [], reads: 0 };
  let archivedAt: string | null = null;
  await routeApi(page, {
    ...extra,
    '/api/workspaces/open-workspace': () =>
      ok({ ...adminWorkspace, ...(archivedAt === null ? {} : { archived_at: archivedAt }) }),
    '/api/workspaces/archive-workspace': (body) => {
      service.archives.push(body);
      archivedAt = ARCHIVED_AT;
      return ok(mutationAt(HEAD));
    },
    '/api/workspaces/unarchive-workspace': (body) => {
      service.unarchives.push(body);
      archivedAt = null;
      return ok(mutationAt(HEAD));
    },
    '/api/workspaces/purge-workspace': (body) => {
      service.purges.push(body);
      return { status: 202, body: purgeOf('requested') };
    },
    '/api/workspaces/get-purge': () => {
      service.reads += 1;
      return ok(purgeOf(service.reads === 1 ? 'running' : 'completed'));
    },
    // Nothing waits for Attention here.
    '/api/attention/get-attention': () => ok({ revision: HEAD, items: [], next_cursor: null }),
  });
  return service;
}
