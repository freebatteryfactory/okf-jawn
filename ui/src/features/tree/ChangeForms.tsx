/**
 * The forms that change the tree, each shown in place under the control that opened it: a new note,
 * a new folder, a move or rename, and the question before a delete. The folder tree and an open
 * tab show the same forms.
 *
 * A name is checked before anything is sent (`names.ts`). A value the service refuses as
 * `invalid_input` is shown at the field it names, in the service's words, when the form draws that
 * field (`drawn`); a refusal at a field the form does not draw is a notice in the form that names
 * the field in plain words (`fields.ts`). So a refused change is always said, by every form here
 * and by every caller of `ChangeFailure`. Any other failure is a notice in the form, which stays
 * open with what was typed:
 * - a `conflict` says that nothing was changed, and why. When its detail names the head the
 *   request was compared with (`stale_base`, ruling 5), the notice offers to show that version, at
 *   once: a change is made on the version the person sees, never on one they have not seen.
 *   Otherwise the head is asked for once: when the workspace has a newer version than the one
 *   shown, the notice offers Refresh; when it has not, the name is taken or the change no longer
 *   fits, and the person chooses another name. A `conflict` because the workspace is archived
 *   (`workspace_archived`) is none of these: it says so, and when, and offers nothing to repeat;
 * - `not_implemented` reads "not available in this build";
 * - what may pass when repeated offers its retry (`useRetry`), which repeats the same request
 *   under the same idempotency key.
 * Nothing is shown as done until the service has answered that it is.
 *
 * Focus: a form opens with focus in its first field, and the question before a delete with focus
 * on the choice that keeps the item. Each submit button keeps focus while it works
 * (`aria-disabled`). The caller says where focus goes when a form closes.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { listTypesOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { ItemSummary, Revision } from '../../api/generated/types.gen';
import type { DraftMark } from '../drafts/marks';
import type { DraftAfterChange } from '../drafts/saving';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, messageFor } from '../errors/failure';
import { type FailureRetry, useRetry } from '../errors/retry';
import { adoptHead, checkHeadsAgain, type HeadChange, headMoved } from '../explorer/head';
import {
  type Attempt,
  attemptFor,
  type CreatedNote,
  createNewFolder,
  createNote,
  DRAFT_AFTER,
  deleteItemNow,
  type ItemChange,
  moveItemTo,
  type NewNote,
} from './changes';
import { useWorkspaceChanges } from './context';
import { fieldInWords } from './fields';
import {
  folderProblem,
  joinPath,
  noteFileName,
  pathProblem,
  segmentProblem,
  splitPath,
} from './names';

/** Whether the workspace has a newer head than the one shown; `unknown` when it could not be asked. */
type Moved = 'checking' | 'moved' | 'same' | 'unknown';

/**
 * What a change did, to be said politely, and how to say it. A change is said as done when the
 * service has answered it. When it was a change of an item, how the person's draft of that item
 * then stands is known later (`ItemChange.draft`: the draft follows after the change, and may
 * wait for a save of its own): it is said after the first words when it comes, and only while
 * that change is still the one last spoken of.
 */
export function useSaid(): [string, (done: string, draft?: Promise<DraftAfterChange>) => void] {
  const [said, setSaid] = useState('');
  const spoken = useRef(0);
  const say = (done: string, draft?: Promise<DraftAfterChange>) => {
    spoken.current += 1;
    const mine = spoken.current;
    setSaid(done);
    void draft?.then((outcome) => {
      const words = DRAFT_AFTER[outcome];
      if (words !== '' && spoken.current === mine) setSaid(`${done} ${words}`);
    });
  };
  return [said, say];
}

export interface Change<T> {
  start: (values: T) => void;
  pending: boolean;
  /** The failure shown as a notice (not one shown at a field). */
  failure: FailureRetry;
  /** The field the service refused, as it names it (`/path`), with its words. */
  refused: { field: string; message: string } | null;
  /** Set while the failure is a `conflict`: what the head check found. */
  conflict: Moved | null;
  /** The head a `stale_base` refusal named; null for any other failure. */
  staleHead: Revision | null;
  /** Show the workspace at `staleHead`; the refusal is then over, and nothing is sent again. */
  showHead: () => void;
  /** What the person is told politely: a refresh that was made for them. */
  said: string;
  refresh: () => void;
  refreshing: boolean;
  /** Forget the last failure: the person changed what they typed. */
  edited: () => void;
  /**
   * The failed request is no longer one the page would send (`current`), so its notice offers no
   * retry and says what to do instead.
   */
  withdrawn: boolean;
}

