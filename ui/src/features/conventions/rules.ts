/**
 * The naming rules of a workspace (SPEC section 6): reading them, saving them, a preview of what
 * they would rename, and applying a preview.
 *
 * The rules are read at the head the workspace shows (`get_rules` at that revision), so they are
 * the rules of the version on screen and are read afresh under a new head. The schema the form is
 * built from is the service's own (`get_form_schema`, name `naming-rules`).
 *
 * A preview writes nothing. `preview_names` takes the items to look at by id, so the items of the
 * chosen folder and of every folder under it are listed first, page by page, at the same
 * revision: one `list_items` request for each 200 entries of each folder (a workspace of 10,000
 * items in 100 folders is about 150 requests, one after another). Pages already listed for the
 * tree are reused. A folder that fails to list fails the preview: a preview of some of the items
 * is not shown as a preview of all of them.
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
  ListItemsResponse,
  NamingRules,
  RenamePlan,
  Revision,
} from '../../api/generated/types.gen';
import { listedDraft } from '../drafts/drafts-list';
import { type DraftAfterChange, draftLeftBehind, followOwnChange } from '../drafts/saving';
import { sessionKey, useDraftSessions } from '../drafts/sessions';
import { listingPageOptions } from '../explorer/folder-listing';
import { adoptHeadFrom, checkHeadsAgain, type HeadChange } from '../explorer/head';
import type { Attempt } from '../tree/changes';

/** The name of the naming rules' schema among the service's form schemas. */
export const RULES_SCHEMA = 'naming-rules';

/**
 * The saved rules at the revision the workspace shows. `get_rules` answers them inside its
 * answer, beside the revision read and a stored file that cannot be read as rules; this page
 * reads the rules alone, and has none when the answer holds none.
 */
export function rulesOptions(workspaceId: string, head: Revision) {
  return queryOptions({
    ...getRulesOptions({
      body: { workspace_id: workspaceId, at: { kind: 'revision', revision: head } },
    }),
    select: (answer: GetRulesResponse): NamingRules | undefined => answer.rules ?? undefined,
  });
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

/**
 * The ids of every item in `folder` ('' is the whole workspace) and in the folders under it, as
 * listed at `revision`. Rejects with what a listing threw.
 */
export async function itemsUnder(
  client: QueryClient,
  workspaceId: string,
  revision: Revision,
  folder: string,
): Promise<string[]> {
  const ids: string[] = [];
  const waiting = [folder];
  for (let next = waiting.pop(); next !== undefined; next = waiting.pop()) {
    let cursor: string | null = null;
    do {
      const page: ListItemsResponse = await client.fetchQuery({
        ...listingPageOptions(workspaceId, revision, next, cursor),
        staleTime: Number.POSITIVE_INFINITY,
      });
      for (const item of page.items) ids.push(item.id);
      waiting.push(...page.folders);
      cursor = page.next_cursor ?? null;
    } while (cursor !== null);
  }
  return ids;
}

/** A preview as the person saw it: the plan, and what it was made of. */
export interface Preview {
  plan: RenamePlan;
  /** The rules it was previewed with: an apply sends these, not what the editor holds later. */
  rules: NamingRules;
  /** The folder whose items were looked at; '' is the whole workspace. */
  folder: string;
  /** How many items were looked at. */
  looked: number;
}

/** Thrown by a preview of a folder that holds no item: nothing is asked of `preview_names`. */
export class NothingToPreview extends Error {
  constructor() {
    super('There is no item to preview');
    this.name = 'NothingToPreview';
  }
}

/** What these rules would rename among the items of `folder`, at `revision`. Writes nothing. */
export async function previewRenames(
  client: QueryClient,
  workspaceId: string,
  revision: Revision,
  rules: NamingRules,
  folder: string,
): Promise<Preview> {
  const itemIds = await itemsUnder(client, workspaceId, revision, folder);
  if (itemIds.length === 0) throw new NothingToPreview();
  const { data } = await previewNames({
    body: {
      workspace_id: workspaceId,
      at: { kind: 'revision', revision },
      selection: { kind: 'items', item_ids: itemIds },
      rules,
    },
    throwOnError: true,
  });
  return { plan: data, rules, folder, looked: itemIds.length };
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
