/**
 * Connectors (ruling C5), on a workspace's Administration page: issue one for this workspace,
 * list those that read it, revoke one. A connector is the credential of a local MCP client, read
 * by default, proposing only when allowed, never verifying or approving (SPEC section 11).
 *
 * The contract's connector operations are the installation's and its local owner's only
 * (`LOCAL_OWNER_ONLY`). The section states that rule up front, with whether the session
 * (`get_session`'s `principal.route`) is the local owner's, but does not decide by it: the list is
 * asked all the same, and the service decides. When the service refuses because the person is not
 * the local owner (`isHostedRefusal`), the section reads the hosted words alone
 * (`LOCAL_OWNER_WORDS`): what a hosted installation does instead, not an error to retry.
 *
 * Issuing (`IssueConnector`) shows the secret once and keeps it nowhere else; the list
 * (`ConnectorList`) offers revoking through the destructive dialog.
 */
import { useQuery } from '@tanstack/react-query';
import { useId, useRef, useState } from 'react';
import { LOCAL_OWNER_WORDS } from '../../explorer/rights';
import { sessionOptions } from '../../imports/limits';
import { useWorkspaceChanges } from '../../tree/context';
import { ConnectorList } from './ConnectorList';
import { connectorsOptions, isHostedRefusal } from './connectors';
import { IssueConnector } from './IssueConnector';

export function WorkspaceConnectors({ workspaceId }: { workspaceId: string }) {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const { rights } = useWorkspaceChanges();
  const session = useQuery(sessionOptions());
  const route = session.data?.principal.route;
  const [includeRevoked, setIncludeRevoked] = useState(false);
  const list = useQuery(connectorsOptions(includeRevoked));
  const heading = (
    <h3 id={headingId} ref={headingRef} tabIndex={-1}>
      Connectors
    </h3>
  );

  if (list.isError && isHostedRefusal(list.error, route)) {
    return (
      <section aria-labelledby={headingId}>
        {heading}
        <p>{LOCAL_OWNER_WORDS}</p>
      </section>
    );
  }
  return (
    <section aria-labelledby={headingId}>
      {heading}
      <p>
        A connector lets an MCP client on this computer, such as a desktop AI app or an editor, read
        this workspace with a secret of its own; it never uses your browser session.
      </p>
      <p>
        Only the installation’s local owner, in their own browser on this computer, issues, lists
        and revokes connectors.
      </p>
      {route === 'local_owner' && <p className="meta">You are the local owner here.</p>}
      {route !== undefined && route !== 'local_owner' && (
        <p className="meta">You are not signed in as this installation’s local owner.</p>
      )}
      <IssueConnector workspaceId={workspaceId} rights={rights} route={route} />
      <ConnectorList
        workspaceId={workspaceId}
        rights={rights}
        list={list}
        includeRevoked={includeRevoked}
        onIncludeRevoked={setIncludeRevoked}
        headingRef={headingRef}
        route={route}
      />
    </section>
  );
}
