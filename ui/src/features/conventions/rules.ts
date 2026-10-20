/**
 * The naming rules of a workspace (SPEC section 6): reading them, saving them, a preview of what
 * they would rename, and applying a preview.
 *
 * The rules are read at the head the workspace shows (`get_rules` at that revision), so they are
 * the rules of the version on screen and are read afresh under a new head. A version holds rules,
 * none at all, or a stored file this build cannot read as rules (`StoredRules`); the last is
 * shown with its text and its problems for repair, never as "no rules". The schema the form is
 * built from is the service's own (`get_form_schema`, name `naming-rules`).
 *
 * A preview writes nothing. `preview_names` takes a selection (ruling 10): the whole workspace, or
 * a folder with every folder under it. The service reads the items itself, in one request, and
 * answers how many it considered, how many would keep their names, and how many archived items it
 * left out (archived items are never renamed); the plan's entries are the renames alone.
 *
 * Applying sends back exactly the plan the service answered and the rules it was previewed with;
 * the service checks both again (the plan's base revision and the digest of the rules). The
 * revision it returns becomes the head the shell shows (`adoptHead`), and the head checks run
 * again; the renamed items keep their identity, so their tabs go on reading them. A draft the
 * person has of a renamed item follows the renaming like any other change of their own
 * (`followOwnChange`): the drafts this page knows of, from its sessions and the listed drafts.
 * The renaming is done when the service has answered it; the drafts follow after that, and how
 * they then stand is a second answer (`Renamed.drafts`), said when it comes. A renaming whose
 * answer is not adopted as the head shown leaves the drafts this page holds behind, on the older
 * version, and says so.
 * Saving the rules is a commit too, and is told to the cache the same way.
 */
import { type QueryClient, queryOptions } from '@tanstack/react-query';
import {
  getFormSchemaOptions,
  getRulesOptions,
} from '../../api/generated/@tanstack/react-query.gen';
import { applyNames, previewNames, setRules } from '../../api/generated/sdk.gen';
import type {
  GetRulesResponse,
  NameSelection,
  NamingRules,
  RenamePlan,
  Revision,
  Warning,
} from '../../api/generated/types.gen';
import { listedDraft } from '../drafts/drafts-list';
import { type DraftAfterChange, draftLeftBehind, followOwnChange } from '../drafts/saving';
import { sessionKey, useDraftSessions } from '../drafts/sessions';
import { adoptHeadFrom, checkHeadsAgain, type HeadChange } from '../explorer/head';
import type { Attempt } from '../tree/changes';

/** The name of the naming rules' schema among the service's form schemas. */
export const RULES_SCHEMA = 'naming-rules';

/** The rules a workspace with none saved starts from: none at all, in this build's format. */
export const NO_RULES: NamingRules = { schema_version: 1, rules: [] };

/**
 * What a version holds as its naming rules (`get_rules`, ruling 11): rules; no rules saved at
 * all; or a stored file that cannot be read as this build's rules, with its text and what is
 * wrong with it. A file that cannot be read is never taken for "no rules": the page shows it, so
 * that the person can repair it.
 */
export type StoredRules =
  | { kind: 'rules'; rules: NamingRules }
  | { kind: 'none' }
  | { kind: 'unreadable'; text: string; problems: Warning[] };

/**
 * The answer of `get_rules` as what the version holds. A stored file that cannot be read wins
 * over rules beside it (the contract never sends both): such rules are not what the file says.
 * Defined once, so the query's selection is not run again at every drawing.
 */
export function storedRulesOf(answer: GetRulesResponse): StoredRules {
  if (answer.unreadable !== undefined && answer.unreadable !== null) {
    return { kind: 'unreadable', ...answer.unreadable };
  }
  if (answer.rules !== undefined && answer.rules !== null) {
    return { kind: 'rules', rules: answer.rules };
  }
  return { kind: 'none' };
}

/** The rules edits start from in what a version holds; none for a file that cannot be read. */
export function startingRules(stored: StoredRules): NamingRules | undefined {
  if (stored.kind === 'rules') return stored.rules;
  return stored.kind === 'none' ? NO_RULES : undefined;
}

/** Whether two versions hold the same rules: rules in any key order, or the same stored text. */
export function sameStored(a: StoredRules, b: StoredRules): boolean {
  if (a.kind === 'rules' && b.kind === 'rules') return sameRules(a.rules, b.rules);
  if (a.kind === 'unreadable' && b.kind === 'unreadable') return a.text === b.text;
  return a.kind === b.kind;
}

/** What the version the workspace shows holds as its naming rules. */
export function rulesOptions(workspaceId: string, head: Revision) {
  return queryOptions({
    ...getRulesOptions({
      body: { workspace_id: workspaceId, at: { kind: 'revision', revision: head } },
    }),
    select: storedRulesOf,
  });
}

/**
 * Where a problem the service found in a stored rules file is (its `location`): a line and a
 * column ("line 2, column 7", as the YAML reader gives one); a JSON Pointer into the file
 * (`/rules/0/glob`), which the page places at a line and column by reading the text; the whole
 * file (no location); or other words, shown as they are.
 */
export type ProblemPlace =
  | { kind: 'line'; line: number; column: number }
  | { kind: 'pointer'; pointer: string }
  | { kind: 'file' }
  | { kind: 'words'; words: string };

