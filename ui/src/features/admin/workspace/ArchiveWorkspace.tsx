/**
 * Archive and Unarchive (`archive_workspace`, `unarchive_workspace`, ruling C9): one button, which
 * archives a workspace that is not archived and unarchives one that is. Both need `admin`, and the
 * service runs both in an archived workspace (`ALLOWED_WHILE_ARCHIVED`). SPEC section 8: "Archive
 * is reversible and keeps history."
 *
 * Archive is made on the head the page shows (`base_revision`); a conflict is said in words with a
 * way to reload the workspace, or, when the head had not moved, why the service holds it back (an
 * import, a conversion or a restore of it waiting or running: the contract's rule). Unarchive
 * carries no base. Once the service has answered, the shell reads the workspace again
 * (`WorkspaceChanges.refresh`): the bar, every action of the workspace and this button then show
 * it as the service has it, archived or not. The button is one element whatever it says, and keeps
 * keyboard focus (`aria-disabled` while the request is out and the workspace is read again). A
 * polite status says what was done.
 */
import { useId, useRef, useState } from 'react';
import { formatInstant } from '../../../lib/instant';
import type { HeadChange } from '../../explorer/head';
import { allowedOperation, archivedWords, Missing } from '../../explorer/rights';
import { NOT_ADOPTED, useChange } from '../../tree/ChangeForms';
import { useWorkspaceChanges } from '../../tree/context';
import { LifecycleFailure } from './LifecycleFailure';
import { type Archiving, setArchived } from './lifecycle';

const ARCHIVED =
  'Archived. Nothing in it can be changed until it is unarchived; it can still be read, exported and backed up.';
const UNARCHIVED = 'Unarchived. It can be changed again.';
const NOT_READ =
  'This page could not read the workspace again: the line at the top of the page says why.';
const HELD_BACK =
  'A workspace is not archived while an import, a conversion or a restore of it is waiting or running. Wait until those jobs have finished (the Jobs page lists them), then press the button again.';

export function ArchiveWorkspace() {
  const { rights, refresh } = useWorkspaceChanges();
  const headingId = useId();
  const aboutId = useId();
  const archivedId = useId();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [said, setSaid] = useState('');
  // Whether the last press archived: the failure is said as what was asked.
  const [archiving, setArchiving] = useState(true);
  // The workspace is being read again after an answer: the button waits for what it then shows.
  const [reading, setReading] = useState(false);
  const change = useChange<Archiving, HeadChange>(
    setArchived,
    (_result, values) => {
      const done = values.archive ? ARCHIVED : UNARCHIVED;
      setSaid(done);
      setReading(true);
      void refresh().then((shown) => {
        setReading(false);
        if (shown === null) setSaid(`${done} ${NOT_READ}`);
      });
    },
    buttonRef,
  );
  const archivedAt = rights.archivedAt;
  const archived = archivedAt !== null;
  const can = allowedOperation(
    rights,
    archived ? 'unarchive_workspace' : 'archive_workspace',
    'admin',
  );
  if (can === 'no-permission') {
    return (
      <section aria-labelledby={headingId}>
        <h4 id={headingId}>Archive</h4>
        <Missing
          action={archived ? 'Unarchiving this workspace' : 'Archiving this workspace'}
          permission="admin"
        />
      </section>
    );
  }
  const busy = change.pending || reading;
  const stopped = can === 'archived';
  const when = archivedAt === null ? null : (formatInstant(archivedAt) ?? archivedAt);
  const label = change.pending
    ? archiving
      ? 'Archiving…'
      : 'Unarchiving…'
    : archived
      ? 'Unarchive this workspace'
      : 'Archive this workspace';
  const status = change.pending
    ? label
    : [said, change.said === NOT_ADOPTED ? '' : change.said]
        .filter((part) => part !== '')
        .join(' ');
  return (
    <section aria-labelledby={headingId}>
      <h4 id={headingId}>Archive</h4>
      <p id={aboutId} className="meta">
        {when === null
          ? 'Archiving is reversible and keeps the history. An archived workspace can still be read, exported and backed up; nothing in it can be changed until it is unarchived.'
          : `This workspace was archived on ${when}. Unarchiving returns it to the ordinary listings, where it can be changed again; its history and records were kept.`}
      </p>
      <p>
        {/* One element whatever it says: it keeps keyboard focus when the workspace changes. */}
        <button
          type="button"
          ref={buttonRef}
          aria-disabled={busy || stopped}
          aria-describedby={stopped ? `${archivedId} ${aboutId}` : aboutId}
          onClick={() => {
            if (busy || stopped) return;
            setSaid('');
            setArchiving(!archived);
            change.start({ archive: !archived });
          }}
        >
          {label}
        </button>
      </p>
      {stopped && archivedAt !== null && (
        <p id={archivedId} className="meta">
          {archivedWords(archivedAt)}
        </p>
      )}
      <LifecycleFailure
        context={archiving ? 'Archiving this workspace' : 'Unarchiving this workspace'}
        change={change}
        sameWords={archiving ? HELD_BACK : 'The service refused it as a conflict.'}
        sameReloads={!archiving}
      />
      {/* Present from the start, so what pressing did is announced. */}
      <p role="status" className="meta">
        {status}
      </p>
    </section>
  );
}
