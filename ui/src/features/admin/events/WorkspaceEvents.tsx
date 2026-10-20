/**
 * A workspace's security events (ruling C8), on its Administration page: `list_events` read as
 * `EventLog` reads a log.
 *
 * In a workspace's log the security events are the requests to it that the service refused
 * (`permission_denied`); sign-ins and connectors are the installation's, listed on its own
 * Administration page, which the section names. The service returns a workspace's security events
 * only to a caller holding `admin` there, checked on every request: were the person's rights taken
 * away meanwhile, the pages hold none, and the section says that is why it may list none. The
 * operation runs in an archived workspace as in any other (`ALLOWED_WHILE_ARCHIVED`).
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import { EventLog } from './EventLog';
import { workspaceEventsOptions } from './events';

export function WorkspaceEvents({ workspaceId }: { workspaceId: string }) {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const events = useInfiniteQuery(workspaceEventsOptions(workspaceId));
  return (
    <section aria-labelledby={headingId}>
      <h3 id={headingId} ref={headingRef} tabIndex={-1}>
        Security events
      </h3>
      <p>
        The requests to this workspace that the service refused, with who asked and what for.
        Sign-ins and the connectors issued or revoked belong to the installation: they are listed
        under <Link to="/admin">Administration of this installation</Link>. The service lists these
        events only to an administrator of this workspace, checking on every request.
      </p>
      <EventLog
        events={events}
        headingId={headingId}
        headingRef={headingRef}
        request={workspaceId}
        context="Listing this workspace’s security events"
        elsewhere="Content changes are shown in History, and jobs on the Jobs page."
        workspaceId={workspaceId}
      />
    </section>
  );
}
