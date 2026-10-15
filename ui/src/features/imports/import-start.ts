/**
 * Starting the import of the uploaded files (`start_import`) into the folder the person chose.
 *
 * The request names the completed upload slots, the destination folder as it was typed, less the
 * spaces around it (empty for the top of the workspace), and the revision the import is based on.
 * The contract states what that base is (ruling 26): the revision the import was prepared
 * against, where card names are chosen. A head that moved since does not refuse the import; a card
 * path taken since fails the job, which is not retried.
 *
 * The base. It is the revision the workspace shows, with one exception: once an import started
 * from this page, under the revision still shown, has finished and reports the revision it made,
 * the next import is based on that revision (`importBase`). The page's own import moved the head,
 * and the revision shown waits for Refresh as always; card names chosen at the older one would
 * miss the files that import added. After a Refresh the base is the revision shown again.
 * Converter settings are left out, which means the deployment's own. Naming rules are not applied
 * (`apply_naming_rules: false`): the wire applies them "after preview acceptance", and no preview
 * is part of this page, so files keep the names they came with.
 *
 * Parts. One request names at most `MAX_IMPORT_UPLOADS` uploads (ruling 15). More uploads are
 * imported in parts of at most that many, in order, one `start_import` and one job each; the page
 * says so before the person presses. A part that failed stops the rest; its retry starts that part
 * again under its key, then the parts after it. Parts already answered are not asked again.
 *
 * The answer of each part is a job, not a finished import. It is told to the cache at once, in the
 * state it reports (`rememberJob`), and remembered as started here; the files it took leave the
 * list, since an upload is the input of one job, and the uploads the service holds are listed
 * again (`held-uploads.ts`).
 *
 * Idempotency: the request is repeated under the keys it had while it names the same uploads, the
 * same folder and the same base, so an import whose answer was lost is the same jobs, not more.
 */
import type { QueryClient } from '@tanstack/react-query';
import { startImport } from '../../api/generated/sdk.gen';
import type { Job, Revision } from '../../api/generated/types.gen';
import { rememberJob } from '../jobs/jobs';
import { heldUploadsChanged } from './held-uploads';
import { useUploads } from './upload-list';

/**
 * The most uploads one `start_import` names: the contract's `MAX_IMPORT_UPLOADS` (ruling 15),
 * the request schema's `maxItems` for `upload_ids` (a unit test holds the two equal).
 */
export const MAX_IMPORT_UPLOADS = 5_000;

/** One `start_import` request of an import, and the job it answered once it has. */
export interface ImportPart {
  key: string;
  uploadIds: string[];
  job: Job | null;
}

/** One import: its uploads in parts, each one request. */
export interface ImportAttempt {
  parts: ImportPart[];
  destination: string;
  baseRevision: Revision;
}

/** The uploads of an attempt, in order. */
export const uploadsOf = (attempt: ImportAttempt) =>
  attempt.parts.flatMap((part) => part.uploadIds);

/** `uploadIds` in parts of at most `MAX_IMPORT_UPLOADS`, in order. */
export function partsOf(uploadIds: readonly string[]): string[][] {
  const parts: string[][] = [];
  for (let at = 0; at < uploadIds.length; at += MAX_IMPORT_UPLOADS) {
    parts.push(uploadIds.slice(at, at + MAX_IMPORT_UPLOADS));
  }
  return parts;
}

/** The request for these uploads, this folder and this base: the last one again while the same. */
export function importAttempt(
  last: ImportAttempt | null,
  uploadIds: string[],
  destination: string,
  baseRevision: Revision,
): ImportAttempt {
  const asked = last === null ? [] : uploadsOf(last);
  const same =
    last !== null &&
    last.destination === destination &&
    last.baseRevision === baseRevision &&
    asked.length === uploadIds.length &&
    asked.every((id, index) => id === uploadIds[index]);
  if (same) return last;
  return {
    parts: partsOf(uploadIds).map((part) => ({
      key: crypto.randomUUID(),
      uploadIds: part,
      job: null,
    })),
    destination,
    baseRevision,
  };
}

/**
 * The revision the next import is based on. `mine`: the jobs started from this page under the
 * revision now shown, newest first, as last reported (undefined when not read yet).
 */
export function importBase(shown: Revision, mine: ReadonlyArray<Job | undefined>): Revision {
  for (const job of mine) {
    const made = job?.state === 'succeeded' ? (job.revision ?? null) : null;
    if (made !== null) return made;
  }
  return shown;
}

/**
 * Start the import, part by part: each part not answered yet, in order. `shown` is the revision
 * the workspace shows now. Resolves with the job of every part. Rejects with what a part threw;
 * the parts before it keep their jobs in the attempt, so its retry starts from that part.
 */
export async function beginImport(
  client: QueryClient,
  workspaceId: string,
  attempt: ImportAttempt,
  shown: Revision,
): Promise<Job[]> {
  for (const part of attempt.parts) {
    if (part.job !== null) continue;
    const { data: job } = await startImport({
      body: {
        workspace_id: workspaceId,
        upload_ids: part.uploadIds,
        destination: attempt.destination,
        base_revision: attempt.baseRevision,
        apply_naming_rules: false,
        idempotency_key: part.key,
      },
      throwOnError: true,
    });
    part.job = job;
    rememberJob(client, workspaceId, job);
    const uploads = useUploads.getState();
    uploads.remove(
      uploads.entries
        .filter(
          (entry) =>
            entry.workspaceId === workspaceId &&
            entry.uploadId !== null &&
            part.uploadIds.includes(entry.uploadId),
        )
        .map((entry) => entry.id),
    );
    uploads.jobStarted(workspaceId, job.id, shown);
    heldUploadsChanged(client, workspaceId);
  }
  return attempt.parts.flatMap((part) => (part.job === null ? [] : [part.job]));
}
