/**
 * The destructive actions of Administration (ruling C3): a workspace purge, an item purge, a
 * restore, a connector's revocation. Each asks through one alert dialog (`reviews/ConfirmDialog`)
 * that says exactly what will happen, and no further challenge: the service's `admin` grant, the
 * human session and the `base_revision` are the guard (SPEC section 8).
 *
 * The dialog is bound to what it was opened with (`open(values)`): the revision the page showed
 * then, whatever the head has become since, so the service answers a conflict rather than acting
 * on something the person did not see. A request has one idempotency key, kept while the same
 * values are asked again (a retry from the dialog's failure, or the dialog closed after a failure
 * and opened again on the same values), and a new one once the service has answered or for other
 * values (`attemptFor`). A failure is shown in the dialog with its retry (`useRetry`); keyboard
 * focus moves into the dialog when it opens, onto Cancel, and back to what opened it when it
 * closes.
 */
import { useMutation } from '@tanstack/react-query';
import { type ReactNode, type RefObject, useRef, useState } from 'react';
import { FailureNotice } from '../errors/FailureNotice';
import type { FailureMessage } from '../errors/failure';
import { type FailureRetry, useRetry } from '../errors/retry';
import { ConfirmDialog } from '../reviews/ConfirmDialog';

/** One request of a destructive action: what it acts on, and its idempotency key. */
export interface Attempt<V> {
  values: V;
  key: string;
}

/** The attempt for these values: the last one again while it says the same, else a new key. */
export function attemptFor<V>(last: Attempt<V> | null, values: V): Attempt<V> {
  if (last !== null && JSON.stringify(last.values) === JSON.stringify(values)) return last;
  return { values, key: crypto.randomUUID() };
}

export interface DestructiveOptions<V, R> {
  /** Send the request. */
  act: (attempt: Attempt<V>) => Promise<R>;
  /** The service accepted: what it answered, for what it was asked. The dialog has closed. */
  onDone?: (result: R, values: V) => void;
  /** What opens the dialog: focus goes back to it when the dialog closes. */
  returnFocus: RefObject<HTMLElement | null>;
}

export interface Destructive<V> {
  /** What the open dialog is bound to; null while it is closed. */
  opened: V | null;
  /** Open the dialog for these values; a failure of an earlier request is not shown in it. */
  open: (values: V) => void;
  /** Close it without acting; nothing is sent. Not while the request is out. */
  close: () => void;
  /** Send the request for what the dialog is bound to. */
  confirm: () => void;
  /** The request is out. */
  busy: boolean;
  /** Its failure, kept through its retry. */
  failure: FailureRetry;
  /** What opened the dialog. */
  returnFocus: RefObject<HTMLElement | null>;
}

export function useDestructive<V, R>({
  act,
  onDone,
  returnFocus,
}: DestructiveOptions<V, R>): Destructive<V> {
  const [opened, setOpened] = useState<V | null>(null);
  const last = useRef<Attempt<V> | null>(null);
  const send = useMutation({
    mutationFn: act,
    onSuccess: (result, attempt) => {
      // The service answered: whatever is asked next is another request.
      last.current = null;
      setOpened(null);
      onDone?.(result, attempt.values);
    },
  });
  const run = () => {
    if (opened === null) return Promise.resolve();
    const attempt = attemptFor(last.current, opened);
    last.current = attempt;
    return send.mutateAsync(attempt);
  };
  const failure = useRetry({
    request: JSON.stringify(opened),
    failed: send.isError && opened !== null,
    error: send.error,
    run,
    loaded: () => returnFocus.current,
  });
  return {
    opened,
    open: (values) => {
      send.reset();
      setOpened(values);
    },
    close: () => {
      if (!send.isPending) setOpened(null);
    },
    confirm: () => {
      if (!send.isPending) void run().catch(() => undefined);
    },
    busy: send.isPending,
    failure,
    returnFocus,
  };
}

export interface DestructiveDialogProps<V> {
  action: Destructive<V>;
  /** The question, which names the dialog. */
  title: string;
  /** What confirming does, in words. */
  description: ReactNode;
  confirmLabel: string;
  busyLabel: string;
  /** What the person was doing, for the failure's notice. */
  context: string;
  /** The workspace the action belongs to, while it is still there. */
  workspaceId?: string;
  /** Words of this place for a failure, where the general ones would mislead. */
  words?: FailureMessage;
  /** Exactly what is acted on. */
  children: ReactNode;
}

/** The alert dialog of a destructive action, with its failure and retry inside it. */
export function DestructiveDialog<V>({
  action,
  title,
  description,
  confirmLabel,
  busyLabel,
  context,
  workspaceId,
  words,
  children,
}: DestructiveDialogProps<V>) {
  return (
    <ConfirmDialog
      open={action.opened !== null}
      onClose={action.close}
      title={title}
      description={description}
      confirmLabel={confirmLabel}
      busyLabel={busyLabel}
      busy={action.busy}
      onConfirm={action.confirm}
      returnFocus={action.returnFocus}
      failure={
        action.failure.shown ? (
          <FailureNotice
            context={context}
            error={action.failure.error}
            retry={action.failure}
            {...(workspaceId === undefined ? {} : { workspaceId })}
            {...(words === undefined ? {} : { words })}
          />
        ) : null
      }
    >
      {children}
    </ConfirmDialog>
  );
}
