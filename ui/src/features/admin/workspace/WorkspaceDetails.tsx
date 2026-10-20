/**
 * The workspace's name and description, changed through `update_workspace` (ruling C9).
 *
 * The form shows them as the shell holds the workspace (`open_workspace`), each until the person
 * types in it: what they typed stays through a failure and a reload, and the field they did not
 * touch follows the workspace as it is read again. A change needs `write`, the operation's
 * permission; a person without it is shown the name and description, and told why they cannot
 * change them (`Missing`). In an archived workspace the service refuses it
 * (`ALLOWED_WHILE_ARCHIVED` does not name it), so Save stays, `aria-disabled`, described by
 * `archivedWords`, the fields are read-only, and nothing is sent.
 *
 * The change is made on the head the page shows, said "as of" beside Save, and sent as its
 * `base_revision`. A conflict is said in words with a way to reload the workspace
 * (`LifecycleFailure`), then Save asks again on what the page then shows, under a new key. A name
 * is needed: an empty one is said at the field and nothing is sent. Values the workspace already
 * has are not sent either. Once the service has answered, the shell reads the workspace again,
 * and shows its new name; Save keeps keyboard focus throughout (`aria-disabled` while it works).
 */
import { type FormEvent, useId, useRef, useState } from 'react';
import type { Workspace } from '../../../api/generated/types.gen';
import { short } from '../../documents/describe';
import type { HeadChange } from '../../explorer/head';
import { allowedOperation, archivedWords, Missing } from '../../explorer/rights';
import { NOT_ADOPTED, useChange } from '../../tree/ChangeForms';
import { useWorkspaceChanges } from '../../tree/context';
import { LifecycleFailure } from './LifecycleFailure';
import { type Details, updateDetails } from './lifecycle';

const NAME_FIELD = '/name';
const DESCRIPTION_FIELD = '/description';
const DRAWN: ReadonlySet<string> = new Set([NAME_FIELD, DESCRIPTION_FIELD]);
const NOT_READ =
  'This page could not read the workspace again: the line at the top of the page says why.';

export function WorkspaceDetails({ workspace }: { workspace: Workspace }) {
  const { head, rights, refresh } = useWorkspaceChanges();
  const headingId = useId();
  const nameId = useId();
  const nameErrorId = useId();
  const descriptionId = useId();
  const descriptionErrorId = useId();
  const aboutId = useId();
  const archivedId = useId();
  const saveRef = useRef<HTMLButtonElement | null>(null);
  const nameRef = useRef<HTMLInputElement | null>(null);
  // What the person typed in each field; a field they did not touch shows the workspace's value.
  const [typed, setTyped] = useState<Partial<Details>>({});
  const [nameProblem, setNameProblem] = useState<string | null>(null);
  const [said, setSaid] = useState('');
  const change = useChange<Details, HeadChange>(
    updateDetails,
    () => {
      setTyped({});
      setSaid('The name and description were saved.');
      void refresh().then((shown) => {
        if (shown === null) setSaid(`The name and description were saved. ${NOT_READ}`);
      });
    },
    saveRef,
  );
  const can = allowedOperation(rights, 'update_workspace', 'write');
  const name = typed.name ?? workspace.name;
  const description = typed.description ?? workspace.description;

  if (can === 'no-permission') {
    return (
      <section aria-labelledby={headingId}>
        <h4 id={headingId} className="mt-3 font-bold">
          Name and description
        </h4>
        <p>
          <strong>{workspace.name}</strong>
          {workspace.description.length > 0 ? `: ${workspace.description}` : ''}
        </p>
        <Missing action="Changing this workspace’s name and description" permission="write" />
      </section>
    );
  }

  const stopped = can === 'archived';
  const refused = change.refused;
  const nameError = nameProblem ?? (refused?.field === NAME_FIELD ? refused.message : null);
  const descriptionError = refused?.field === DESCRIPTION_FIELD ? refused.message : null;
  const type = (field: keyof Details, value: string) => {
    setTyped((before) => ({ ...before, [field]: value }));
    if (field === 'name') setNameProblem(null);
    setSaid('');
    change.edited();
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (stopped || change.pending) return;
    setSaid('');
    if (name.trim().length === 0) {
      setNameProblem('Give the workspace a name.');
      nameRef.current?.focus();
      return;
    }
    if (name === workspace.name && description === workspace.description) {
      setSaid('Nothing to save: the workspace already has this name and description.');
      return;
    }
    change.start({ name, description });
  };
  const status = change.pending
    ? 'Saving…'
    : [said, change.said === NOT_ADOPTED ? '' : change.said]
        .filter((part) => part !== '')
        .join(' ');
  const describedBy = (error: string | null, errorId: string) =>
    [error === null ? null : errorId, stopped ? archivedId : null].filter((id) => id !== null);
  const nameDescribedBy = describedBy(nameError, nameErrorId);
  const descriptionDescribedBy = describedBy(descriptionError, descriptionErrorId);
  return (
    <section aria-labelledby={headingId}>
      <h4 id={headingId} className="mt-3 font-bold">
        Name and description
      </h4>
      <form noValidate onSubmit={submit}>
        <div className="change-field">
          <label htmlFor={nameId}>Name</label>
          <input
            id={nameId}
            ref={nameRef}
            type="text"
            value={name}
            autoComplete="off"
            readOnly={stopped}
            aria-invalid={nameError !== null}
            {...(nameDescribedBy.length > 0
              ? { 'aria-describedby': nameDescribedBy.join(' ') }
              : {})}
            onChange={(event) => type('name', event.target.value)}
          />
          {nameError !== null && (
            <p id={nameErrorId} className="field-error">
              {nameError}
            </p>
          )}
        </div>
        <div className="change-field">
          <label htmlFor={descriptionId}>Description</label>
          <input
            id={descriptionId}
            type="text"
            value={description}
            autoComplete="off"
            readOnly={stopped}
            aria-invalid={descriptionError !== null}
            {...(descriptionDescribedBy.length > 0
              ? { 'aria-describedby': descriptionDescribedBy.join(' ') }
              : {})}
            onChange={(event) => type('description', event.target.value)}
          />
          {descriptionError !== null && (
            <p id={descriptionErrorId} className="field-error">
              {descriptionError}
            </p>
          )}
        </div>
        <p id={aboutId} className="meta">
          A change is made on the workspace as of revision <code title={head}>{short(head)}</code>,
          the version this page shows. If it has changed since, nothing is changed: the service says
          so, and you can reload it.
        </p>
        <p className="change-buttons">
          {/* aria-disabled rather than disabled: the button keeps focus while the change is made. */}
          <button
            type="submit"
            ref={saveRef}
            aria-disabled={change.pending || stopped}
            aria-describedby={stopped ? `${archivedId} ${aboutId}` : aboutId}
          >
            {change.pending ? 'Saving…' : 'Save the name and description'}
          </button>
        </p>
        {stopped && rights.archivedAt !== null && (
          <p id={archivedId} className="meta">
            {archivedWords(rights.archivedAt)}
          </p>
        )}
        <LifecycleFailure
          context="Saving the name and description"
          change={change}
          sameWords="The service refused it as a conflict with what the workspace holds now."
          sameReloads
          drawn={DRAWN}
        />
        {/* Present from the start, so what Save did is announced. */}
        <p role="status" className="meta">
          {status}
        </p>
      </form>
    </section>
  );
}
