/**
 * Starting an export: of the workspace (`export_workspace`) or of one View (`export_view`). Both
 * need `write`, both run in an archived workspace too (`ALLOWED_WHILE_ARCHIVED`, see
 * `explorer/rights.tsx`), and both answer 202 with the job they queued, never with an archive: an
 * export is durable work, and its archive is offered from the job's row once the service reports
 * that the job succeeded with it (`jobs/JobRow.tsx`).
 *
 * Which version is exported. The workspace is asked for at the version the person chose: by
 * default its latest version (`at: latest`), which the service resolves once, when it takes the
 * request, and which may be newer than the version the page shows; or a version of its history,
 * by its resolved revision. For the latest, the wire's `Job` does not say which revision it was
 * (its `revision` is a revision the job committed, and an export commits none), so nothing here
 * names one. A View is asked for at the revision its tab reads, never at the latest.
 *
 * Idempotency, as for an import (`imports/import-start.ts`): a request is repeated under the key it
 * had while it asks the same thing (the same version and choice of history; the same View at the
 * same revision)
 * and no job has answered it, so a request whose answer was lost is answered with the job it
 * started, not a second one. Once a job has answered, the next press is a new export under a new
 * key.
 *
 * The job answered is told to the cache at once (`rememberJob`): the Jobs page lists it as it lists
 * every job, in the state the service reported.
 */
import type { QueryClient } from '@tanstack/react-query';
import { exportView, exportWorkspace } from '../../api/generated/sdk.gen';
import type { At, Job, Revision } from '../../api/generated/types.gen';
import { rememberJob } from '../jobs/jobs';

/** One request of an export, with the key it is sent under. */
export interface ExportAttempt<T> {
  key: string;
  values: T;
}

/** What an export of the workspace asks for: which version, and whether its history too. */
export interface WorkspaceExport {
  /** The latest version when the service takes the request, or a chosen resolved revision. */
  at: At;
  includeHistory: boolean;
}

/** Which View an export of a View is of, and at which revision. */
export interface ViewExport {
  itemId: string;
  revision: Revision;
}

/** The request for these values: the last one again while it asks the same, else a new key. */
export function exportAttempt<T>(last: ExportAttempt<T> | null, values: T): ExportAttempt<T> {
  const same = last !== null && JSON.stringify(last.values) === JSON.stringify(values);
  return same ? last : { key: crypto.randomUUID(), values };
}

/** Start an export of the workspace at the version asked for. */
export async function startWorkspaceExport(
  client: QueryClient,
  workspaceId: string,
  attempt: ExportAttempt<WorkspaceExport>,
): Promise<Job> {
  const { data: job } = await exportWorkspace({
    body: {
      workspace_id: workspaceId,
      at: attempt.values.at,
      include_history: attempt.values.includeHistory,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  rememberJob(client, workspaceId, job);
  return job;
}

/** Start an export of one View at the revision given. */
export async function startViewExport(
  client: QueryClient,
  workspaceId: string,
  attempt: ExportAttempt<ViewExport>,
): Promise<Job> {
  const { data: job } = await exportView({
    body: {
      workspace_id: workspaceId,
      item_id: attempt.values.itemId,
      at: { kind: 'revision', revision: attempt.values.revision },
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  rememberJob(client, workspaceId, job);
  return job;
}
