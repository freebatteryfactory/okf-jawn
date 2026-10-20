/**
 * New workspace (ruling C9): a name and a description, and `create_workspace`, which makes the
 * workspace blank, with no example content, and records its creator's grant there. Once the
 * service has answered, the new workspace opens (`/w/<id>`), and the workspace list is asked again
 * so it is listed there.
 *
 * Who may. `create_workspace` needs `admin` on the installation, and the contract names no tenant
 * grant a client can read: the form is offered to whoever reaches this page, the service decides,
 * and its `forbidden` is said in words with what the person can do (`installation-words.ts`).
 *
 * Reached from the workspace list ("New workspace", `NewWorkspaceLink.tsx`) at this page's address
 * with this section's name as its fragment (`/admin#new-workspace`): the router brings the section
 * into view, and the name field takes keyboard focus as the section mounts (in a layout effect, so
 * the router's focus rule finds focus on the page and leaves it there).
 *
 * Idempotency, as for an export (`exports/ExportStart.tsx`): a press is one request under an
 * idempotency key, kept while the same values are asked again (the notice's "Try again", or a
 * press after a failure with the fields unchanged), so a request whose answer was lost answers
 * the workspace it made rather than making a second one (the service names the workspace from
 * the key's mutation). Once the service has answered, the next press is a new workspace; other
 * values are a new request too (`attemptFor`). What is said is about what the fields ask now: a
 * failure of other values is not shown, and its "Try again" never repeats them.
 *
 * A name is asked for before anything is sent: an empty one would make a workspace listed by
 * nothing. The service's own rules on names, if any, are its to answer. The button keeps focus
 * while the request is out (`aria-disabled`); on success the router's focus rule takes focus to
 * the new workspace's heading.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useLocation, useNavigate } from '@tanstack/react-router';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import { createWorkspace } from '../../../api/generated/sdk.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { type Attempt, attemptFor } from '../destructive';
import { createWords } from './installation-words';

/** The fragment that names this section: a link to it focuses the name field. */
export const NEW_WORKSPACE_HASH = 'new-workspace';

/** What a new workspace is asked with. */
interface NewValues {
  name: string;
  description: string;
}

/**
 * The workspace list in the cache, whatever its page size (TanStack Query matches the leading part
 * of a key): `_id` is the generated name of `list_workspaces`.
 */
const WORKSPACE_LIST = [{ _id: 'listWorkspaces' }] as const;

export function NewWorkspace() {
  const headingId = useId();
  const aboutId = useId();
  const hintId = useId();
  const missingId = useId();
  const client = useQueryClient();
  const navigate = useNavigate();
  const hash = useLocation({ select: (location) => location.hash });
  const nameRef = useRef<HTMLInputElement | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [missing, setMissing] = useState(false);

  // The last attempt, until the service answers it.
  const last = useRef<Attempt<NewValues> | null>(null);
  const send = useMutation({
    mutationFn: async (attempt: Attempt<NewValues>) => {
      const { data } = await createWorkspace({
        body: {
          name: attempt.values.name,
          description: attempt.values.description,
          idempotency_key: attempt.key,
        },
        throwOnError: true,
      });
      return data;
    },
    onSuccess: (created, attempt) => {
      if (last.current === attempt) last.current = null;
      void client.invalidateQueries({ queryKey: WORKSPACE_LIST });
      void navigate({ to: '/w/$workspaceId', params: { workspaceId: created.id } });
    },
  });

  const values: NewValues = { name: name.trim(), description: description.trim() };
  const request = JSON.stringify(values);
  // The last press is shown only while it asked what the fields ask now.
  const asked = send.variables;
  const mine = asked !== undefined && JSON.stringify(asked.values) === request;
  const failure = useRetry({
    request,
    failed: mine && send.isError,
    error: mine ? send.error : null,
    run: () => (mine ? send.mutateAsync(asked) : Promise.resolve()),
    loaded: () => buttonRef.current,
  });
  const pending = mine && send.isPending;

  // Reached by its fragment: the name field takes focus as the section mounts.
  useLayoutEffect(() => {
    if (hash === NEW_WORKSPACE_HASH) nameRef.current?.focus();
  }, [hash]);

  const submit = () => {
    if (send.isPending) return;
    if (values.name.length === 0) {
      setMissing(true);
      nameRef.current?.focus();
      return;
    }
    const attempt = attemptFor(last.current, values);
    last.current = attempt;
    send.mutate(attempt);
  };

  return (
    <section id={NEW_WORKSPACE_HASH} aria-labelledby={headingId} className="mt-8 grid gap-2">
      <h2 id={headingId} className="page-heading">
        New workspace
      </h2>
      <p id={aboutId}>
        A new workspace is created blank, with no example content, and you hold administrator rights
        in it. It opens once it is created.
      </p>
      <p className="meta">
        Creating a workspace needs administrator rights for this installation. The service decides,
        and says so if you do not have them.
      </p>
      <form
        noValidate
        className="grid justify-items-start gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <p className="change-field">
          <label htmlFor={`${headingId}-name`}>Name</label>
          <input
            id={`${headingId}-name`}
            ref={nameRef}
            type="text"
            value={name}
            aria-invalid={missing}
            {...(missing ? { 'aria-describedby': missingId } : {})}
            onChange={(event) => {
              setName(event.target.value);
              setMissing(false);
            }}
          />
        </p>
        {missing && (
          <p id={missingId} className="field-error">
            Give the workspace a name.
          </p>
        )}
        <p className="change-field">
          <label htmlFor={`${headingId}-description`}>Description</label>
          <textarea
            id={`${headingId}-description`}
            className="w-[min(100%,30rem)] rounded border border-[color:GrayText] bg-[Field] p-1 text-[FieldText]"
            rows={3}
            value={description}
            aria-describedby={hintId}
            onChange={(event) => setDescription(event.target.value)}
          />
          <span id={hintId} className="meta">
            What it is for; it is shown under its name in your workspace list. It can stay empty.
          </span>
        </p>
        {/* aria-disabled rather than disabled: the button keeps focus while the request is out. */}
        <button type="submit" ref={buttonRef} aria-disabled={pending} aria-describedby={aboutId}>
          {pending ? 'Creating the workspace…' : 'Create the workspace'}
        </button>
        {/* Present from the start, so the request is announced when it is sent. */}
        <p role="status" className="meta">
          {pending && !failure.retrying ? 'Creating the workspace…' : ''}
        </p>
        {failure.shown && (
          <FailureNotice
            context="Creating the workspace"
            error={failure.error}
            retry={failure}
            words={createWords(failure.error)}
          />
        )}
      </form>
    </section>
  );
}
