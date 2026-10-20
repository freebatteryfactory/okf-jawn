/**
 * Backup of the installation (ruling C6): "Back up the installation" asks `backup_installation`,
 * which answers 202 with the job it queued, a job of the installation. The archive holds the
 * installation's identities, tenant grants, connector records without their secrets, purge records
 * and tenant events, and no workspace content (SPEC section 12); each workspace is backed up from
 * its own Administration page.
 *
 * Who may. The service takes it only from an administrator of the installation in their own
 * browser session (`HUMAN_SESSION_ONLY`); the page says so in words (`humanSessionWords`), offers
 * the button to whoever reaches the page (no tenant grant can be read), and says the service's
 * `forbidden` in words (`installation-words.ts`).
 *
 * The answer is a job, said as one (`acceptedWords`), in the state the service reported when it
 * answered. The list of the installation's jobs below is asked again at once
 * (`rememberTenantJob`), so the job is shown there as it stands now, followed while it runs; its
 * row offers the archive once it has finished, by the artifact's own rule
 * (`TenantArtifactDownload`). Idempotency as for an export (`useExportStart`): a request is
 * repeated under its key until a job answers it, so a request whose answer was lost does not start
 * a second backup; the next press after an answer is a new backup.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useId, useRef } from 'react';
import { backupInstallation } from '../../../api/generated/sdk.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import { humanSessionWords } from '../../explorer/rights';
import { acceptedWords, useExportStart } from '../../exports/ExportStart';
import { installationBackupWords } from './installation-words';
import { rememberTenantJob } from './tenant-jobs';

export function InstallationBackups() {
  const headingId = useId();
  const aboutId = useId();
  const client = useQueryClient();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const start = useExportStart<null>({
    // One thing is ever asked here.
    request: 'backup_installation',
    start: async (attempt) => {
      const { data: job } = await backupInstallation({
        body: { idempotency_key: attempt.key },
        throwOnError: true,
      });
      rememberTenantJob(client);
      return job;
    },
    loaded: () => buttonRef.current,
  });
  const said =
    start.pending && !start.failure.retrying
      ? 'Starting the backup…'
      : start.started === null
        ? ''
        : `${acceptedWords('The backup of the installation', start.started)} Its row under “Installation jobs” shows how it stands now, and offers the download once it has finished.`;
  return (
    <section aria-labelledby={headingId} className="mt-8 grid gap-2">
      <h2 id={headingId} className="page-heading">
        Backup of the installation
      </h2>
      <p id={aboutId}>
        A backup of the installation holds its identities, tenant grants, connector records without
        their secrets, purge records and security events: no workspace content. Each workspace is
        backed up from its own Administration page. Whoever holds the file can read all of it.
      </p>
      <p className="meta">{humanSessionWords('Backing up the installation')}</p>
      <p>
        {/* aria-disabled rather than disabled: the button keeps focus while the backup starts. */}
        <button
          type="button"
          ref={buttonRef}
          aria-disabled={start.pending}
          aria-describedby={aboutId}
          onClick={() => start.begin(null)}
        >
          {start.pending ? 'Starting the backup…' : 'Back up the installation'}
        </button>
      </p>
      {/* Present from the start, so what pressing did is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
      {start.failure.shown && (
        <FailureNotice
          context="Backing up the installation"
          error={start.failure.error}
          retry={start.failure}
          words={installationBackupWords(start.failure.error)}
        />
      )}
    </section>
  );
}
