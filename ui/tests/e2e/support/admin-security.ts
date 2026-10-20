/**
 * Ground for the connectors and security-events journeys (rulings C5 and C8): the service's
 * connector operations as a local installation keeps them (one connector per idempotency key, its
 * secret answered once, revoked ones listed only when asked), the session of the local owner, and
 * the installation's log in two pages. Answered through `page.route` (`routeApi`), as every
 * Explorer spec is.
 */

import { type Answers, ok, WS, workspace } from './explorer';

type Body = Record<string, unknown>;

export const SECRET = 'okfc_7k2m9q4w8x1z5v3n6b0p';
export const ISSUED_AT = '2026-10-19T08:00:00.000Z';
export const REVOKED_AT = '2026-10-19T09:30:00.000Z';
export const DESKTOP = '77777777-7777-4777-8777-777777777777';
export const MADE = 'aaaaaaaa-3333-4333-8333-333333333333';

/** The Explorer's workspace, with `admin` in the person's grant there. */
export const adminWorkspace = {
  ...workspace,
  permissions: ['read', 'write', 'propose', 'approve', 'review', 'admin'],
};

/** A `get_session` answer for the installation's local owner. */
export const localOwner = {
  csrf_required: false,
  limits: { item_text_bytes: 1_048_576, request_body_bytes: 4_194_304, upload_bytes: '1073741824' },
  principal: { route: 'local_owner', subject: 'local-owner', tenant_id: 'tenant_01' },
};

export interface ConnectorService {
  connectors: Body[];
  /** Every `create_connector` request, in order. */
  issues: Body[];
  /** Every `revoke_connector` request, in order. */
  revocations: Body[];
  answers: Answers;
}

/** The connector operations, over one connector of this workspace ("Desktop app"). */
export function connectorService(): ConnectorService {
  const byKey = new Map<string, Body>();
  const service: ConnectorService = {
    connectors: [
      {
        connector_id: DESKTOP,
        label: 'Desktop app',
        workspace_ids: [WS],
        permissions: ['read'],
        created_at: ISSUED_AT,
      },
    ],
    issues: [],
    revocations: [],
    answers: {},
  };
  service.answers = {
    '/api/session/list-connectors': (body) =>
      ok({
        connectors: service.connectors.filter(
          (connector) => body.include_revoked === true || !connector.revoked_at,
        ),
      }),
    '/api/session/create-connector': (body) => {
      service.issues.push(body);
      const key = String(body.idempotency_key);
      const known = byKey.get(key);
      if (known !== undefined) {
        return {
          status: 409,
          body: {
            code: 'already_issued',
            message: 'Connector was already issued under this idempotency key',
            detail: { kind: 'already_issued', connector_id: known.connector_id },
          },
        };
      }
      const made: Body = {
        connector_id: MADE,
        label: body.label,
        workspace_ids: body.workspace_ids,
        permissions: body.allow_propose === true ? ['read', 'propose'] : ['read'],
        created_at: ISSUED_AT,
      };
      byKey.set(key, made);
      service.connectors.push(made);
      return ok({ connector: made, secret: SECRET });
    },
    '/api/session/revoke-connector': (body) => {
      service.revocations.push(body);
      const found = service.connectors.find(
        (connector) => connector.connector_id === body.connector_id,
      );
      if (found === undefined) {
        return { status: 404, body: { code: 'not_found', message: 'No such connector' } };
      }
      found.revoked_at = found.revoked_at ?? REVOKED_AT;
      return ok(found);
    },
  };
  return service;
}

const event = (id: string, kind: string, extra: Body = {}): Body => ({
  id,
  kind,
  at: '2026-10-19T08:00:00.000Z',
  ...extra,
});

const owner = { subject: 'local-owner', route: 'local_owner' };

/** The installation's log: a first page with a continuation, then the last page. */
export const tenantPages: Body[] = [
  {
    events: [
      event('1', 'signed_in', { actor: owner }),
      event('2', 'connector_issued', { actor: owner, connector_id: DESKTOP }),
      event('3', 'sign_in_refused', { count: 12 }),
    ],
    next_cursor: '3',
  },
  {
    events: [
      event('4', 'job_updated', { job_id: '88888888-8888-4888-8888-888888888888' }),
      event('5', 'permission_denied', {
        actor: { subject: 'agent-7', route: 'mcp_delegation', client_id: 'desktop-app' },
        operation: 'list_tenant_events',
      }),
      event('6', 'permission_denied', {
        actor: { subject: 'user_02', route: 'browser_session' },
        transport: 'download_tenant_artifact',
        count: 3,
      }),
      event('7', 'connector_revoked', { actor: owner, connector_id: DESKTOP }),
    ],
    next_cursor: null,
  },
];

/** A workspace's log: two refused requests among its content changes. */
export const workspacePage: Body = {
  events: [
    event('10', 'changed', { revision: 'a'.repeat(40) }),
    event('11', 'permission_denied', {
      actor: { subject: 'agent-7', route: 'mcp_delegation', client_id: 'desktop-app' },
      operation: 'purge_workspace',
      workspace_id: WS,
    }),
    event('12', 'permission_denied', {
      actor: { subject: 'user_02', route: 'browser_session' },
      transport: 'download_artifact',
      workspace_id: WS,
      count: 40,
    }),
  ],
  next_cursor: null,
};

/** Answer `list_tenant_events` from `tenantPages` by the request's cursor; record each request. */
export function tenantLog(asked: Body[]): Answers {
  return {
    '/api/events/list-tenant-events': (body) => {
      asked.push(body);
      const page = body.page as { cursor?: string } | undefined;
      if (body.after !== undefined) return ok({ events: [], next_cursor: null });
      return ok(page?.cursor === '3' ? tenantPages[1] : tenantPages[0]);
    },
  };
}
