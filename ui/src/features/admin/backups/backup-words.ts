/**
 * The words of a workspace's backups and restore for the failures the general words of
 * `errors/failure.ts` would say wrongly (ruling C2: a refusal the service answers is said in plain
 * words with what it allows):
 * - a `forbidden` of a backup or a restore: the human-session rule and an administrator's rights,
 *   the two things the service checks (`HUMAN_SESSION_ONLY`);
 * - a restore's `conflict`, by the field the service names (`crates/core`, `restore_workspace`):
 *   `/workspace_id` the workspace is not blank, `/upload_id` the upload is not complete or another
 *   restore took it, `/sha256` the upload does not have the SHA-256 the restore names. A conflict
 *   that names none of them keeps the general words;
 * - an archive's upload: the import's words (`imports/upload-words.ts`), except where they speak of
 *   the import's list of files, which this page has not.
 * Every other failure keeps its general words.
 */
import { classifyFailure, type FailureMessage } from '../../errors/failure';
import type { UploadStep } from '../../imports/upload-list';
import { uploadWords } from '../../imports/upload-words';
import { FileChanged, slotIsAhead } from '../../imports/uploading';

const NONE = { kind: 'none' } as const;

/** The service's refusal, when it answered one. */
function refusal(error: unknown) {
  const failure = classifyFailure(error);
  return failure.kind === 'api' ? failure.error : null;
}

/** A refused `backup_workspace`. */
export function backupWords(error: unknown): FailureMessage | undefined {
  if (refusal(error)?.code !== 'forbidden') return undefined;
  return {
    title: 'The service did not allow this backup.',
    advice:
      'A backup is made only for an administrator of this workspace, in their own browser session: not for an agent, a connector or a service.',
    action: NONE,
  };
}

/** A refused `restore_workspace`. */
export function restoreWords(error: unknown): FailureMessage | undefined {
  const refused = refusal(error);
  if (refused?.code === 'forbidden') {
    return {
      title: 'The service did not allow this restore.',
      advice:
        'A restore is done only for an administrator of this workspace, in their own browser session: not for an agent, a connector or a service.',
      action: NONE,
    };
  }
  if (refused?.code !== 'conflict') return undefined;
  switch (refused.field) {
    case '/workspace_id':
      return {
        title: 'This workspace is not blank.',
        advice:
          'A restore fills only a workspace that holds nothing yet: no items, no drafts and no history since it was created. Create a new workspace from the installation’s Administration, and restore into it.',
        action: NONE,
      };
    case '/upload_id':
      return {
        title: 'This upload can no longer be restored.',
        advice:
          'It is not complete, or another restore has taken it. Choose the archive and upload it again.',
        action: NONE,
      };
    case '/sha256':
      return {
        title: 'The upload does not have the SHA-256 this restore names.',
        advice: 'Choose the archive and upload it again.',
        action: NONE,
      };
    default:
      return undefined;
  }
}

/** Where a try of the archive's upload failed. */
export interface ArchiveUploadFailure {
  error: unknown;
  step: UploadStep;
  /** Bytes had been sent to the slot before this try began. */
  resumed: boolean;
}

/** The failed upload of an archive. */
export function archiveUploadWords(failure: ArchiveUploadFailure): FailureMessage | undefined {
  if (failure.error instanceof FileChanged) {
    return {
      title: 'This file changed since you chose it.',
      advice: `It was chosen with ${failure.error.listed} bytes and now has ${failure.error.read}. Nothing of it was sent. Choose it again.`,
      action: NONE,
    };
  }
  // A slot that holds more than it said is not an archive too large: the import's words say so.
  if (refusal(failure.error)?.code === 'too_large' && !slotIsAhead(failure)) {
    return {
      title: 'This archive is larger than the service accepts.',
      advice: 'Nothing was kept of it.',
      action: NONE,
    };
  }
  return uploadWords(failure);
}