/**
 * One kind of change, asked through `run`: its request (the same one again while it says the
 * same), its failure and what the failure allows. `submit` names the form's submit button, where
 * keyboard focus goes when a notice leaves.
 *
 * `current` says whether a request of these values is still one the page would send. A failed
 * request is repeated by its notice's retry only while it is: a retry never sends what the page
 * itself would no longer send (a preview that is out of date or no longer on screen, rules that
 * were edited since). Without `current` a failed request can always be repeated; the forms whose
 * values are typed forget the failure when a value changes (`edited`).
 */
export function useChange<T, R extends HeadChange>(
  run: (
    client: ReturnType<typeof useQueryClient>,
    workspaceId: string,
    attempt: Attempt<T>,
  ) => Promise<R>,
  onDone: (result: R, values: T) => void,
  submit: RefObject<HTMLElement | null>,
  current?: (values: T) => boolean,
): Change<T> {
  const { workspaceId, head, refresh, onChanged } = useWorkspaceChanges();
  const client = useQueryClient();
  const last = useRef<Attempt<T> | null>(null);
  const [moved, setMoved] = useState<Moved>('checking');
  const [said, setSaid] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  const mutation = useMutation({
    mutationFn: (attempt: Attempt<T>) => run(client, workspaceId, attempt),
    onSuccess: (result, attempt) => {
      last.current = null;
      // Tabs follow to the revision the change made only when the shell shows it.
      if (result.adopted) onChanged(result);
      else setSaid(NOT_ADOPTED);
      onDone(result, attempt.values);
    },
    onError: async (error, attempt) => {
      const failure = classifyFailure(error);
      if (failure.kind !== 'api' || failure.error.code !== 'conflict') return;
      // An archived workspace refuses whatever its head: nothing to ask of it.
      if (failure.error.detail?.kind === 'workspace_archived') return;
      // A conflict may mean a newer head: the shell's own check runs again and announces it.
      checkHeadsAgain(client, workspaceId);
      // The refusal names the head it compared with: the head moved, and there is no need to ask.
      if (failure.error.detail?.kind === 'stale_base') {
        setMoved('moved');
        return;
      }
      const answer = await headMoved(workspaceId, attempt.base);
      setMoved(answer === null ? 'unknown' : answer ? 'moved' : 'same');
    },
  });
  const failure = mutation.isError ? classifyFailure(mutation.error) : null;
  const api = failure?.kind === 'api' ? failure.error : null;
  const refused =
    api !== null && api.code === 'invalid_input' && typeof api.field === 'string'
      ? { field: api.field, message: api.message }
      : null;
  // A conflict about the head or what the workspace holds; an archived workspace is said as such.
  const conflict = api?.code === 'conflict' && api.detail?.kind !== 'workspace_archived';
  const staleHead = api?.detail?.kind === 'stale_base' ? api.detail.head : null;
  // The request that failed, while its failure is the one shown.
  const failed = mutation.isError ? mutation.variables : undefined;
  const withdrawn = failed !== undefined && current !== undefined && !current(failed.values);
  const retry = useRetry({
    request: workspaceId,
    failed: mutation.isError,
    error: mutation.error,
    // No way to repeat a request the page would no longer send. (The notices of a conflict and of
    // a refusal at a field offer no retry button whatever is given here.)
    ...(withdrawn
      ? {}
      : {
          run: () =>
            last.current === null ? Promise.resolve() : mutation.mutateAsync(last.current),
        }),
    loaded: () => submit.current,
  });
  return {
    start: (values) => {
      if (mutation.isPending) return;
      setMoved('checking');
      setSaid('');
      const attempt = attemptFor(last.current, head, values);
      last.current = attempt;
      mutation.mutate(attempt);
    },
    pending: mutation.isPending,
    failure: retry,
    refused,
    conflict: conflict ? moved : null,
    staleHead,
    showHead: () => {
      if (staleHead === null) return;
      adoptHead(client, workspaceId, staleHead);
      checkHeadsAgain(client, workspaceId);
      // The refusal is over with the version it was about; the form asks again on the new one.
      mutation.reset();
      setSaid(
        `The workspace now shows revision ${staleHead.slice(0, 10)}. Nothing was changed yet: check this form, then press its button again.`,
      );
    },
    said,
    refreshing,
    refresh: () => {
      if (refreshing) return;
      setRefreshing(true);
      void refresh().then((refreshed) => {
        setRefreshing(false);
        if (refreshed === null) {
          setSaid(
            'The workspace could not be refreshed; the line at the top of the page says why.',
          );
          return;
        }
        // The refusal is over with the version it was about; the form asks again on the new one.
        mutation.reset();
        setSaid(
          'The workspace was refreshed. Nothing was changed yet: check this form, then press its button again.',
        );
      });
    },
    edited: () => {
      if (mutation.isError) mutation.reset();
    },
    withdrawn,
  };
}

