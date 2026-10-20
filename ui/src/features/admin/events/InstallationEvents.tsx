/**
 * The installation's security events (ruling C8), on its Administration page:
 * `list_tenant_events` read as `EventLog` reads a log.
 *
 * The installation's log holds the sign-ins and the refused ones, the connectors issued and
 * revoked, and the requests refused outside any workspace. Reading it takes the installation's
 * (tenant) `admin`, which the contract gives the page no way to know beforehand: the service
 * decides, and its refusal is said in words with what it still allows (each workspace's refused
 * requests, on that workspace's Administration page), without a retry, since asking again would be
 * refused the same way.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useId, useRef } from 'react';
import type { FailureMessage } from '../../errors/failure';
import { EventLog } from './EventLog';
import { installationEventsOptions } from './events';

const REFUSED: FailureMessage = {
  title: 'Only an administrator of this installation may read its security events.',
  advice:
    'Each workspace’s refused requests are listed on that workspace’s Administration page, for its administrators.',
  action: { kind: 'none' },
};

export function InstallationEvents() {
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const events = useInfiniteQuery(installationEventsOptions());
  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId} ref={headingRef} tabIndex={-1}>
        Security events of the installation
      </h2>
      <p>
        Who signed in and which sign-ins were refused, the connectors issued and revoked, and the
        requests refused outside any workspace. Each workspace’s refused requests are listed on its
        own Administration page.
      </p>
      <EventLog
        events={events}
        headingId={headingId}
        headingRef={headingRef}
        request="installation"
        context="Listing the installation’s security events"
        elsewhere="The installation’s jobs are listed under Installation jobs."
        refusal={REFUSED}
      />
    </section>
  );
}
