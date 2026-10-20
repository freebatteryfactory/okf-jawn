/**
 * "Purge this workspace…" (rulings C3 and C4), the last part of the workspace's lifecycle on its
 * Administration page.
 *
 * It asks through the destructive dialog (`admin/destructive.tsx`): the dialog says in SPEC
 * section 8's words what a purge removes and what it cannot erase (`PurgeWords`), names exactly
 * the workspace and the revision it was opened at, and is bound to that revision whatever the
 * shell shows after. No further challenge is asked. The human-session rule is said beside the
 * button and in the dialog (`humanSessionWords`); the service applies it, and its refusal is said
 * in words, with what it allows (`purgeRefusedWords`). The purge is offered by the person's grant
 * here and stays offered in an archived workspace, where the service runs it
 * (`ALLOWED_WHILE_ARCHIVED`); were the archive to stop it, the button would stay, say why, and
 * open nothing.
 *
 * The service answers 202 with the purge's record: it goes into the cache (`rememberPurge`) and
 * the page goes to that record (`/purges/<id>`), which is the installation's and stays readable
 * after the workspace is gone. Keyboard focus then rests on the record's heading (the router's
 * focus rule: the button that opened the dialog leaves with the workspace's page).
 */
import { useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useId, useRef } from 'react';
import type { Purge } from '../../../api/generated/types.gen';
import { short } from '../../documents/describe';
import { allowedOperation, archivedWords, humanSessionWords, Missing } from '../../explorer/rights';
import { useWorkspaceChanges } from '../../tree/context';
import { DestructiveDialog, useDestructive } from '../destructive';
import { PurgeWords } from './PurgeWords';
import {
  purgeRefusedWords,
  rememberPurge,
  startWorkspacePurge,
  type WorkspacePurge,
} from './purges';

const RULE = humanSessionWords('Purging a workspace');

export function PurgeWorkspace({ name }: { name: string }) {
  const { workspaceId, head, rights } = useWorkspaceChanges();
  const client = useQueryClient();
  const navigate = useNavigate();
  const headingId = useId();
  const aboutId = useId();
  const ruleId = useId();
  const archivedId = useId();
  const opener = useRef<HTMLButtonElement | null>(null);
  const purge = useDestructive<WorkspacePurge, Purge>({
    act: startWorkspacePurge,
    returnFocus: opener,
    onDone: (answered) => {
      rememberPurge(client, answered);
      void navigate({ to: '/purges/$purgeId', params: { purgeId: answered.id } });
    },
  });
  const can = allowedOperation(rights, 'purge_workspace', 'admin');
  const bound = purge.opened;
  const words = purge.failure.shown ? purgeRefusedWords(purge.failure.error) : undefined;
  return (
    <section aria-labelledby={headingId}>
      <h4 id={headingId} className="mt-3 font-bold">
        Purge
      </h4>
      {can === 'no-permission' ? (
        <Missing action="Purging this workspace" permission="admin" />
      ) : (
        <>
          <p id={aboutId} className="meta">
            A purge removes this workspace and its history for good, with the backups and exports
            the application keeps of it, and leaves a record of what it removed. Archiving keeps
            them, and can be undone.
          </p>
          <p id={ruleId} className="meta">
            {RULE}
          </p>
          <p>
            <button
              type="button"
              ref={opener}
              aria-disabled={can === 'archived'}
              aria-describedby={
                can === 'archived' ? `${archivedId} ${aboutId} ${ruleId}` : `${aboutId} ${ruleId}`
              }
              onClick={() => {
                if (can === 'yes') purge.open({ workspaceId, baseRevision: head });
              }}
            >
              Purge this workspace…
            </button>
          </p>
          {can === 'archived' && rights.archivedAt !== null && (
            <p id={archivedId} className="meta">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          <DestructiveDialog
            action={purge}
            title="Purge this workspace?"
            description={<PurgeWords of="workspace" />}
            confirmLabel="Purge the workspace"
            busyLabel="Purging…"
            context="Purging this workspace"
            workspaceId={workspaceId}
            {...(words === undefined ? {} : { words })}
          >
            {bound !== null && (
              <>
                <p>
                  The workspace “{name}” (<code>{bound.workspaceId}</code>), as of revision{' '}
                  <code title={bound.baseRevision}>{short(bound.baseRevision)}</code>: the version
                  this page showed when you asked. If it has changed since, nothing is removed and
                  the service says so.
                </p>
                <p>{RULE}</p>
              </>
            )}
          </DestructiveDialog>
        </>
      )}
    </section>
  );
}
