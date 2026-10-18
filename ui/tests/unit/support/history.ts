/**
 * Contract-valid answers for history: versions (`log_items`), comparisons (`diff_items`), Who
 * (`blame_item`) and Rewind (`restore_items`), over the workspace of `workspace.tsx`.
 */
import type { Commit, DiffResponse, FileChange } from '../../../src/api/generated/types.gen';
import type { Json } from './api';
import { HEAD, OLD, RECEIPT } from './workspace';

export const LOG = '/api/history/log-items';
export const DIFF = '/api/history/diff-items';
export const BLAME = '/api/history/blame-item';
export const RESTORE = '/api/history/restore-items';

/** The version before OLD, and the first one. */
export const OLDER = 'e'.repeat(40);
export const FIRST = 'f'.repeat(40);
/** The version a Rewind makes. */
export const REWOUND = '1'.repeat(40);

export function commitOf(
  revision: string,
  message: string,
  parents: string[],
  committedAt = '2026-01-01T09:00:00.000Z',
): Commit {
  return { revision, parents, message, author: 'ada', committed_at: committedAt };
}

/** The workspace's versions, newest first: HEAD, OLD, OLDER, FIRST. */
export const VERSIONS: Commit[] = [
  commitOf(HEAD, 'Publish the budget', [OLD], '2026-01-01T12:00:00.000Z'),
  commitOf(OLD, 'Draft the budget', [OLDER], '2026-01-01T11:00:00.000Z'),
  commitOf(OLDER, 'Add the meeting notes', [FIRST], '2026-01-01T10:00:00.000Z'),
  commitOf(FIRST, 'Start the workspace', [], '2026-01-01T09:00:00.000Z'),
];

/** A patch as Git writes it: its header, then a hunk. */
export function patchOf(path: string, removed: string, added: string): string {
  return [
    `diff --git a/${path} b/${path}`,
    'index 1111111..2222222 100644',
    `--- a/${path}`,
    `+++ b/${path}`,
    '@@ -3,2 +3,2 @@',
    ' # Meeting notes',
    `-${removed}`,
    `+${added}`,
    '',
  ].join('\n');
}

export function changeOf(path: string, removed: string, added: string): FileChange {
  return {
    old_path: path,
    new_path: path,
    kind: 'modified',
    binary: false,
    patch: patchOf(path, removed, added),
  };
}

/** A comparison answer for a `diff_items` request: its two versions, and `changes`. */
export function diffOf(request: Json, changes: FileChange[]): DiffResponse {
  return { from: String(request.from), to: String(request.to), changes, warnings: [] };
}

/** A Rewind's answer. */
export function rewound(revision = REWOUND): Json {
  return { revision, receipt_id: RECEIPT, warnings: [] };
}
