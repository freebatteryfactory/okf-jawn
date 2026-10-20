/**
 * The connectors that read this workspace (ruling C5): `list_connectors` answers the
 * installation's, and those whose `workspace_ids` include this one are listed, each with its name,
 * what it may do, the other workspaces it reads, when it was issued and, once revoked, when.
 * Revoked ones are listed only when the person asks ("Show revoked connectors too", the request's
 * `include_revoked`).
 *
 * Revoking is destructive (ruling C3): it goes through the alert dialog of `useDestructive`, bound
 * to the connector it was opened for, as it was shown then, and its idempotency key is kept
 * through a retry. `revoke_connector` takes no `base_revision`: the connector is the whole of what
 * the dialog is bound to. Once the service has revoked it, a polite status says so and the list is
 * asked again; the button that opened the dialog goes with the revocation (a revoked connector
 * is not offered again), so keyboard focus goes to the section's heading instead.
 *
 * In an archived workspace the button stays and says why it cannot run, and sends nothing: the
 * contract does not run `revoke_connector` while a workspace is archived (it is not on
 * `ALLOWED_WHILE_ARCHIVED`), as `allowedOperation` reads it. A failure to list is said with its
 * retry; a hosted refusal is the caller's to say (`WorkspaceConnectors`).
 */
import type { UseQueryResult } from '@tanstack/react-query';
import { useQueryClient } from '@tanstack/react-query';
import { type RefObject, useId, useRef, useState } from 'react';
import type {
  AccessRoute,
  Connector,
  ListConnectorsResponse,
} from '../../../api/generated/types.gen';
import { formatInstant, Instant } from '../../../lib/instant';
import { FailureNotice } from '../../errors/FailureNotice';
import type { FailureMessage } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import { allowedOperation, archivedWords, type Rights } from '../../explorer/rights';
import { DestructiveDialog, useDestructive } from '../destructive';
import {
  isHostedRefusal,
  listedAgain,
  nameOf,
  othersWords,
  type RevokeValues,
  revokeOne,
  rightsWords,
} from './connectors';

export interface ConnectorListProps {
  workspaceId: string;
  rights: Rights;
  list: UseQueryResult<ListConnectorsResponse>;
  includeRevoked: boolean;
  onIncludeRevoked: (include: boolean) => void;
  /** The section's heading: focus goes there when what had it is gone. */
  headingRef: RefObject<HTMLHeadingElement | null>;
  /** The route of the person's session, once known. */
  route: AccessRoute | undefined;
}

/** The hosted refusal of a revocation, in the dialog: the rule, with no retry. */
const HOSTED: FailureMessage = {
  title: 'Connectors are issued only on a local installation.',
  advice: 'Hosted agents connect through WorkOS Connect.',
  action: { kind: 'none' },
};

