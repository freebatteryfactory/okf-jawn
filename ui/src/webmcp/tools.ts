/**
 * The five page tools an assistant in this browser may use through WebMCP (ruling B5; SPEC section
 * 9: "Page tools expose the selected source/revision/range, open a comparison/source, preview
 * names, and prepare a proposal. The small set uses the page's existing authenticated client
 * functions. No Verify/Approve page tools.").
 *
 * Each tool reads the page when it runs (`page()`: the Explorer shell as it last rendered): the
 * workspace, the head it shows, and the item the focused pane shows with the selection and the
 * revision its address names. What it read then is what it works on to the end: a revision is
 * always exact (the one the pane reads, or the head the page shows at that moment), never
 * "latest", and the page moving on while the tool waits for an answer moves nothing. Its own
 * requests are its own, so the page leaving an item does not cancel them.
 *
 * They act through the page's own client: the generated functions over the page's transport, the
 * session's cookie and CSRF header included, and the router to show what they open. Each input is
 * checked with a schema made of the generated ones, and the JSON Schema offered to the assistant is
 * made from that same schema. A failure rejects with plain words (`errors/failure.ts`).
 *
 * The four that read are marked `readOnlyHint`; opening a source or a comparison changes what the
 * person sees, never the workspace. `prepare_proposal` fills the Propose change form of the note on
 * screen (`proposal-form.ts`) and sends nothing: only the person can press Propose. A tool asked to
 * run once its registration has been withdrawn (the setting turned off, the workspace left) does
 * nothing and says so.
 */