/** Said when a change was made and the head shown had moved on while it was asked. */
export const NOT_ADOPTED =
  'The change was made, but the workspace shown here moved on while it was asked, so this page still shows the version it showed. Refresh the workspace to see the change.';

const CONFLICT_WORDS: Record<Moved, string> = {
  checking:
    'The service refused it as a conflict. Checking whether the workspace has a newer version…',
  moved:
    'The workspace has a newer version than the one shown here, and a change is only made on the version you see. Refresh the workspace, check this form, then press its button again.',
  same: 'The service refused it because it does not fit what the workspace holds now. A name that is already taken is the usual cause: choose another name, then press the button again.',
  unknown:
    'The service refused it as a conflict, and it could not be checked whether the workspace has a newer version. Refresh the workspace, or choose another name, then press the button again.',
};

const NO_FIELDS: ReadonlySet<string> = new Set();

/**
 * A change's failure as a notice in its form. A refusal at a field the form draws (`drawn`) is
 * shown at that field by the form, so nothing is shown here; a refusal at any other field is shown
 * here, with the field named in words.
 */
export function ChangeFailure<T>({
  context,
  change,
  sameWords,
  drawn = NO_FIELDS,
  withdrawnWords = 'What this page holds has changed since, so the same request is not sent again. Check it, then press the button again.',
}: {
  context: string;
  change: Change<T>;
  /** What a conflict means for this change when the head has not moved (not a taken name). */
  sameWords?: string;
  /** The fields, as the service names them, whose refusal the form shows at the field itself. */
  drawn?: ReadonlySet<string>;
  /** What to do in place of a retry that was withdrawn (`Change.withdrawn`). */
  withdrawnWords?: string;
}) {
  const { workspaceId } = useWorkspaceChanges();
  const { failure, conflict } = change;
  const status = (
    // Present with the form, so a refresh made for the person is announced.
    <p role="status" className="meta">
      {change.said}
    </p>
  );
  const { refused } = change;
  if (!failure.shown || (refused !== null && drawn.has(refused.field))) return status;
  if (refused !== null) {
    return (
      <>
        {/* Followed by the retry hook like any failure notice, though it offers no retry: the
            person changes the value first. */}
        <div ref={failure.noticeRef} tabIndex={-1} className="failure-notice">
          <div role="alert">
            <p>
              <strong>{context}: nothing was changed.</strong>
            </p>
            <p>The service did not accept {fieldInWords(refused.field)}.</p>
            {refused.message.length > 0 && (
              <p>
                The service said: <q>{refused.message}</q>
              </p>
            )}
          </div>
        </div>
        {status}
      </>
    );
  }
  if (conflict === null && change.withdrawn) {
    const failed = classifyFailure(failure.error);
    const words = failed.kind === 'api' ? failed.error.message : '';
    return (
      <>
        {/* The failure, without its retry: followed by the retry hook like any failure notice. */}
        <div ref={failure.noticeRef} tabIndex={-1} className="failure-notice">
          <div role="alert">
            <p>
              <strong>
                {context}: {messageFor(failed).title}
              </strong>
            </p>
            <p>{withdrawnWords}</p>
            {words.length > 0 && (
              <p>
                The service said: <q>{words}</q>
              </p>
            )}
          </div>
        </div>
        {status}
      </>
    );
  }
  if (conflict === null) {
    return (
      <>
        <FailureNotice
          context={context}
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
        {status}
      </>
    );
  }
  const refusal = classifyFailure(failure.error);
  const words = refusal.kind === 'api' ? refusal.error.message : '';
  return (
    <>
      {/* Followed by the retry hook like any failure notice, though it offers no retry. */}
      <div ref={failure.noticeRef} tabIndex={-1} className="failure-notice">
        <div role="alert">
          <p>
            <strong>{context}: nothing was changed.</strong>
          </p>
          <p>
            {change.staleHead !== null
              ? `The workspace has a newer version than the one shown here: it now holds revision ${change.staleHead.slice(0, 10)}, and a change is only made on the version you see. Show that version, check this form, then press its button again.`
              : conflict === 'same' && sameWords !== undefined
                ? sameWords
                : CONFLICT_WORDS[conflict]}
          </p>
          {words.length > 0 && (
            <p>
              The service said: <q>{words}</q>
            </p>
          )}
        </div>
        {change.staleHead !== null && (
          <button type="button" ref={failure.buttonRef} onClick={change.showHead}>
            Show revision {change.staleHead.slice(0, 10)}
          </button>
        )}
        {change.staleHead === null && (conflict === 'moved' || conflict === 'unknown') && (
          <button
            type="button"
            ref={failure.buttonRef}
            aria-disabled={change.refreshing}
            onClick={change.refresh}
          >
            {change.refreshing ? 'Refreshing…' : 'Refresh the workspace'}
          </button>
        )}
      </div>
      {status}
    </>
  );
}

