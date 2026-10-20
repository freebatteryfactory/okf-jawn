/**
 * The requests of a workspace's details and lifecycle (ruling C9): its name and description
 * (`update_workspace`, `write`), Archive and Unarchive (`archive_workspace`,
 * `unarchive_workspace`, `admin`), and a rebuild of its search index (`rebuild_index`, `admin`).
 *
 * The name, the description and the archive are the workspace's own record, which the shell reads
 * with `open_workspace`: after any of them the shell reads it again (`WorkspaceChanges.refresh`,
 * the shell's Refresh), so it shows the workspace as the service now has it, its name, its
 * description, whether it is archived and its head, and nothing is copied from what was sent.
 * Until that answer, the revision the change returned becomes the head shown when the shell
 * still shows the head the change was made on (`adoptHeadFrom`), as for every change of the
 * workspace, and the head checks run again.
 *
 * A change and an archive carry the head the page shows as their `base_revision`, so the service
 * refuses them as a `conflict` when the head has moved; Unarchive carries none (the contract's
 * request has no base). Each is repeated under the key it had while it asks the same thing on the
 * same base (`tree/changes.ts`'s `Attempt`, through `useChange`).
 *
 * A rebuild answers 202 with its job, never with a finished index: the job is told to the cache
 * at once (`rememberJob`), so the Jobs page lists it as it lists every job, in the state the
 * service reported. It is repeated under its key until a job answers it (`useExportStart`), so a
 * request whose answer was lost does not start a second rebuild.
 */
import type { QueryClient } from '@tanstack/react-query';
import {
  archiveWorkspace,
  rebuildIndex,
  unarchiveWorkspace,
  updateWorkspace,
} from '../../../api/generated/sdk.gen';
import type { Job, Revision } from '../../../api/generated/types.gen';
import { adoptHeadFrom, checkHeadsAgain, type HeadChange } from '../../explorer/head';
import type { ExportAttempt } from '../../exports/exports';
import { rememberJob } from '../../jobs/jobs';
import type { Attempt } from '../../tree/changes';

/** A workspace's name and description, as a person types them. */
export interface Details {
  name: string;
  description: string;
}

/** Archive the workspace (`true`), or unarchive it. */
export interface Archiving {
  archive: boolean;
}

/** What a rebuild is asked for: the workspace whose index it rebuilds. */
export interface Rebuild {
  workspaceId: string;
}

/** The shell shows the revision a change made when it still shows the head it was made on. */
function changed(
  client: QueryClient,
  workspaceId: string,
  base: Revision,
  revision: Revision,
): HeadChange {
  const { previousHead, adopted } = adoptHeadFrom(client, workspaceId, revision, base);
  checkHeadsAgain(client, workspaceId);
  return { revision, previousHead, itemIds: [], adopted };
}

/** Change the workspace's name and description, on the head the page shows. */
export async function updateDetails(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<Details>,
): Promise<HeadChange> {
  const { data } = await updateWorkspace({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      name: attempt.values.name,
      description: attempt.values.description,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  return changed(client, workspaceId, attempt.base, data.revision);
}

/** Archive the workspace on the head the page shows, or unarchive it. */
export async function setArchived(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<Archiving>,
): Promise<HeadChange> {
  const { data } = attempt.values.archive
    ? await archiveWorkspace({
        body: {
          workspace_id: workspaceId,
          base_revision: attempt.base,
          idempotency_key: attempt.key,
        },
        throwOnError: true,
      })
    : await unarchiveWorkspace({
        body: { workspace_id: workspaceId, idempotency_key: attempt.key },
        throwOnError: true,
      });
  return changed(client, workspaceId, attempt.base, data.revision);
}

/** Start a rebuild of the workspace's search and link index; its job goes into the cache. */
export async function startRebuild(
  client: QueryClient,
  attempt: ExportAttempt<Rebuild>,
): Promise<Job> {
  const { workspaceId } = attempt.values;
  const { data: job } = await rebuildIndex({
    body: { workspace_id: workspaceId, idempotency_key: attempt.key },
    throwOnError: true,
  });
  rememberJob(client, workspaceId, job);
  return job;
}
