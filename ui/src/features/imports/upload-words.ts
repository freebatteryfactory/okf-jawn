/**
 * The words of an upload's failure, where the general words of `features/errors/failure.ts` would
 * mislead: a failure the page found by itself (the file could not be read, the service's answer
 * did not confirm the upload) is not "the service could not be reached", `too_large` for an upload
 * is not "too large to show", and a `conflict` here is not a version that changed. A `too_large`
 * answered to the rest of a file whose slot an earlier try had sent bytes to is not a file too
 * large for the service either: the slot holds more than it said (`slotIsAhead`).
 *
 * The action each allows is what a retry does (`uploading.ts`): "Try again" goes on with the same
 * slot, "Send it again" opens a new one. A `conflict` whose detail says what the slot holds against
 * what was announced (`upload_mismatch`, ruling 5) says that, in place of the general guess. Every
 * other failure keeps its general words.
 */
import { classifyFailure, type FailureMessage, mismatchWords } from '../errors/failure';
import { FileUnreadable, HashUnavailable } from './hash';
import type { UploadStep } from './upload-list';
import {
  FileChanged,
  SlotUnusable,
  slotIsAhead,
  UploadIncomplete,
  UploadNotConfirmed,
} from './uploading';

const TRY_AGAIN = { kind: 'retry', label: 'Try again' } as const;
const SEND_AGAIN = { kind: 'retry', label: 'Send it again' } as const;

export function uploadWords(failure: {
  error: unknown;
  step: UploadStep;
  resumed: boolean;
}): FailureMessage | undefined {
  const { error, step } = failure;
  if (error instanceof FileUnreadable) {
    return {
      title: 'This file could not be read.',
      advice:
        'It may have been moved, changed or deleted since you chose it, or be too large for this browser to read at once. Try again, or remove it and choose it again.',
      action: TRY_AGAIN,
    };
  }
  if (error instanceof FileChanged) {
    return {
      title: 'This file changed since you chose it.',
      advice: `It was listed with ${error.listed} bytes and now has ${error.read}. Nothing of it was sent. Remove it from the list and choose it again.`,
      action: { kind: 'none' },
    };
  }
  if (error instanceof HashUnavailable) {
    return {
      title: 'This page cannot check a file before sending it.',
      advice:
        'A browser computes a file’s SHA-256 hash only on a secure page: HTTPS, or the service on this computer. Open the workspace that way and choose the file again.',
      action: { kind: 'none' },
    };
  }
  if (error instanceof UploadIncomplete) {
    return {
      title: 'The service did not keep the whole file.',
      advice: `It holds ${error.received} of ${error.size} bytes. Try again to send the rest.`,
      action: TRY_AGAIN,
    };
  }
  if (error instanceof UploadNotConfirmed) {
    return {
      title: 'The service did not confirm this upload.',
      advice:
        'Its answer did not say that the file’s length and hash were verified, so the file is not counted as uploaded. Send it again.',
      action: SEND_AGAIN,
    };
  }
  if (error instanceof SlotUnusable) {
    return {
      title: 'The service’s answer about this upload could not be read.',
      advice: 'It did not say how many bytes of the file it holds. Send it again.',
      action: SEND_AGAIN,
    };
  }
  if (slotIsAhead(failure)) {
    return {
      title: 'The service already holds part of this file, and did not take the rest.',
      advice:
        'It holds more of the file than it said when it was asked, so what was sent would have been too much. Send it again: the file goes into a new upload.',
      action: SEND_AGAIN,
    };
  }
  const classified = classifyFailure(error);
  if (classified.kind !== 'api') return undefined;
  switch (classified.error.code) {
    case 'too_large':
      return {
        title: 'This file is larger than the service accepts.',
        advice: 'Nothing was kept of it. Remove it from the list.',
        action: { kind: 'none' },
      };
    case 'not_found':
      return {
        title: 'The service no longer holds this upload.',
        advice: 'Send the file again.',
        action: SEND_AGAIN,
      };
    case 'conflict':
      if (step !== 'bytes' && classified.error.detail?.kind === 'upload_mismatch') {
        return {
          title: 'What the service holds does not match this file.',
          advice: `${mismatchWords(classified.error.detail)} Send it again: the file goes into a new upload.`,
          action: SEND_AGAIN,
        };
      }
      return step === 'bytes'
        ? {
            title: 'The service could not take this file’s bytes now.',
            advice: 'Try again: the upload goes on from what the service already holds.',
            action: TRY_AGAIN,
          }
        : {
            title: 'The service did not accept this file.',
            advice:
              'What it holds does not match the file’s length and SHA-256 hash, or the file was announced differently before. Send it again.',
            action: SEND_AGAIN,
          };
    default:
      return undefined;
  }
}
