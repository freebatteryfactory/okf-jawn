/**
 * Turn whatever an operation threw into a plain-language failure with the action it allows.
 *
 * The generated client throws the server's `ApiError` body when the call fails. A request that went
 * unanswered for the client's bounded wait (`app/api-client.ts`) timed out; one the person stopped
 * says so, and that it may have been carried out all the same. Anything else (a
 * network error, a non-JSON answer) means the service was not reached. `not_implemented` is a
 * failure like any other: it reads "not available in this build" and is never shown as a success,
 * an empty result or a silent nothing. The message table is keyed by every `ErrorCode`, so a new
 * code fails type checking until it has words.
 *
 * A typed detail (ruling 5) says more than its code, and its words say so with the action it
 * allows: `stale_base` names the head the request was compared with, and offers to show that
 * version (`show-head`); `workspace_archived` says when, and that nothing can be changed until it is
 * unarchived; `upload_mismatch` says what the slot holds against what was announced. A refusal's
 * `field` is a JSON Pointer into the request (`/base_revision`, `/rules/0/glob`).
 */
import type { z } from 'zod';
import type { ErrorCode, Revision } from '../../api/generated/types.gen';
import { zApiError } from '../../api/generated/zod.gen';
import { RequestStopped, RequestTimedOut } from '../../app/api-client';
import { formatInstant } from '../../lib/instant';

export type ApiFailure = z.infer<typeof zApiError>;

export type Failure =
  | { kind: 'api'; error: ApiFailure }
  | { kind: 'timed_out'; seconds: number }
  | { kind: 'stopped' }
  | { kind: 'unreachable'; detail: string };

/** What the person can do about a failure. */
export type FailureAction =
  | { kind: 'retry'; label: string }
  | { kind: 'home'; label: string }
  /** Show the workspace at `head`, the version the service compared the request with. */
  | { kind: 'show-head'; label: string; head: Revision }
  | { kind: 'none' };

export interface FailureMessage {
  /** One line that says what happened. */
  title: string;
  /** What the person can do next, in words. */
  advice: string;
  action: FailureAction;
}

const RETRY: FailureAction = { kind: 'retry', label: 'Try again' };

const MESSAGES: Record<ErrorCode, FailureMessage> = {
  invalid_input: {
    title: 'The request was not valid.',
    advice: 'Check the address or the values you entered, then try again.',
    action: { kind: 'none' },
  },
  unauthenticated: {
    title: 'You are not signed in.',
    advice:
      'Sign in again. On this computer, open the launch link the server printed when it started; then try again.',
    action: { kind: 'retry', label: 'Try again after signing in' },
  },
  forbidden: {
    title: 'You do not have permission to do this.',
    advice: 'Ask the owner of this workspace to give you access.',
    action: { kind: 'none' },
  },
  not_found: {
    title: 'This could not be found.',
    advice: 'It may have been moved or deleted, or the link may be wrong.',
    action: { kind: 'home', label: 'Go to your workspaces' },
  },
  conflict: {
    title: 'This changed since it was loaded.',
    advice: 'Reload to see the current version.',
    action: { kind: 'retry', label: 'Reload' },
  },
  already_issued: {
    title: 'This was already issued.',
    advice: 'A secret is shown only once. Issue a new one if it was lost.',
    action: { kind: 'none' },
  },
  in_progress: {
    title: 'This is still in progress.',
    advice: 'Wait a moment, then try again.',
    action: RETRY,
  },
  too_large: {
    title: 'This is too large to show at once.',
    advice: 'Choose a smaller part, such as one section or a range of pages.',
    action: { kind: 'none' },
  },
  unsupported: {
    title: 'This is not supported for this item.',
    advice: 'Try another way of viewing it.',
    action: { kind: 'none' },
  },
  unavailable: {
    title: 'The service is temporarily unavailable.',
    advice: 'Try again in a moment.',
    action: RETRY,
  },
  cancelled: {
    title: 'The operation was cancelled.',
    advice: 'Try again if you still need it.',
    action: RETRY,
  },
  internal: {
    title: 'Something went wrong in the service.',
    advice: 'Try again. If it keeps happening, report the reference shown here.',
    action: RETRY,
  },
  not_implemented: {
    title: 'Not available in this build.',
    advice: 'This part of okf-jawn has not been built yet. Nothing was done.',
    action: { kind: 'none' },
  },
};

