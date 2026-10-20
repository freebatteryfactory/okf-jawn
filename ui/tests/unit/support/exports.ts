/**
 * Ground for the export and download tests: the two operations that start an export, added to the
 * contract the mocked fetch holds every request and answer to, and contract-valid export jobs and
 * artifacts.
 *
 * An export is durable work: both operations answer 202 with the `Job` they queued (`accepted`
 * in `imports.ts`). A job that ended with an artifact names it as the service does
 * (`DownloadArtifact`): its kind, its size as a decimal string, its SHA-256 and the
 * application-relative path it is downloaded from.
 */
import type { ArtifactKind, DownloadArtifact, Job } from '../../../src/api/generated/types.gen';
import {
  zExportViewRequest,
  zExportWorkspaceRequest,
  zJob,
} from '../../../src/api/generated/zod.gen';
import { CONTRACT } from './api';
import { jobOf } from './imports';
import { WS } from './workspace';

export const EXPORT_WORKSPACE = '/api/workspaces/export-workspace';
export const EXPORT_VIEW = '/api/views/export-view';

Object.assign(CONTRACT, {
  [EXPORT_WORKSPACE]: { request: zExportWorkspaceRequest, response: zJob },
  [EXPORT_VIEW]: { request: zExportViewRequest, response: zJob },
});

/** The export job's identity. */
export const EXPORT_JOB = '77777777-7777-4777-8777-777777777777';
/** The artifact it produced. */
export const ARTIFACT = '99999999-9999-4999-8999-999999999999';
/** The archive's SHA-256, as the service reports it. */
export const ARCHIVE_SHA = 'e'.repeat(64);

/** The artifact a job produced, workspace-scoped unless it is an installation backup. */
export function artifactOf(
  kind: ArtifactKind,
  extra: Partial<DownloadArtifact> = {},
): DownloadArtifact {
  return {
    artifact_id: ARTIFACT,
    kind,
    sha256: ARCHIVE_SHA,
    size: '2048',
    download_path:
      kind === 'installation_backup'
        ? `/api/artifacts/${ARTIFACT}`
        : `/api/workspaces/${WS}/artifacts/${ARTIFACT}`,
    ...extra,
  };
}

/** An export of the workspace, as the service records it. */
export function exportJobOf(state: Job['state'], extra: Partial<Job> = {}): Job {
  return jobOf(state, { id: EXPORT_JOB, kind: 'export_workspace', ...extra });
}