interface FieldProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** What is wrong with the value; null when nothing is. */
  error: string | null;
  inputRef?: RefObject<HTMLInputElement | null>;
  hint?: string;
  /** Suggested values. */
  suggestions?: readonly string[];
}

/** A labelled text field with its hint and, when its value is refused, the reason beside it. */
function Field({ label, value, onChange, error, inputRef, hint, suggestions }: FieldProps) {
  const id = useId();
  const hintId = useId();
  const errorId = useId();
  const listId = useId();
  const described = [hint === undefined ? null : hintId, error === null ? null : errorId].filter(
    (part) => part !== null,
  );
  return (
    <div className="change-field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        ref={inputRef}
        type="text"
        value={value}
        autoComplete="off"
        aria-invalid={error !== null}
        {...(described.length > 0 ? { 'aria-describedby': described.join(' ') } : {})}
        {...(suggestions === undefined ? {} : { list: listId })}
        onChange={(event) => onChange(event.target.value)}
      />
      {suggestions !== undefined && (
        <datalist id={listId}>
          {suggestions.map((suggestion) => (
            <option key={suggestion} value={suggestion} />
          ))}
        </datalist>
      )}
      {hint !== undefined && (
        <p id={hintId} className="meta">
          {hint}
        </p>
      )}
      {error !== null && (
        <p id={errorId} className="field-error">
          {error}
        </p>
      )}
    </div>
  );
}

/** The two buttons every form ends with: the one that sends, and the one that leaves. */
function FormButtons({
  submitRef,
  pending,
  label,
  working,
  onCancel,
}: {
  submitRef: RefObject<HTMLButtonElement | null>;
  pending: boolean;
  label: string;
  working: string;
  onCancel: () => void;
}) {
  return (
    <p className="change-buttons">
      {/* aria-disabled rather than disabled: the button keeps focus while the change is made. */}
      <button type="submit" ref={submitRef} aria-disabled={pending}>
        {pending ? working : label}
      </button>{' '}
      <button type="button" onClick={onCancel}>
        Cancel
      </button>
    </p>
  );
}

/** Escape, pressed inside a form, leaves it like its Cancel button. */
function leaveOnEscape(onCancel: () => void) {
  return (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    onCancel();
  };
}

/** Focus the first field when a form opens: in the commit that shows it, before it is painted. */
function useOpeningFocus(target: RefObject<HTMLElement | null>) {
  useLayoutEffect(() => {
    target.current?.focus();
  }, [target]);
}

