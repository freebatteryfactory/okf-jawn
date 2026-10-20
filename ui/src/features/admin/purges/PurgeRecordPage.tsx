/**
 * A purge's record (UI M4c, ruling C4), at `/purges/<id>`: the installation's page, not a
 * workspace's, so it stays readable after the workspace the purge removed is gone. It asks with
 * `get_purge` alone, by the purge's identity (a tenant operation: no workspace is opened or named).
 *
 * It says the purge's state in words (`requested`, `running`, `completed`, `failed`), what it
 * removes (identities only, as the record holds them), who asked for it and when, when it
 * completed, its last failure in words with the service's own, and, once it has completed, every
 * count of its report (`PurgeReport`) in words, the large ones grouped. A record holds no content
 * of what was removed, and the page says that copies outside the application are not erased.
 *
 * While the purge has not finished it is asked again on a clock (`purgeOptions`), and a polite
 * status says how it stands each time that changes. The address's identity is checked with the
 * generated schema first: an address that names no purge says so and asks nothing. A failure is
 * shown with its retry (`useRetry`); a refusal says, in this page's words, who may read a record.
 *
 * Like every page outside a workspace it has a level-one heading that takes focus from script
 * (`lib/focus.ts`): where focus goes after the purge's dialog has left with the workspace's page,
 * and when a notice leaves with nothing nearer to take it.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import type { Purge, PurgeTarget } from '../../../api/generated/types.gen';
import { zPurgeId } from '../../../api/generated/zod.gen';
import { Instant } from '../../../lib/instant';
import { FailureNotice } from '../../errors/FailureNotice';
import { classifyFailure, messageFor } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import { OUTSIDE_WORDS } from './PurgeWords';
import {
  PURGE_POLL_MS,
  purgeMayChange,
  purgeOptions,
  purgeStateWords,
  recordRefusedWords,
  reportWords,
} from './purges';

export function PurgeRecordPage({ purgeId }: { purgeId: string }) {
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const valid = zPurgeId.safeParse(purgeId).success;
  const purge = useQuery({ ...purgeOptions(purgeId), enabled: valid });
  const reading = useRetry({
    request: purgeId,
    failed: purge.isError,
    error: purge.error,
    run: () => purge.refetch(),
    loaded: () => headingRef.current,
  });
  const words = reading.shown ? recordRefusedWords(reading.error) : undefined;
  return (
    <section className="home" aria-labelledby="purge-heading">
      <h1 id="purge-heading" ref={headingRef} tabIndex={-1}>
        Purge {purgeId.slice(0, 8)}
      </h1>
      <p>
        <Link to="/">Your workspaces</Link> ·{' '}
        <Link to="/admin">Administration of this installation</Link>
      </p>
      {!valid ? (
        <div role="alert" className="failure-notice">
          <p>
            <strong>This link is not valid.</strong>
          </p>
          <p>The address does not name a purge. Nothing was requested.</p>
        </div>
      ) : (
        <>
          {reading.shown && (
            <FailureNotice
              context="Reading this purge"
              error={reading.error}
              retry={reading}
              {...(words === undefined ? {} : { words })}
            />
          )}
          {purge.data === undefined ? (
            purge.isPending && !reading.retrying && <p role="status">Reading the purge…</p>
          ) : (
            <PurgeRecord purge={purge.data} />
          )}
        </>
      )}
    </section>
  );
}

/** What a purge removes, by identity. */
function TargetWords({ target }: { target: PurgeTarget }) {
  return target.kind === 'item' ? (
    <>
      the item <code>{target.item_id}</code> of the workspace <code>{target.workspace_id}</code>
    </>
  ) : (
    <>
      the workspace <code>{target.workspace_id}</code>
    </>
  );
}

function PurgeRecord({ purge }: { purge: Purge }) {
  const reportId = useId();
  const report = purge.report ?? null;
  const failure = purge.error === null || purge.error === undefined ? null : purge.error;
  const failed = failure === null ? null : messageFor(classifyFailure(failure));
  return (
    <>
      {/* Says the state each time the record is answered in another one. */}
      <p role="status">
        <strong>{purgeStateWords(purge.state)}</strong>
      </p>
      {purgeMayChange(purge) && (
        <p className="meta">
          This page asks again every {PURGE_POLL_MS / 1000} seconds until the purge has finished.
        </p>
      )}
      <p>
        What it removes: <TargetWords target={purge.target} />.
      </p>
      <p>
        Asked for by {purge.requested_by} on <Instant value={purge.requested_at} />.
      </p>
      {purge.completed_at !== null && purge.completed_at !== undefined && (
        <p>
          Completed on <Instant value={purge.completed_at} />.
        </p>
      )}
      <p className="meta">Its work is the installation’s job {purge.job_id.slice(0, 8)}.</p>
      {failure !== null && failed !== null && (
        <div className="notice">
          <p>
            <strong>Its last failure: {failed.title}</strong>
          </p>
          {failure.message.length > 0 && (
            <p>
              The service said: <q>{failure.message}</q>
            </p>
          )}
          {failure.request_id ? (
            <p>
              Reference: <code>{failure.request_id}</code>
            </p>
          ) : null}
        </div>
      )}
      {report === null ? (
        <p className="meta">Its counts are shown once it has completed.</p>
      ) : (
        <>
          <h2 id={reportId} className="mt-3 font-bold">
            What it removed
          </h2>
          <ul aria-labelledby={reportId} className="list-disc pl-6">
            {reportWords(report).map((sentence) => (
              <li key={sentence}>{sentence}</li>
            ))}
          </ul>
        </>
      )}
      <p className="meta">
        {OUTSIDE_WORDS} This record holds identities and counts, never content, and stays readable
        after what it removed is gone.
      </p>
    </>
  );
}