const UNREACHABLE: FailureMessage = {
  title: 'The workspace service could not be reached.',
  advice: 'Check that the okf-jawn server is running, then try again.',
  action: RETRY,
};

/** An error body whose typed detail this client cannot read is still the server's error. */
const zApiErrorWithoutDetail = zApiError.omit({ detail: true });

export function classifyFailure(thrown: unknown): Failure {
  if (thrown instanceof RequestTimedOut)
    return { kind: 'timed_out', seconds: thrown.waitedMs / 1000 };
  if (thrown instanceof RequestStopped) return { kind: 'stopped' };
  const parsed = zApiError.safeParse(thrown);
  if (parsed.success) return { kind: 'api', error: parsed.data };
  const undetailed = zApiErrorWithoutDetail.safeParse(thrown);
  if (undetailed.success) return { kind: 'api', error: { ...undetailed.data, detail: null } };
  if (thrown instanceof Error) return { kind: 'unreachable', detail: thrown.message };
  return { kind: 'unreachable', detail: typeof thrown === 'string' ? thrown : '' };
}

export function messageFor(failure: Failure): FailureMessage {
  if (failure.kind === 'unreachable') return UNREACHABLE;
  if (failure.kind === 'timed_out') {
    return {
      title: 'The workspace service did not answer in time.',
      advice: `No answer came within ${failure.seconds} seconds. Try again.`,
      action: RETRY,
    };
  }
  if (failure.kind === 'stopped') {
    return {
      title: 'You stopped this before the service answered.',
      advice: 'Whether the service carried it out is not known. Try again to make sure.',
      action: RETRY,
    };
  }
  const base = MESSAGES[failure.error.code];
  const detail = failure.error.detail;
  if (detail?.kind === 'in_progress') {
    return { ...base, advice: `Try again in ${detail.retry_after} seconds.` };
  }
  if (detail?.kind === 'stale_base') {
    return {
      title: 'The workspace changed since the version this was made on.',
      advice: `It now holds revision ${detail.head.slice(0, 10)}. Show that version, check what you did, then do it again.`,
      action: {
        kind: 'show-head',
        label: `Show revision ${detail.head.slice(0, 10)}`,
        head: detail.head,
      },
    };
  }
  if (detail?.kind === 'workspace_archived') {
    const when = formatInstant(detail.archived_at) ?? detail.archived_at;
    return {
      title: 'This workspace is archived.',
      advice: `It was archived on ${when}: nothing in it can be changed until it is unarchived. It can still be read.`,
      action: { kind: 'none' },
    };
  }
  if (detail?.kind === 'upload_mismatch') {
    return {
      title: 'What the service holds does not match this file.',
      advice: mismatchWords(detail),
      action: { kind: 'none' },
    };
  }
  if (detail?.kind === 'invalidated') {
    return {
      ...base,
      advice:
        detail.replacement === null || detail.replacement === undefined
          ? 'This content was removed by a purge.'
          : `This revision was rewritten by a purge; the replacement is ${detail.replacement}.`,
    };
  }
  return base;
}

/** What an upload's slot holds against what was announced (`upload_mismatch`), in words. */
export function mismatchWords(detail: {
  expected_bytes: string;
  received_bytes: string;
  digest_matches?: boolean | null | undefined;
}): string {
  if (detail.expected_bytes !== detail.received_bytes) {
    return `It was announced with ${detail.expected_bytes} bytes and holds ${detail.received_bytes}.`;
  }
  return detail.digest_matches === false
    ? `It holds the ${detail.expected_bytes} bytes announced, but they do not have the file's SHA-256 hash.`
    : `It holds the ${detail.expected_bytes} bytes announced.`;
}

/** Codes a repeat of the same request can cure without anyone changing anything. */
const TRANSIENT: ReadonlySet<ErrorCode> = new Set(['unavailable', 'in_progress']);

/**
 * Query retry policy: retry once only what may pass by itself; never a refusal or a gap. A request
 * that timed out is not repeated by itself: the person has waited the bounded wait once already.
 */
export function shouldRetry(failureCount: number, thrown: unknown): boolean {
  if (failureCount >= 1) return false;
  const failure = classifyFailure(thrown);
  if (failure.kind === 'timed_out' || failure.kind === 'stopped') return false;
  return failure.kind === 'unreachable' || TRANSIENT.has(failure.error.code);
}
