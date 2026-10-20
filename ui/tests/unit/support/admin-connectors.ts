/**
 * Connectors (ruling C5) as a local service keeps them: one per idempotency key, its secret
 * answered once, a replay of a completed issue answered `already_issued` without it, revoked
 * ones listed only when asked. A test scripts the awkward answers, one step each.
 */
import type { Connector } from '../../../src/api/generated/types.gen';
import { fail, type Json, ok, type Reply } from './api';
import { WS } from './workspace';

export const OTHER_WS = '99999999-9999-4999-8999-999999999999';
export const ISSUED_AT = '2026-10-19T08:00:00.000Z';
export const REVOKED_AT = '2026-10-19T09:30:00.000Z';
export const SECRET = 'okfc_7k2m9q4w8x1z5v3n6b0p';
export const LOCAL_OWNER_REFUSAL =
  'Connector credentials are issued, listed and revoked only by the local owner';

export function connectorOf(id: string, label: string, extra: Partial<Connector> = {}): Connector {
  return {
    connector_id: id,
    label,
    workspace_ids: [WS],
    permissions: ['read'],
    created_at: ISSUED_AT,
    ...extra,
  };
}

/** A connector that reads this workspace only. */
export const desktop = connectorOf('77777777-7777-4777-8777-777777777777', 'Desktop app');
/** One that reads this workspace and another, and may propose. */
export const editor = connectorOf('66666666-6666-4666-8666-666666666666', 'Code editor', {
  workspace_ids: [OTHER_WS, WS],
  permissions: ['read', 'propose'],
});
/** One that reads another workspace only: not listed on this one's page. */
export const elsewhere = connectorOf('55555555-5555-4555-8555-555555555555', 'Notes tool', {
  workspace_ids: [OTHER_WS],
});
/** A revoked one of this workspace. */
export const retired = connectorOf('44444444-4444-4444-8444-444444444444', 'Old laptop', {
  revoked_at: REVOKED_AT,
});

/** One scripted answer: `usual` is what the service answers when nothing goes wrong. */
export type Step = (usual: () => Reply) => Reply | Promise<Reply>;

export interface ConnectorService {
  connectors: Connector[];
  /** Every `create_connector` request, in order. */
  issues: Json[];
  /** Every `revoke_connector` request, in order. */
  revocations: Json[];
  /** Scripted answers of `create_connector`, used one per request. */
  issueSteps: Step[];
  /** Scripted answers of `revoke_connector`, used one per request. */
  revokeSteps: Step[];
  handlers: Record<string, (body: Json) => Reply | Promise<Reply>>;
}

/** The service, holding `connectors` at first; a new connector gets `SECRET`. */
export function connectorService(
  paths: { create: string; list: string; revoke: string },
  connectors: Connector[] = [desktop, editor, elsewhere, retired],
): ConnectorService {
  const byKey = new Map<string, Connector>();
  const service: ConnectorService = {
    connectors: [...connectors],
    issues: [],
    revocations: [],
    issueSteps: [],
    revokeSteps: [],
    handlers: {},
  };
  service.handlers = {
    [paths.list]: (body) =>
      ok({
        connectors: service.connectors.filter(
          (connector) => body.include_revoked === true || !connector.revoked_at,
        ),
      }),
    [paths.create]: (body) => {
      service.issues.push(body);
      const key = String(body.idempotency_key);
      const usual = (): Reply => {
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
        const made = connectorOf(
          `${'abcdef'.charAt(byKey.size).repeat(8)}-3333-4333-8333-333333333333`,
          String(body.label),
          {
            workspace_ids: (body.workspace_ids as string[] | undefined) ?? [],
            permissions: body.allow_propose === true ? ['read', 'propose'] : ['read'],
          },
        );
        byKey.set(key, made);
        service.connectors.push(made);
        return ok({ connector: made, secret: SECRET });
      };
      const step = service.issueSteps.shift();
      return step === undefined ? usual() : step(usual);
    },
    [paths.revoke]: (body) => {
      service.revocations.push(body);
      const usual = (): Reply => {
        const index = service.connectors.findIndex(
          (connector) => connector.connector_id === body.connector_id,
        );
        const found = service.connectors[index];
        if (found === undefined) return fail(404, 'not_found', 'No such connector');
        const revoked = { ...found, revoked_at: found.revoked_at ?? REVOKED_AT };
        service.connectors.splice(index, 1, revoked);
        return ok(revoked);
      };
      const step = service.revokeSteps.shift();
      return step === undefined ? usual() : step(usual);
    },
  };
  return service;
}