import type { QueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { getItemQueryKey } from '../api/generated/@tanstack/react-query.gen';
import { getItem } from '../api/generated/sdk.gen';
import type { DiffResponse, ItemDocument, NamingRules, Revision } from '../api/generated/types.gen';
import {
  zItemId,
  zNamingRules,
  zRevision,
  zSelection,
  zWorkspacePath,
} from '../api/generated/zod.gen';
import {
  type Preview,
  previewRenames,
  rulesOptions,
  type StoredRules,
  startingRules,
  storedRulesOf,
} from '../features/conventions/rules';
import { classifyFailure, messageFor } from '../features/errors/failure';
import { allowed, archivedWords, missingWords, type Rights } from '../features/explorer/rights';
import type { ItemSelection } from '../features/explorer/search-params';
import type { Comparison } from '../features/history/ComparisonPage';
import { diffOptions } from '../features/history/history';
import { fillProposalForm } from './proposal-form';
import type { ModelContextTool } from './register';

/** The page as a tool finds it when it runs. */
export interface PageState {
  workspaceId: string;
  /** The revision the workspace shows. */
  head: Revision;
  /** The person's permissions here, and whether the workspace is archived. */
  rights: Rights;
  /** The item the focused pane shows, as its address names it; null when none is open. */
  open: {
    itemId: string;
    selection?: ItemSelection | undefined;
    revision?: Revision | undefined;
  } | null;
  client: QueryClient;
  /** Show an item in the focused pane at an exact revision, as the shell opens one. */
  openItem: (itemId: string, location: { sel?: ItemSelection; rev: Revision }) => void;
  /** Show the comparison page. */
  openComparison: (comparison: Comparison) => void;
}

const NoInput = z.object({}).strict();

const SourceInput = z
  .object({
    item_id: zItemId.describe('The item to open: its id, as get_selection or a link gives it.'),
    revision: zRevision
      .optional()
      .describe('The exact revision to open it at. Omitted: the revision the page shows now.'),
    selection: zSelection
      .optional()
      .describe(
        'A part of the item to show: lines, pages, cells or a section. Omitted: all of it.',
      ),
  })
  .strict();

const ComparisonInput = z
  .object({
    from: zRevision.describe('The earlier revision.'),
    to: zRevision
      .optional()
      .describe(
        'The later revision. Omitted: the revision of the item on screen, else the revision the page shows.',
      ),
    item_id: zItemId.optional().describe('Compare this item only. Omitted: the whole workspace.'),
  })
  .strict();

const PreviewInput = z
  .object({
    folder: z
      .union([z.literal(''), zWorkspacePath])
      .optional()
      .describe(
        'A folder: its items and those of every folder under it are previewed. Omitted or empty: the whole workspace.',
      ),
    rules: zNamingRules
      .optional()
      .describe('Naming rules to preview. Omitted: the rules saved in the workspace.'),
  })
  .strict();

const ProposalInput = z
  .object({
    title: z.string().trim().min(1).describe('The proposal’s title.'),
    description: z.string().optional().describe('Why the change is proposed.'),
    body: z.string().describe('The whole new text of the note, in Markdown.'),
  })
  .strict();

/** The JSON Schema an assistant is offered for an input: the one the tool checks it with. */
const schemaOf = (input: z.ZodType) => z.toJSONSchema(input, { io: 'input' });

function inputOf<T extends z.ZodType>(schema: T, input: unknown): z.output<T> {
  const checked = schema.safeParse(input ?? {});
  if (!checked.success) {
    throw new Error(`The input is not valid; nothing was done. ${z.prettifyError(checked.error)}`);
  }
  return checked.data;
}

/** Naming rules by the generated schema, as the generated type. */
const isNamingRules = (value: unknown): value is NamingRules =>
  zNamingRules.safeParse(value).success;

/** A failed request, in plain words. */
function failed(doing: string, thrown: unknown): Error {
  return new Error(`${doing}: ${messageFor(classifyFailure(thrown)).title}`);
}

/** An item at an exact revision: from the page's cache when the page has read it, else asked. */
async function itemAt(
  now: PageState,
  itemId: string,
  revision: Revision,
  signal: AbortSignal,
): Promise<ItemDocument> {
  const body = {
    workspace_id: now.workspaceId,
    item_id: itemId,
    at: { kind: 'revision' as const, revision },
  };
  const cached = now.client.getQueryData<ItemDocument>(getItemQueryKey({ body }));
  if (cached !== undefined) return cached;
  try {
    const { data } = await getItem({ body, signal, throwOnError: true });
    return data;
  } catch (thrown) {
    throw failed('Reading the item failed', thrown);
  }
}

const describeItem = (document: ItemDocument) => ({
  title: document.summary.title,
  path: document.summary.path,
  kind: document.summary.kind,
  type_name: document.summary.type_name,
});

/**
 * The five tools over `page`, for one registration: `offered` aborts when it is withdrawn, and
 * `workspaceId` is the workspace it was made for.
 */
export function pageTools(
  page: () => PageState,
  workspaceId: string,
  offered: AbortSignal,
): ModelContextTool[] {
  /** The page as it is now, while the tools are offered on it. */
  const pinned = (): PageState => {
    const now = page();
    if (offered.aborted || now.workspaceId !== workspaceId) {
      throw new Error(
        'WebMCP was turned off on this page, or the page left this workspace: nothing was done.',
      );
    }
    return now;
  };
  return [
    {
      name: 'get_selection',
      title: 'What the person has open',
      description:
        'Describe what the person has open on this page now: the workspace, the revision the page shows (head), and the item in the focused pane with the exact revision it is read at and the part of it selected (lines, pages, cells or a section). The revision is exact, never "latest"; pass it to later reads to see the same version.',
      inputSchema: schemaOf(NoInput),
      annotations: { readOnlyHint: true },
      execute: async (input, { signal }) => {
        const now = pinned();
        inputOf(NoInput, input);
        const shown = { workspace_id: now.workspaceId, head: now.head };
        if (now.open === null) return { ...shown, item: null };
        const { itemId, selection, revision } = now.open;
        const at = revision ?? now.head;
        const document = await itemAt(now, itemId, at, signal);
        pinned();
        return {
          ...shown,
          item: {
            item_id: itemId,
            revision: at,
            revision_named_by_link: revision !== undefined,
            selection: selection ?? { kind: 'all' },
            ...describeItem(document),
          },
        };
      },
    },
    {
      name: 'open_source',
      title: 'Open an item',
      description:
        'Open an item of this workspace for the person, in the focused pane of their Explorer, at an exact revision (the revision the page shows now when none is given), optionally at a part of it. It changes nothing in the workspace.',
      inputSchema: schemaOf(SourceInput),
      annotations: { readOnlyHint: true },
      execute: async (input, { signal }) => {
        const now = pinned();
        const { item_id: itemId, revision, selection } = inputOf(SourceInput, input);
        const at = revision ?? now.head;
        // Asked first: an item that is not there at that revision is not opened.
        const document = await itemAt(now, itemId, at, signal);
        const part = selection === undefined || selection.kind === 'all' ? undefined : selection;
        pinned().openItem(itemId, { rev: at, ...(part === undefined ? {} : { sel: part }) });
        return {
          item_id: itemId,
          revision: at,
          selection: part ?? { kind: 'all' },
          ...describeItem(document),
        };
      },
    },
    {
      name: 'open_comparison',
      title: 'Open a comparison',
      description:
        'Open for the person the changes between two exact revisions of this workspace, or of one item of it, and list the files that changed. "to" is, when omitted, the revision of the item on screen, else the revision the page shows. It changes nothing in the workspace.',
      inputSchema: schemaOf(ComparisonInput),
      annotations: { readOnlyHint: true },
      execute: async (input) => {
        const now = pinned();
        const { from, to, item_id: itemId } = inputOf(ComparisonInput, input);
        const until = to ?? now.open?.revision ?? now.head;
        const item = itemId ?? null;
        let result: DiffResponse;
        try {
          result = await now.client.fetchQuery(diffOptions(now.workspaceId, from, until, item));
        } catch (thrown) {
          throw failed('Comparing the two versions failed', thrown);
        }
        pinned().openComparison({ from, to: until, item });
        return {
          from: result.from,
          to: result.to,
          item_id: item,
          changed_files: result.changes.length,
          files: result.changes.map((change) => ({
            kind: change.kind,
            old_path: change.old_path ?? null,
            new_path: change.new_path ?? null,
          })),
          warnings: result.warnings,
        };
      },
    },
    {
      name: 'preview_names',
      title: 'Preview names',
      description:
        'Preview what naming rules would rename in this workspace at the revision the page shows now: the rules saved in the workspace unless rules are given, over the whole workspace or one folder and every folder under it. It renames nothing; only the person can apply a renaming.',
      inputSchema: schemaOf(PreviewInput),
      annotations: { readOnlyHint: true },
      execute: async (input) => {
        const now = pinned();
        const { folder = '', rules: given } = inputOf(PreviewInput, input);
        // Checked already: this only gives the rules their generated type.
        let rules = given !== undefined && isNamingRules(given) ? given : undefined;
        if (rules === undefined) {
          let saved: StoredRules;
          try {
            saved = storedRulesOf(
              await now.client.fetchQuery(rulesOptions(now.workspaceId, now.head)),
            );
          } catch (thrown) {
            throw failed('Reading the workspace’s naming rules failed', thrown);
          }
          rules = startingRules(saved);
          if (rules === undefined) {
            throw new Error(
              'The workspace’s naming rules file cannot be read as rules, so nothing was previewed. Give the rules to preview.',
            );
          }
        }
        let preview: Preview;
        try {
          preview = await previewRenames(now.workspaceId, now.head, rules, folder);
        } catch (thrown) {
          throw failed('Previewing the names failed', thrown);
        }
        pinned();
        return { ...preview.plan, folder: preview.folder, rules: preview.rules };
      },
    },
    {
      name: 'prepare_proposal',
      title: 'Prepare a proposal',
      description:
        'Fill in, for the person, the Propose change form of the note open on the page: a title, why, and the whole new text. It never sends the proposal: the person reads it, may change it, and only they can press Propose. A proposal is made on the version the workspace shows, so the note must be open at that version.',
      inputSchema: schemaOf(ProposalInput),
      execute: async (input) => {
        const now = pinned();
        const { title, description = '', body } = inputOf(ProposalInput, input);
        const can = allowed(now.rights, 'propose');
        if (can === 'no-permission') throw new Error(missingWords('Proposing a change', 'propose'));
        if (can === 'archived') throw new Error(archivedWords(now.rights.archivedAt ?? ''));
        if (now.open === null) {
          throw new Error('No item is open on the page: open the note first. Nothing was filled.');
        }
        const { itemId } = now.open;
        const at = now.open.revision ?? now.head;
        if (at !== now.head) {
          throw new Error(
            `The note on screen is shown at revision ${at}, not at the version the workspace shows (${now.head}); a proposal is made on that version. Open the note at it first. Nothing was filled.`,
          );
        }
        const outcome = fillProposalForm(now.workspaceId, itemId, {
          title,
          description,
          body,
          base: at,
        });
        if (outcome === 'absent') {
          throw new Error(
            'The item on screen offers no Propose change form: only a Note shown at the version the workspace shows has one, once it has loaded. Nothing was filled.',
          );
        }
        if (outcome === 'busy') {
          throw new Error(
            'The person is writing in the Propose change form of this note; it was left as it is.',
          );
        }
        if (outcome === 'stale') {
          throw new Error('The workspace shows another version now; nothing was filled.');
        }
        return {
          prepared: true,
          sent: false,
          workspace_id: now.workspaceId,
          item_id: itemId,
          base_revision: at,
          title,
          note: 'The Propose change form is filled in on the page for the person to read. Nothing was sent: only the person can press Propose.',
        };
      },
    },
  ];
}