const FOLDER_HINT = 'Leave empty for the top level. Separate folders with “/”.';
const PATH_FIELDS: ReadonlySet<string> = new Set(['/path', '/folder', '/destination']);
const TITLE_FIELDS: ReadonlySet<string> = new Set(['/title']);
const TYPE_FIELDS: ReadonlySet<string> = new Set(['/type_name']);
/** The fields each form draws: a refusal at one of them is shown there, not in the notice. */
const NOTE_FIELDS: ReadonlySet<string> = new Set([...PATH_FIELDS, ...TITLE_FIELDS, ...TYPE_FIELDS]);

/** The words of a refusal the service made about `fields`; null when it made none about them. */
/**
 * Keyboard focus to the field a refusal names (its `field` is a JSON Pointer into the request,
 * ruling 5), once, when the refusal arrives: the person reads its words there and changes the
 * value. `fields` maps the pointers a form draws to their inputs.
 */
function useFocusRefused<T>(
  change: Change<T>,
  fields: ReadonlyArray<readonly [ReadonlySet<string>, RefObject<HTMLElement | null>]>,
) {
  const field = change.refused?.field ?? null;
  const focused = useRef<string | null>(null);
  const drawn = useRef(fields);
  useLayoutEffect(() => {
    drawn.current = fields;
  });
  useLayoutEffect(() => {
    if (field === null) {
      focused.current = null;
      return;
    }
    if (focused.current === field) return;
    focused.current = field;
    for (const [names, input] of drawn.current) {
      if (names.has(field)) {
        input.current?.focus();
        return;
      }
    }
  }, [field]);
}

function refusedAt<T>(change: Change<T>, fields: ReadonlySet<string>): string | null {
  return change.refused !== null && fields.has(change.refused.field)
    ? `The service refused this: ${change.refused.message}`
    : null;
}

export interface NewEntryFormProps {
  kind: 'note' | 'folder';
  /** The folder the form starts in; '' is the top level. */
  folder: string;
  /** A note was created (`item`), or a folder (`folder`). */
  onDone: (created: CreatedNote | (HeadChange & { folder: string })) => void;
  onCancel: () => void;
}

