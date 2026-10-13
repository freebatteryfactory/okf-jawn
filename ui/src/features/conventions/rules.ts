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
 * again; the renamed items keep their identity, so their tabs go on reading them. Saving the
 * rules is a commit too, and is told to the cache the same way.
 */
import type { QueryClient } from '@tanstack/react-query';
import {
  getFormSchemaOptions,
  getRulesOptions,
} from '../../api/generated/@tanstack/react-query.gen';
import { applyNames, previewNames, setRules } from '../../api/generated/sdk.gen';
import type {
  ListItemsResponse,
  NamingRules,
  RenamePlan,
  Revision,
} from '../../api/generated/types.gen';
import { listingPageOptions } from '../explorer/folder-listing';
import { adoptHead, checkHeadsAgain, type HeadChange } from '../explorer/head';
import type { Attempt } from '../tree/changes';

/** The name of the naming rules' schema among the service's form schemas. */
export const RULES_SCHEMA = 'naming-rules';

/** The saved rules at the revision the workspace shows. */
export function rulesOptions(workspaceId: string, head: Revision) {
  return getRulesOptions({
    body: { workspace_id: workspaceId, at: { kind: 'revision', revision: head } },
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
  const previousHead = adoptHead(client, workspaceId, data.revision);
  checkHeadsAgain(client, workspaceId);
  return { revision: data.revision, previousHead, itemIds: [] };
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
      item_ids: itemIds,
      rules,
    },
    throwOnError: true,
  });
  return { plan: data, rules, folder, looked: itemIds.length };
}

/** Apply a preview: exactly the plan and the rules the person saw. */
export async function applyPreview(
  client: QueryClient,
  workspaceId: string,
  attempt: Attempt<Preview>,
): Promise<HeadChange> {
  const { plan, rules } = attempt.values;
  const { data } = await applyNames({
    body: { workspace_id: workspaceId, idempotency_key: attempt.key, plan, rules },
    throwOnError: true,
  });
  const previousHead = adoptHead(client, workspaceId, data.revision);
  checkHeadsAgain(client, workspaceId);
  return {
    revision: data.revision,
    previousHead,
    itemIds: plan.entries.map((entry) => entry.item_id),
  };
}