export function problemPlace(location: string | null | undefined): ProblemPlace {
  if (location === undefined || location === null || location.trim() === '') {
    return { kind: 'file' };
  }
  const at = /^\s*line (\d+)(?:,\s*column (\d+))?\s*$/i.exec(location);
  if (at !== null) {
    return { kind: 'line', line: Number(at[1] ?? '1'), column: Number(at[2] ?? '1') };
  }
  if (location.startsWith('/')) return { kind: 'pointer', pointer: location };
  return { kind: 'words', words: location };
}

/** The schema the rules form is built from; it does not change while the page is open. */
export function rulesSchemaOptions() {
  return {
    ...getFormSchemaOptions({ path: { name: RULES_SCHEMA } }),
    staleTime: Number.POSITIVE_INFINITY,
  };
}

/** The same rules, whatever order their keys come in. */
export function sameRules(a: unknown, b: unknown): boolean {
  return JSON.stringify(ordered(a)) === JSON.stringify(ordered(b));
}

function ordered(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(ordered);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([key, entry]) => [key, ordered(entry)]),
    );
  }
  return value;
}

/**
 * `after` with its keys in the order `before` has them: a key both have stays where it was, a key
 * only `after` has goes last, and lists are matched place by place. The form library hands a
 * value back with its keys in the order of its schema; this keeps the order the person's YAML had,
 * so a form edit changes only the keys it changes.
 */
export function keepOrder(before: unknown, after: unknown): unknown {
  if (Array.isArray(after)) {
    const was = Array.isArray(before) ? before : [];
    return after.map((entry, place) => keepOrder(was.at(place), entry));
  }
  if (after === null || typeof after !== 'object') return after;
  const now = new Map(Object.entries(after));
  const was =
    before !== null && typeof before === 'object' && !Array.isArray(before)
      ? new Map(Object.entries(before))
      : new Map<string, unknown>();
  const keys = [
    ...[...was.keys()].filter((key) => now.has(key)),
    ...[...now.keys()].filter((key) => !was.has(key)),
  ];
  return Object.fromEntries(keys.map((key) => [key, keepOrder(was.get(key), now.get(key))]));
}

/** Save the rules as the workspace's naming rules: one commit on the head shown. */
export async function saveRules(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<NamingRules>,
): Promise<HeadChange> {
  const { data } = await setRules({
    body: {
      workspace_id: workspaceId,
      base_revision: attempt.base,
      idempotency_key: attempt.key,
      rules: attempt.values,
    },
    throwOnError: true,
  });
  const { previousHead, adopted } = adoptHeadFrom(client, workspaceId, data.revision, attempt.base);
  checkHeadsAgain(client, workspaceId);
  return { revision: data.revision, previousHead, itemIds: [], adopted };
}

/** A preview as the person saw it: the plan, and what it was made of. */
export interface Preview {
  plan: RenamePlan;
  /** The rules it was previewed with: an apply sends these, not what the editor holds later. */
  rules: NamingRules;
  /** The folder whose items, and those of every folder under it, were looked at; '' is all. */
  folder: string;
}

/** What a preview of `folder` asks for: the whole workspace, or the folder and those under it. */
export function selectionOf(folder: string): NameSelection {
  return folder === '' ? { kind: 'workspace' } : { kind: 'folder', folder, recursive: true };
}

/** What these rules would rename among the items of `folder`, at `revision`. Writes nothing. */
export async function previewRenames(
  workspaceId: string,
  revision: Revision,
  rules: NamingRules,
  folder: string,
): Promise<Preview> {
  const { data } = await previewNames({
    body: {
      workspace_id: workspaceId,
      at: { kind: 'revision', revision },
      selection: selectionOf(folder),
      rules,
    },
    throwOnError: true,
  });
  return { plan: data, rules, folder };
}

/**
 * A renaming, done, and how each of the person's drafts of the renamed items stands after it:
 * known later, once they have followed. `drafts` never rejects.
 */
export type Renamed = HeadChange & { drafts: Promise<DraftAfterChange[]> };

/** Apply a preview: exactly the plan and the rules the person saw. */
export async function applyPreview(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<Preview>,
): Promise<Renamed> {
  const { plan, rules } = attempt.values;
  const { data } = await applyNames({
    body: { workspace_id: workspaceId, idempotency_key: attempt.key, plan, rules },
    throwOnError: true,
  });
  // The request began on the version the preview was made on.
  const { previousHead, adopted } = adoptHeadFrom(
    client,
    workspaceId,
    data.revision,
    plan.base_revision,
  );
  checkHeadsAgain(client, workspaceId);
  // The person's drafts of the items that were renamed follow their own change, when the page
  // shows it; they are started here and not waited for.
  const drafted = plan.entries
    .filter((entry) => entry.from !== entry.to)
    .map((entry) => entry.item_id)
    .filter(
      (itemId) =>
        useDraftSessions.getState().sessions[sessionKey(workspaceId, itemId)] !== undefined ||
        listedDraft(client, workspaceId, itemId) !== undefined,
    );
  const drafts = Promise.all(
    drafted.map((itemId) =>
      adopted
        ? followOwnChange(client, workspaceId, itemId, plan.base_revision, data.revision, {
            status: false,
          })
        : Promise.resolve(draftLeftBehind(workspaceId, itemId)),
    ),
  );
  return {
    revision: data.revision,
    previousHead,
    itemIds: plan.entries.map((entry) => entry.item_id),
    adopted,
    drafts,
  };
}