/** A new note or a new folder: where, and under which name. */
export function NewEntryForm({ kind, folder: startFolder, onDone, onCancel }: NewEntryFormProps) {
  const { workspaceId } = useWorkspaceChanges();
  const firstRef = useRef<HTMLInputElement | null>(null);
  const nameRef = useRef<HTMLInputElement | null>(null);
  const folderRef = useRef<HTMLInputElement | null>(null);
  const typeRef = useRef<HTMLInputElement | null>(null);
  const submitRef = useRef<HTMLButtonElement | null>(null);
  useOpeningFocus(kind === 'note' ? firstRef : nameRef);
  const [folder, setFolder] = useState(startFolder);
  const [title, setTitle] = useState('');
  // Until the person types a file name, it follows the title.
  const [typedName, setTypedName] = useState<string | null>(null);
  const [typeName, setTypeName] = useState('note');
  const [checked, setChecked] = useState(false);
  const name =
    typedName ?? (kind === 'note' && title.trim().length > 0 ? `${title.trim()}.md` : '');
  const types = useQuery({
    ...listTypesOptions({ body: { workspace_id: workspaceId } }),
    enabled: kind === 'note',
  });

  const note = useChange<NewNote, CreatedNote>(createNote, (result) => onDone(result), submitRef);
  const made = useChange<{ folder: string }, HeadChange>(
    createNewFolder,
    (result, values) => onDone({ ...result, folder: values.folder }),
    submitRef,
  );
  const change = kind === 'note' ? note : made;
  useFocusRefused(note, [
    [TITLE_FIELDS, firstRef],
    [PATH_FIELDS, nameRef],
    [TYPE_FIELDS, typeRef],
  ]);
  useFocusRefused(made, [[PATH_FIELDS, nameRef]]);
  const edited = () => {
    note.edited();
    made.edited();
  };

  // A note's file ends in ".md" in small letters (`noteFileName`).
  const sent = kind === 'note' ? noteFileName(name) : name;
  const problems = {
    title: kind === 'note' && title.trim().length === 0 ? 'Give the note a title.' : null,
    folder: folderProblem(folder),
    name: segmentProblem(sent) ?? pathProblem(joinPath(folder, sent)),
    type:
      kind === 'note' && typeName.trim().length === 0
        ? 'Give the note a type: any word of your own, such as “note”.'
        : null,
  };
  const shown = (problem: string | null) => (checked ? problem : null);
  const send = () => {
    setChecked(true);
    const firstWrong = problems.title
      ? firstRef
      : problems.folder
        ? folderRef
        : problems.name
          ? nameRef
          : problems.type
            ? typeRef
            : null;
    if (firstWrong !== null) {
      firstWrong.current?.focus();
      return;
    }
    if (kind === 'note') {
      note.start({ folder, name: sent, title: title.trim(), typeName: typeName.trim() });
    } else made.start({ folder: joinPath(folder, name) });
  };

  return (
    <form
      className="change-form"
      noValidate
      onKeyDown={leaveOnEscape(onCancel)}
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      <fieldset className="confirm">
        <legend>{kind === 'note' ? 'New note' : 'New folder'}</legend>
        {kind === 'note' && (
          <Field
            label="Title"
            value={title}
            inputRef={firstRef}
            error={shown(problems.title) ?? refusedAt(note, TITLE_FIELDS)}
            onChange={(next) => {
              setTitle(next);
              edited();
            }}
          />
        )}
        <Field
          label="In folder"
          value={folder}
          inputRef={folderRef}
          hint={FOLDER_HINT}
          error={shown(problems.folder)}
          onChange={(next) => {
            setFolder(next);
            edited();
          }}
        />
        <Field
          label={kind === 'note' ? 'File name' : 'Folder name'}
          value={name}
          inputRef={nameRef}
          error={shown(problems.name) ?? refusedAt(change, PATH_FIELDS)}
          onChange={(next) => {
            setTypedName(next);
            edited();
          }}
        />
        {kind === 'note' && (
          <Field
            label="Type"
            value={typeName}
            inputRef={typeRef}
            hint={
              types.isError
                ? 'A word of your own. The types this workspace defines could not be listed.'
                : 'A word of your own, or one of the types this workspace defines.'
            }
            suggestions={types.data?.items.map((type) => type.name) ?? []}
            error={shown(problems.type) ?? refusedAt(note, TYPE_FIELDS)}
            onChange={(next) => {
              setTypeName(next);
              edited();
            }}
          />
        )}
        <FormButtons
          submitRef={submitRef}
          pending={change.pending}
          label={kind === 'note' ? 'Create the note' : 'Create the folder'}
          working="Creating…"
          onCancel={onCancel}
        />
        <ChangeFailure
          context={kind === 'note' ? 'Creating the note' : 'Creating the folder'}
          change={change}
          drawn={kind === 'note' ? NOTE_FIELDS : PATH_FIELDS}
        />
      </fieldset>
    </form>
  );
}

/** What a move or a status change does to the person's draft of the item, when they have one. */
export function DraftWarning({ mark, children }: { mark: DraftMark | null; children: ReactNode }) {
  if (mark === null) return null;
  return (
    <p role="note" className="notice">
      {children}
    </p>
  );
}

export interface MoveFormProps {
  item: ItemSummary;
  /** How the person's draft of the item stands, when they have one. */
  mark: DraftMark | null;
  /** The item was moved; how the person's draft of it then stands comes later (`moved.draft`). */
  onDone: (moved: ItemChange, destination: string) => void;
  onCancel: () => void;
}