export function ConnectorList({
  workspaceId,
  rights,
  list,
  includeRevoked,
  onIncludeRevoked,
  headingRef,
  route,
}: ConnectorListProps) {
  const client = useQueryClient();
  const archivedId = useId();
  const revokedBoxRef = useRef<HTMLInputElement | null>(null);
  const opener = useRef<HTMLElement | null>(null);
  const [said, setSaid] = useState('');
  const revoke = useDestructive<RevokeValues, Connector>({
    act: revokeOne,
    onDone: (_revoked, values) => {
      // The button that opened the dialog goes with the revocation.
      opener.current = headingRef.current;
      setSaid(`“${values.name}” was revoked: its MCP client is refused from now on.`);
      void listedAgain(client);
    },
    returnFocus: opener,
  });
  const failure = useRetry({
    request: String(includeRevoked),
    // The hosted refusal is not a failure to retry: the caller says it in place of the list.
    failed: list.isError && !isHostedRefusal(list.error, route),
    error: list.error,
    run: () => list.refetch(),
    loaded: () => revokedBoxRef.current,
  });
  const can = allowedOperation(rights, 'revoke_connector', 'admin');
  const listed = (list.data?.connectors ?? []).filter((connector) =>
    connector.workspace_ids.includes(workspaceId),
  );
  const opened = revoke.opened;

  return (
    <>
      <p className="m-0">
        <label className="flex items-center gap-2">
          <input
            ref={revokedBoxRef}
            type="checkbox"
            checked={includeRevoked}
            onChange={(event) => onIncludeRevoked(event.target.checked)}
          />
          Show revoked connectors too
        </label>
      </p>
      {list.isPending && !failure.retrying && <p role="status">Listing the connectors…</p>}
      {failure.shown && (
        <FailureNotice
          context="Listing the connectors"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {list.data !== undefined && listed.length === 0 && (
        <p>
          {includeRevoked
            ? 'No connector, revoked or not, reads this workspace.'
            : 'No connector reads this workspace.'}
        </p>
      )}
      {can === 'archived' && rights.archivedAt !== null && listed.length > 0 && (
        <p id={archivedId} className="meta">
          {archivedWords(rights.archivedAt)}
        </p>
      )}
      {listed.length > 0 && (
        <ul className="job-list" aria-label="Connectors that read this workspace">
          {listed.map((connector) => (
            <ConnectorRow
              key={connector.connector_id}
              connector={connector}
              workspaceId={workspaceId}
              offered={can !== 'no-permission'}
              stoppedBy={can === 'archived' ? archivedId : null}
              onRevoke={(button, values) => {
                opener.current = button;
                revoke.open(values);
              }}
            />
          ))}
        </ul>
      )}
      {/* Present from the start, so a revocation is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
      <DestructiveDialog
        action={revoke}
        title={`Revoke the connector “${opened?.name ?? ''}”?`}
        description={
          <p>
            The MCP client holding its secret is refused from the moment the service answers, in
            this workspace and in every other one it reads. This cannot be undone: to connect that
            client again, issue a new connector and put its new secret in the client.
          </p>
        }
        confirmLabel="Revoke it"
        busyLabel="Revoking…"
        context="Revoking the connector"
        workspaceId={workspaceId}
        {...(revoke.failure.shown && isHostedRefusal(revoke.failure.error, route)
          ? { words: HOSTED }
          : {})}
      >
        {opened !== null && (
          <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-0.5 [&_dd]:m-0">
            <dt>Name</dt>
            <dd>{opened.name}</dd>
            <dt>Connector</dt>
            <dd>
              <code>{opened.connectorId}</code>
            </dd>
            <dt>It may</dt>
            <dd>{opened.rights}</dd>
            <dt>Workspaces</dt>
            <dd>
              {opened.others === 0
                ? 'This one only.'
                : `This one and ${opened.others} other${opened.others === 1 ? '' : 's'}.`}
            </dd>
            <dt>Issued</dt>
            <dd>{formatInstant(opened.createdAt) ?? opened.createdAt}</dd>
          </dl>
        )}
      </DestructiveDialog>
    </>
  );
}

interface ConnectorRowProps {
  connector: Connector;
  workspaceId: string;
  /** Revoking is offered to this person. */
  offered: boolean;
  /** The id of the words saying why revoking cannot run now; null when it can. */
  stoppedBy: string | null;
  onRevoke: (button: HTMLElement, values: RevokeValues) => void;
}

function ConnectorRow({ connector, workspaceId, offered, stoppedBy, onRevoke }: ConnectorRowProps) {
  const titleId = useId();
  const name = nameOf(connector);
  const rights = rightsWords(connector.permissions);
  const others = othersWords(connector, workspaceId);
  const revokedAt = connector.revoked_at;
  return (
    <li className="job" aria-labelledby={titleId} data-connector={connector.connector_id}>
      <p id={titleId} className="job-title">
        {name}
      </p>
      <p>{rights}</p>
      {others !== null && <p>{others}</p>}
      <p className="meta">
        Issued <Instant value={connector.created_at} />, connector{' '}
        {connector.connector_id.slice(0, 8)}.
      </p>
      {revokedAt !== null && revokedAt !== undefined ? (
        <p>
          Revoked on <Instant value={revokedAt} />: its MCP client is refused.
        </p>
      ) : (
        offered && (
          <p>
            <button
              type="button"
              aria-disabled={stoppedBy !== null}
              aria-describedby={stoppedBy ?? undefined}
              onClick={(event) => {
                if (stoppedBy !== null) return;
                onRevoke(event.currentTarget, {
                  connectorId: connector.connector_id,
                  name,
                  rights,
                  others: connector.workspace_ids.filter((id) => id !== workspaceId).length,
                  createdAt: connector.created_at,
                });
              }}
            >
              Revoke “{name}”…
            </button>
          </p>
        )
      )}
    </li>
  );
}
