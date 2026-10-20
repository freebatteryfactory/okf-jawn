/**
 * Administration (UI M4c): the operations its pages ask, added to the mocked client's contract so
 * every request and answer of a test is held to the generated schemas, and the answers the
 * Administration tests share: a workspace whose grant includes `admin`, and a session on a route.
 */
import type { AccessRoute, Workspace } from '../../../src/api/generated/types.gen';
import {
  zArchiveWorkspaceRequest,
  zBackupInstallationRequest,
  zBackupWorkspaceRequest,
  zConnector,
  zCreateConnectorRequest,
  zCreateWorkspaceRequest,
  zGetPurgeRequest,
  zGetTenantJobRequest,
  zIssuedConnector,
  zJob,
  zListConnectorsRequest,
  zListConnectorsResponse,
  zListEventsRequest,
  zListEventsResponse,
  zListJobsResponse,
  zListTenantEventsRequest,
  zListTenantJobsRequest,
  zMutationResult,
  zPurge,
  zPurgeItemRequest,
  zPurgeWorkspaceRequest,
  zRebuildIndexRequest,
  zRestoreWorkspaceRequest,
  zRevokeConnectorRequest,
  zUnarchiveWorkspaceRequest,
  zUpdateWorkspaceRequest,
  zWorkspace,
} from '../../../src/api/generated/zod.gen';
import { CONTRACT, type Json } from './api';
import { workspace } from './workspace';

export const CREATE_WORKSPACE = '/api/workspaces/create-workspace';
export const UPDATE_WORKSPACE = '/api/workspaces/update-workspace';
export const ARCHIVE_WORKSPACE = '/api/workspaces/archive-workspace';
export const UNARCHIVE_WORKSPACE = '/api/workspaces/unarchive-workspace';
export const PURGE_WORKSPACE = '/api/workspaces/purge-workspace';
export const GET_PURGE = '/api/workspaces/get-purge';
export const PURGE_ITEM = '/api/items/purge-item';
export const BACKUP_WORKSPACE = '/api/workspaces/backup-workspace';
export const RESTORE_WORKSPACE = '/api/workspaces/restore-workspace';
export const BACKUP_INSTALLATION = '/api/workspaces/backup-installation';
export const GET_TENANT_JOB = '/api/imports/get-tenant-job';
export const LIST_TENANT_JOBS = '/api/imports/list-tenant-jobs';
export const REBUILD_INDEX = '/api/attention/rebuild-index';
export const LIST_EVENTS = '/api/events/list-events';
export const LIST_TENANT_EVENTS = '/api/events/list-tenant-events';
export const CREATE_CONNECTOR = '/api/session/create-connector';
export const LIST_CONNECTORS = '/api/session/list-connectors';
export const REVOKE_CONNECTOR = '/api/session/revoke-connector';
export const SESSION = '/api/session/get-session';
export const WORKSPACES = '/api/workspaces/list-workspaces';

Object.assign(CONTRACT, {
  [CREATE_WORKSPACE]: { request: zCreateWorkspaceRequest, response: zWorkspace },
  [UPDATE_WORKSPACE]: { request: zUpdateWorkspaceRequest, response: zMutationResult },
  [ARCHIVE_WORKSPACE]: { request: zArchiveWorkspaceRequest, response: zMutationResult },
  [UNARCHIVE_WORKSPACE]: { request: zUnarchiveWorkspaceRequest, response: zMutationResult },
  [PURGE_WORKSPACE]: { request: zPurgeWorkspaceRequest, response: zPurge },
  [GET_PURGE]: { request: zGetPurgeRequest, response: zPurge },
  [PURGE_ITEM]: { request: zPurgeItemRequest, response: zPurge },
  [BACKUP_WORKSPACE]: { request: zBackupWorkspaceRequest, response: zJob },
  [RESTORE_WORKSPACE]: { request: zRestoreWorkspaceRequest, response: zJob },
  [BACKUP_INSTALLATION]: { request: zBackupInstallationRequest, response: zJob },
  [GET_TENANT_JOB]: { request: zGetTenantJobRequest, response: zJob },
  [LIST_TENANT_JOBS]: { request: zListTenantJobsRequest, response: zListJobsResponse },
  [REBUILD_INDEX]: { request: zRebuildIndexRequest, response: zJob },
  [LIST_EVENTS]: { request: zListEventsRequest, response: zListEventsResponse },
  [LIST_TENANT_EVENTS]: { request: zListTenantEventsRequest, response: zListEventsResponse },
  [CREATE_CONNECTOR]: { request: zCreateConnectorRequest, response: zIssuedConnector },
  [LIST_CONNECTORS]: { request: zListConnectorsRequest, response: zListConnectorsResponse },
  [REVOKE_CONNECTOR]: { request: zRevokeConnectorRequest, response: zConnector },
});

/** The Explorer's workspace, with `admin` in the person's grant there. */
export const adminWorkspace: Workspace = {
  ...workspace,
  permissions: ['read', 'write', 'propose', 'approve', 'review', 'admin'],
};

/** A `get_session` answer for a person who came by `route`. */
export function sessionOn(route: AccessRoute): Json {
  return {
    csrf_required: route === 'browser_session',
    limits: {
      item_text_bytes: 1_048_576,
      request_body_bytes: 4_194_304,
      upload_bytes: '1073741824',
    },
    principal: {
      route,
      subject: route === 'local_owner' ? 'local-owner' : 'user_01',
      tenant_id: 'tenant_01',
    },
  };
}

/** A `list_workspaces` answer holding `items`, with no further page. */
export function workspacesPage(items: readonly Workspace[]): Json {
  return { items, next_cursor: null };
}