/** Move or rename an item: another folder, another file name, or both. */
export function MoveForm({ item, mark, onDone, onCancel }: MoveFormProps) {
  const nameRef = useRef<HTMLInputElement | null>(null);
  const folderRef = useRef<HTMLInputElement | null>(null);
  const submitRef = useRef<HTMLButtonElement | null>(null);
  useOpeningFocus(nameRef);
  const from = splitPath(item.path);
  const [folder, setFolder] = useState(from.folder);
  const [name, setName] = useState(from.name);
  const [checked, setChecked] = useState(false);
  const change = useChange<{ itemId: string; destination: string }, ItemChange>(
    moveItemTo,
    (result, values) => onDone(result, values.destination),
    submitRef,
  );
  // An item whose file ends in ".md" keeps that ending in small letters (`noteFileName`).
  const sent = item.path.endsWith('.md') ? noteFileName(name) : name;
  const destination = joinPath(folder, sent);
  useFocusRefused(change, [[PATH_FIELDS, nameRef]]);
  const problems = {
    folder: folderProblem(folder),
    name:
      segmentProblem(sent) ??
      pathProblem(destination) ??
      (destination === item.path
        ? 'This is where it is now. Change the name or the folder.'
        : null),
  };
  const shown = (problem: string | null) => (checked ? problem : null);
  const title = item.title || item.path;
  return (
    <form
      className="change-form"
      noValidate
      onKeyDown={leaveOnEscape(onCancel)}
      onSubmit={(event) => {
        event.preventDefault();
        setChecked(true);
        if (problems.folder !== null) folderRef.current?.focus();
        else if (problems.name !== null) nameRef.current?.focus();
        else change.start({ itemId: item.id, destination });
      }}
    >
      <fieldset className="confirm">
        <legend>Rename or move “{title}”</legend>
        <p className="meta">
          It is at {item.path} now. Links to it are rewritten, and its open tabs stay open.
        </p>
        <Field
          label="File name"
          value={name}
          inputRef={nameRef}
          error={shown(problems.name) ?? refusedAt(change, PATH_FIELDS)}
          onChange={(next) => {
            setName(next);
            change.edited();
          }}
        />
        <Field
          label="In folder"
          value={folder}
          inputRef={folderRef}
          hint={FOLDER_HINT}
          error={shown(problems.folder)}
          onChange={(next) => {
            setFolder(next);
            change.edited();
          }}
        />
        <DraftWarning mark={mark}>
          You have a draft of this item. It is kept, and follows the item: it is saved again on the
          version this change makes, and what comes of that is said here.
        </DraftWarning>
        <FormButtons
          submitRef={submitRef}
          pending={change.pending}
          label="Rename or move it"
          working="Moving…"
          onCancel={onCancel}
        />
        <ChangeFailure context="Renaming or moving this item" change={change} drawn={PATH_FIELDS} />
      </fieldset>
    </form>
  );
}

export interface DeleteQuestionProps {
  item: ItemSummary;
  mark: DraftMark | null;
  onDone: (deleted: HeadChange) => void;
  onCancel: () => void;
}

/** The question before a delete: what is removed, what is kept, and what happens to a draft. */
export function DeleteQuestion({ item, mark, onDone, onCancel }: DeleteQuestionProps) {
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const deleteRef = useRef<HTMLButtonElement | null>(null);
  useOpeningFocus(keepRef);
  const title = item.title || item.path;
  const change = useChange<{ itemId: string; title: string }, HeadChange>(
    deleteItemNow,
    (result) => onDone(result),
    deleteRef,
  );
  // The question is a group, not a form: nothing is typed, and Enter on "Keep it" keeps it.
  return (
    <fieldset className="confirm change-form" onKeyDown={leaveOnEscape(onCancel)}>
      <legend>Delete “{title}”?</legend>
      <p>
        {item.path} is removed from the workspace as it is now: one item. Its history is kept, so
        its earlier versions can still be read.
      </p>
      <DraftWarning mark={mark}>
        {mark === 'draft'
          ? 'You have a draft of this item. Deleting the item does not delete your draft, but it can no longer become part of the workspace through a Snapshot. The deleted item’s tab, which the Drafts list opens too, shows the draft read-only, to copy from, and offers to discard it.'
          : 'You have text of this item that is not saved as a draft. After the delete it can no longer be saved, nor become part of the workspace through a Snapshot. The deleted item’s tab shows it read-only, to copy from, until you discard it or leave this page.'}
      </DraftWarning>
      <p className="change-buttons">
        <button type="button" ref={keepRef} onClick={onCancel}>
          Keep it
        </button>{' '}
        <button
          type="button"
          ref={deleteRef}
          aria-disabled={change.pending}
          onClick={() => change.start({ itemId: item.id, title })}
        >
          {change.pending ? 'Deleting…' : 'Delete it'}
        </button>
      </p>
      <ChangeFailure context="Deleting this item" change={change} />
    </fieldset>
  );
}
