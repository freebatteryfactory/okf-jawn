/**
 * Starting the import of the uploaded files (`start_import`) into the folder the person chose.
 *
 * The request names the completed upload slots, the destination folder as it was typed, less the
 * spaces around it (empty for the top of the workspace), and the revision the workspace shows as
 * its base: the
 * service refuses a stale base as a `conflict`, and the page then says to refresh. Converter
 * settings are left out, which means the deployment's own. Naming rules are not applied
 * (`apply_naming_rules: false`): the wire applies them "after preview acceptance", and no preview
 * is part of this page, so files keep the names they came with.
 *
 * The answer is a job, not a finished import. It is told to the cache at once, in the state it
 * reports (`rememberJob`), and remembered as started here; the files it took leave the list, since
 * an upload is the input of one job.
 *
 * Idempotency: the request is repeated under the key it had while it names the same uploads, the
 * same folder and the same base, so an import whose answer was lost is the same job, not a second.
 */
import type { QueryClient } from '@tanstack/react-query';
import { startImport } from '../../api/generated/sdk.gen';
import type { Job, Revision } from '../../api/generated/types.gen';
import { rememberJob } from '../jobs/jobs';
import { useUploads } from './upload-list';

/** One `start_import` request. */
export interface ImportAttempt {
  key: string;
  uploadIds: string[];
  destination: string;
  baseRevision: Revision;
}

/** The request for these uploads, this folder and this base: the last one again while the same. */
export function importAttempt(
  last: ImportAttempt | null,
  uploadIds: string[],
  destination: string,
  baseRevision: Revision,
): ImportAttempt {
  const same =
    last !== null &&
    last.destination === destination &&
    last.baseRevision === baseRevision &&
    last.uploadIds.length === uploadIds.length &&
    last.uploadIds.every((id, index) => id === uploadIds[index]);
  return same ? last : { key: crypto.randomUUID(), uploadIds, destination, baseRevision };
}

/** Start the import. Resolves with the job as the service answered it. */
export async function beginImport(
  client: QueryClient,
  workspaceId: string,
  attempt: ImportAttempt,
): Promise<Job> {
  const { data: job } = await startImport({
    body: {
      workspace_id: workspaceId,
      upload_ids: attempt.uploadIds,
      destination: attempt.destination,
      base_revision: attempt.baseRevision,
      apply_naming_rules: false,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  rememberJob(client, workspaceId, job);
  const uploads = useUploads.getState();
  uploads.remove(
    uploads.entries
      .filter(
        (entry) =>
          entry.workspaceId === workspaceId &&
          entry.uploadId !== null &&
          attempt.uploadIds.includes(entry.uploadId),
      )
      .map((entry) => entry.id),
  );
  uploads.jobStarted(workspaceId, job.id);
  return job;
}
