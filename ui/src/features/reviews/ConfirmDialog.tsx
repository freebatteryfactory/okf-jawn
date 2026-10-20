/**
 * The one confirmation dialog of the workspace: Verify and Approve ask through it, showing exactly
 * what the action will cover or accept (ruling R5), on Base UI's alert dialog.
 *
 * It is modal: keyboard focus moves into it when it opens, onto Cancel, the action that changes
 * nothing, and stays in it; Escape and Cancel close it; when it closes, focus goes back to what
 * opened it (`returnFocus`). While the action runs, its button keeps focus with `aria-disabled`,
 * says so, and the dialog does not close; a failure is shown inside it with its retry, which the
 * caller holds (`useRetry`).
 */
import { AlertDialog } from '@base-ui/react/alert-dialog';
import { type ReactNode, type RefObject, useRef } from 'react';

export interface ConfirmDialogProps {
  open: boolean;
  /** Close without acting (Cancel, Escape): nothing is sent. */
  onClose: () => void;
  /** The question, which names the dialog. */
  title: string;
  /** What confirming does, in words; it describes the dialog. */
  description: ReactNode;
  /** Exactly what is confirmed. */
  children: ReactNode;
  confirmLabel: string;
  busyLabel: string;
  /** The action is under way. */
  busy: boolean;
  onConfirm: () => void;
  /** The action's failure, shown inside the dialog. */
  failure?: ReactNode;
  /** What opened the dialog: focus goes back to it when the dialog closes. */
  returnFocus: RefObject<HTMLElement | null>;
}

export function ConfirmDialog({
  open,
  onClose,
  title,
  description,
  children,
  confirmLabel,
  busyLabel,
  busy,
  onConfirm,
  failure,
  returnFocus,
}: ConfirmDialogProps) {
  const cancelRef = useRef<HTMLButtonElement | null>(null);
  return (
    <AlertDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next && !busy) onClose();
      }}
    >
      <AlertDialog.Portal>
        <AlertDialog.Backdrop className="dialog-backdrop" />
        <AlertDialog.Popup className="dialog" initialFocus={cancelRef} finalFocus={returnFocus}>
          <AlertDialog.Title className="dialog-title">{title}</AlertDialog.Title>
          <AlertDialog.Description render={<div />}>{description}</AlertDialog.Description>
          <div className="dialog-content">{children}</div>
          {failure}
          <p className="dialog-buttons">
            {/* aria-disabled, never disabled: the button keeps focus while the action runs. */}
            <button
              type="button"
              aria-disabled={busy}
              onClick={() => {
                if (!busy) onConfirm();
              }}
            >
              {busy ? busyLabel : confirmLabel}
            </button>{' '}
            <button
              type="button"
              ref={cancelRef}
              aria-disabled={busy}
              onClick={() => {
                if (!busy) onClose();
              }}
            >
              Cancel
            </button>
          </p>
          {/* Present from the start, so the action is announced when it starts. */}
          <p role="status" className="sr-only">
            {busy ? busyLabel : ''}
          </p>
        </AlertDialog.Popup>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
