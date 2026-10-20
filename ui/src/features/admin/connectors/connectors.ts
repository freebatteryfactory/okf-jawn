/**
 * Local connector credentials in the cache and on the wire (ruling C5): the installation's list
 * (`list_connectors`), issuing one (`create_connector`) and revoking one (`revoke_connector`).
 *
 * The secret of a new connector never reaches a store. `issueConnector` hands the answer's secret
 * to the caller's `reveal` (the state of the component that shows it, once) and gives the mutation
 * only the connector, so neither the query cache nor the mutation cache ever holds it; the list is
 * asked again rather than told the new connector (`listedAgain`).
 *
 * All three operations are the local owner's only (`LOCAL_OWNER_ONLY`): the service refuses them
 * on every other route with `forbidden` before any grant is looked up, naming no request field
 * (`crates/core/src/access.rs`, `check_local_owner_route`; its message is "Connector credentials
 * are issued, listed and revoked only by the local owner"). That refusal, answered to a session
 * that is not known to be the local owner's, is the hosted refusal (`isHostedRefusal`). A
 * `forbidden` that names a request field is another refusal: the owner's own grant on a workspace
 * the connector would read (`/workspace_ids/<index>`, from dispatch's `name_refused_entry`).
 */
import { type QueryClient, queryOptions } from '@tanstack/react-query';
import { createConnector, listConnectors, revokeConnector } from '../../../api/generated/sdk.gen';
import type {
  AccessRoute,
  Connector,
  IssuedConnector,
  Permission,
} from '../../../api/generated/types.gen';
import { classifyFailure } from '../../errors/failure';
import type { Attempt } from '../destructive';

/** The generated helpers offer no query key for the list; this is its own. */
const LIST_ID = 'listConnectors';

/** The installation's connectors; revoked ones too when asked. */
export function connectorsOptions(includeRevoked: boolean) {
  const body = { include_revoked: includeRevoked };
  return queryOptions({
    queryKey: [{ _id: LIST_ID, body }] as const,
    queryFn: async ({ signal }) => {
      const { data } = await listConnectors({ body, signal, throwOnError: true });
      return data;
    },
  });
}

/** Ask every listing of the connectors again: one was issued or revoked. */
export function listedAgain(client: QueryClient): Promise<void> {
  return client.invalidateQueries({ queryKey: [{ _id: LIST_ID }] });
}

/** What issuing asks for. */
export interface IssueValues {
  workspaceId: string;
  label: string;
  allowPropose: boolean;
}

/**
 * Issue a connector reading `values.workspaceId`, under the attempt's key. Its secret goes to
 * `reveal` and nowhere else; the connector alone is answered.
 */
export async function issueConnector(
  attempt: Attempt<IssueValues>,
  reveal: (issued: IssuedConnector) => void,
): Promise<Connector> {
  const { data } = await createConnector({
    body: {
      label: attempt.values.label,
      allow_propose: attempt.values.allowPropose,
      workspace_ids: [attempt.values.workspaceId],
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  reveal(data);
  return data.connector;
}

/** What revoking is bound to: the connector the dialog was opened for, as it was shown. */
export interface RevokeValues {
  connectorId: string;
  name: string;
  rights: string;
  others: number;
  createdAt: string;
}

/** Revoke the connector the attempt is bound to, under the attempt's key. */
export async function revokeOne(attempt: Attempt<RevokeValues>): Promise<Connector> {
  const { data } = await revokeConnector({
    body: { connector_id: attempt.values.connectorId, idempotency_key: attempt.key },
    throwOnError: true,
  });
  return data;
}

/** The service's `forbidden` that names no request field: a refusal by the person's route. */
function routeRefusal(error: unknown): boolean {
  const failure = classifyFailure(error);
  return (
    failure.kind === 'api' &&
    failure.error.code === 'forbidden' &&
    (failure.error.field === null ||
      failure.error.field === undefined ||
      failure.error.field === '')
  );
}

/**
 * Whether the service refused because the person is not the local owner: its route refusal, to a
 * session not known to be the local owner's (`route`, from `get_session`; undefined while not
 * known). The local owner refused that way lacks the installation's `admin` instead.
 */
export function isHostedRefusal(error: unknown, route: AccessRoute | undefined): boolean {
  return route !== 'local_owner' && routeRefusal(error);
}

/** The service's `forbidden` naming a request field: the owner's grant on the workspace. */
export function isGrantRefusal(error: unknown): boolean {
  const failure = classifyFailure(error);
  return (
    failure.kind === 'api' &&
    failure.error.code === 'forbidden' &&
    typeof failure.error.field === 'string' &&
    failure.error.field !== ''
  );
}

/** The service answered that this request issued a connector already (a replay). */
export function alreadyIssued(error: unknown): string | null {
  const failure = classifyFailure(error);
  if (failure.kind !== 'api' || failure.error.code !== 'already_issued') return null;
  const detail = failure.error.detail;
  return detail?.kind === 'already_issued' ? detail.connector_id : '';
}

/** A connector's name, or how it is called without one. */
export function nameOf(connector: Connector): string {
  return connector.label.trim() === ''
    ? `connector ${connector.connector_id.slice(0, 8)}`
    : connector.label;
}

/** What a connector may do, in words: read, propose, and never verify or approve. */
export function rightsWords(permissions: readonly Permission[]): string {
  const may = [
    permissions.includes('read') ? 'read' : null,
    permissions.includes('propose') ? 'propose changes' : null,
  ].filter((part): part is string => part !== null);
  const granted = may.length === 0 ? 'May do nothing' : `May ${may.join(' and ')}`;
  return `${granted}; it can never verify or approve anything.`;
}

/** The other workspaces a connector reads besides `workspaceId`, counted in words. */
export function othersWords(connector: Connector, workspaceId: string): string | null {
  const others = connector.workspace_ids.filter((id) => id !== workspaceId).length;
  if (others === 0) return null;
  return `It also reads ${others} other ${others === 1 ? 'workspace' : 'workspaces'}.`;
}
