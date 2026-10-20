/**
 * The failure of a change to the workspace's own record (its name and description, Archive,
 * Unarchive), in its part of the lifecycle section.
 *
 * A `conflict` is said in words with a way to reload the workspace: nothing was changed, the
 * workspace changed since the page showed it (with the revision it now holds, when the refusal
 * names it), and "Reload the workspace" reads it again as the shell's Refresh does (`useChange`'s
 * `refresh`): its name, description, archive and head as they are now. The person then presses
 * the button again, on what they see. A conflict that is not about the head (`same`: the head
 * had not moved) is said in the part's own words (`sameWords`), with the reload only where it
 * helps. A workspace that is archived is not a conflict to reload: it is said as any failure is.
 *
 * Any other failure is a `FailureNotice` with its retry (`useRetry`), which repeats the request
 * under the key it had. A refusal at a field the form draws (`drawn`) is shown at that field.
 * Keyboard focus in the notice, when it leaves, goes to the part's button (`useChange`'s
 * `submit`).
 */

import { short } from '../../documents/describe';
import { FailureNotice } from '../../errors/FailureNotice';
import { classifyFailure } from '../../errors/failure';
import type { Change } from '../../tree/ChangeForms';
import { useWorkspaceChanges } from '../../tree/context';

const NO_FIELDS: ReadonlySet<string> = new Set();

/** Why a conflict was refused, by what the head check found (`Change.conflict`). */
const CONFLICT_WORDS = {
  checking:
    'The service refused it as a conflict. Checking whether the workspace has a newer version…',
  moved: 'The workspace changed since this page showed it.',
  unknown:
    'The service refused it as a conflict, and it could not be checked whether the workspace has a newer version.',
} as const;

export interface LifecycleFailureProps<T> {
  /** What the person was doing: "Archiving this workspace". */
  context: string;
  change: Change<T>;
  /** Why a conflict was refused when the head had not moved. */
  sameWords: string;
  /** Whether reloading the workspace helps with that conflict. */
  sameReloads: boolean;
  /** The fields, as the service names them, whose refusal the form shows at the field itself. */
  drawn?: ReadonlySet<string>;
}

export function LifecycleFailure<T>({
  context,
  change,
  sameWords,
  sameReloads,
  drawn = NO_FIELDS,
}: LifecycleFailureProps<T>) {
  const { workspaceId } = useWorkspaceChanges();
  const { failure, conflict, refused, staleHead } = change;
  if (!failure.shown || (refused !== null && drawn.has(refused.field))) return null;
  if (conflict === null) {
    return (
      <FailureNotice
        context={context}
        error={failure.error}
        retry={failure}
        workspaceId={workspaceId}
      />
    );
  }
  const refusal = classifyFailure(failure.error);
  const said = refusal.kind === 'api' ? refusal.error.message : '';
  const why =
    staleHead !== null
      ? `The workspace changed since this page showed it: it now holds revision ${short(staleHead)}.`
      : conflict === 'same'
        ? sameWords
        : CONFLICT_WORDS[conflict];
  const reloads = staleHead !== null || conflict !== 'same' || sameReloads;
  return (
    // Followed by the retry hook like any failure notice; its button reloads, it repeats nothing.
    <div ref={failure.noticeRef} tabIndex={-1} className="failure-notice">
      <div role="alert">
        <p>
          <strong>{context}: nothing was changed.</strong>
        </p>
        <p>
          {why}
          {reloads ? ' Reload it to see it as it is now, then press the button again.' : ''}
        </p>
        {said.length > 0 && (
          <p>
            The service said: <q>{said}</q>
          </p>
        )}
      </div>
      {reloads && (
        <button
          type="button"
          ref={failure.buttonRef}
          aria-disabled={change.refreshing}
          onClick={change.refresh}
        >
          {change.refreshing ? 'Reloading…' : 'Reload the workspace'}
        </button>
      )}
    </div>
  );
}
