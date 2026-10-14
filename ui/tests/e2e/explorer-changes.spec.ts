/**
 * Changing the tree and the naming rules from the keyboard, in Chromium and WebKit: a new note and
 * a new folder; an open item renamed with its tab kept, then given another status word; a delete
 * after its question; a naming preview, then its apply; and one refused change shown and retried.
 * Every journey ends with keyboard focus on a sensible element and zero axe violations, and saves
 * a screenshot of its screen under `.artifacts/ui/` for the owner.
 *
 * The service is played by `page.route` (no server exists in this lane): it keeps the tree and the
 * rules of every revision it made, answers a listing or a read at a revision with what that
 * revision held, and refuses a write on another base than its head.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  type Answers,
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  markdown,
  NOTE,
  NOW,
  note,
  ok,
  PLAN,
  plan,
  type Reply,
  readAnswer,
  routeApi,
  SOURCE,
  shot,
  source,
  summary,
  useExplorerServer,
  WS,
  writableWorkspace,
} from './support/explorer';

type Body = Record<string, unknown>;
type Item = ReturnType<typeof summary>;

const NEW = '66666666-6666-4666-8666-666666666666';
const RECEIPT = '55555555-5555-4555-8555-555555555555';
const MADE = ['c', 'd', 'e', 'f'].map((digit) => digit.repeat(40));
const schemaFile = fileURLToPath(
  new URL('../../../api/forms/naming-rules.schema.json', import.meta.url),
);
const rule = {
  glob: '**/*.md',
  case: 'kebab',
  separator: '-',
  prefix: '',
  date_order: 'ymd',
  date_format: '%Y-%m-%d',
  strip_version_suffix: true,
  extension_policy: 'preserve',
  alias_preservation: true,
  collision: 'reject',
};

const folderOf = (path: string) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
const text = (body: Body, key: string) => (typeof body[key] === 'string' ? String(body[key]) : '');

interface Service {
  answers: Answers;
  /** Every write request, in the order they came. */
  sent: Array<{ path: string; body: Body }>;
  /** Refuse the next `count` writes as unavailable. */
  failNext: (count: number) => void;
  /** Someone else saves other naming rules: the head moves to the revision that holds them. */
  othersSave: (saved: unknown) => string;
  /** Hold every answer about the workspace itself until the function given back is called. */
  holdWorkspace: () => () => void;
}

async function service(): Promise<Service> {
  const schema: unknown = JSON.parse(await readFile(schemaFile, 'utf8'));
  const trees = new Map<string, { items: Item[]; folders: string[] }>([
    [HEAD, { items: [note, plan, source], folders: ['notes'] }],
  ]);
  const rules = new Map<string, unknown>([[HEAD, { schema_version: 1, rules: [rule] }]]);
  const state = { head: HEAD, made: 0, failing: 0 };
  const held: { wait: Promise<void> | null } = { wait: null };
  const sent: Service['sent'] = [];
  const at = (body: Body) => {
    const asked = body.at as { revision?: string } | undefined;
    return asked?.revision ?? state.head;
  };
  const treeAt = (revision: string) => trees.get(revision) ?? { items: [], folders: [] };
  const commit = (
    change: (tree: { items: Item[]; folders: string[] }) => {
      items: Item[];
      folders: string[];
    },
  ) => {
    const next = MADE[state.made] ?? '';
    state.made += 1;
    trees.set(next, change(treeAt(state.head)));
    rules.set(next, rules.get(state.head));
    state.head = next;
    return next;
  };
  const done = (revision: string): Reply => ok({ revision, receipt_id: RECEIPT, warnings: [] });
  /** A write: recorded, refused while the service is told to fail or the base is not its head. */
  const write =
    (path: string, run: (body: Body) => Reply) =>
    (body: Body): Reply => {
      sent.push({ path, body });
      if (state.failing > 0) {
        state.failing -= 1;
        return { status: 503, body: { code: 'unavailable', message: 'the store is busy' } };
      }
      const base = typeof body.base_revision === 'string' ? body.base_revision : state.head;
      if (base !== state.head) {
        return { status: 409, body: { code: 'conflict', message: 'the workspace head moved' } };
      }
      return run(body);
    };
  const found = (body: Body) => {
    const revision = at(body);
    const item = treeAt(revision).items.find((each) => each.id === body.item_id);
    return item === undefined ? null : { ...item, revision };
  };
  const missing: Reply = {
    status: 404,
    body: { code: 'not_found', message: 'no such item at this revision' },
  };
  const document = (item: Item, body: string) => ({
    summary: item,
    body,
    content_digest: 'c'.repeat(64),
    properties: { type: item.type_name, okf_jawn: { id: item.id } },
  });
  const answers: Answers = {
    '/api/workspaces/open-workspace': async () => {
      if (held.wait !== null) await held.wait;
      return ok({ ...writableWorkspace, head: state.head });
    },
    '/api/items/list-items': (body) => {
      const revision = at(body);
      const tree = treeAt(revision);
      const folder = text(body, 'folder');
      return ok({
        folders: tree.folders.filter((each) => folderOf(each) === folder),
        items: tree.items
          .filter((item) => folderOf(item.path) === folder && !item.archived)
          .map((item) => ({ ...item, revision })),
        revision,
        next_cursor: null,
      });
    },
    '/api/items/get-item': (body) => {
      const item = found(body);
      return item === null ? missing : ok(document(item, markdown));
    },
    '/api/reads/read-item': (body) => {
      const item = found(body);
      return item === null
        ? missing
        : ok({
            ...readAnswer(item, body),
            source: { ...readAnswer(item, body).source, revision: item.revision },
          });
    },
    '/api/reads/get-sources': (body) => ok({ revision: at(body), sources: [], declared: [] }),
    // A draft is saved as the service saves it; this spec does not study drafts.
    '/api/items/save-draft': (body) =>
      ok({
        item_id: body.item_id,
        base_revision: body.base_revision,
        content_digest: 'c'.repeat(64),
        editor: 'ada',
        saved_at: NOW,
      }),
    '/api/items/create-item': write('create_item', (body) => {
      const made = summary(NEW, text(body, 'path'), text(body, 'title'), text(body, 'type_name'));
      const revision = commit((tree) => ({ ...tree, items: [...tree.items, made] }));
      return ok(document({ ...made, revision }, text(body, 'body')));
    }),
    '/api/items/create-folder': write('create_folder', (body) =>
      done(commit((tree) => ({ ...tree, folders: [...tree.folders, text(body, 'folder')] }))),
    ),
    '/api/items/move-item': write('move_item', (body) =>
      done(
        commit((tree) => ({
          ...tree,
          items: tree.items.map((item) =>
            item.id === body.item_id ? { ...item, path: text(body, 'destination') } : item,
          ),
        })),
      ),
    ),
    '/api/items/delete-item': write('delete_item', (body) =>
      done(
        commit((tree) => ({
          ...tree,
          items: tree.items.filter((item) => item.id !== body.item_id),
        })),
      ),
    ),
    '/api/items/set-lifecycle': write('set_lifecycle', (body) =>
      done(
        commit((tree) => ({
          ...tree,
          items: tree.items.map((item) =>
            item.id === body.item_id
              ? {
                  ...item,
                  ...(typeof body.status === 'string' ? { status: body.status } : {}),
                  ...(typeof body.archived === 'boolean' ? { archived: body.archived } : {}),
                }
              : item,
          ),
        })),
      ),
    ),
    '/api/schemas/naming-rules': () => ok(schema),
    '/api/conventions/get-rules': (body) => ok(rules.get(at(body))),
    '/api/conventions/preview-names': (body) => {
      const revision = at(body);
      const ids = Array.isArray(body.item_ids) ? body.item_ids : [];
      const asked = body.rules as { rules: Array<{ prefix: string }> };
      const prefix = asked.rules[0]?.prefix ?? '';
      return ok({
        base_revision: revision,
        rules_digest: 'c'.repeat(64),
        entries: treeAt(revision)
          .items.filter((item) => ids.includes(item.id))
          .map((item) => {
            const cut = item.path.lastIndexOf('/') + 1;
            const to =
              item.id === SOURCE
                ? item.path
                : `${item.path.slice(0, cut)}${prefix}${item.path.slice(cut)}`;
            return { item_id: item.id, from: item.path, to, warnings: [] };
          }),
        duplicate_observations: [],
        warnings: [],
      });
    },
    '/api/conventions/apply-names': (body) => {
      sent.push({ path: 'apply_names', body });
      const asked = body.plan as {
        base_revision: string;
        entries: Array<{ item_id: string; to: string }>;
      };
      if (asked.base_revision !== state.head) {
        return { status: 409, body: { code: 'conflict', message: 'the plan is out of date' } };
      }
      return done(
        commit((tree) => ({
          ...tree,
          items: tree.items.map((item) => ({
            ...item,
            path: asked.entries.find((entry) => entry.item_id === item.id)?.to ?? item.path,
          })),
        })),
      );
    },
  };
  return {
    answers,
    sent,
    failNext: (count) => {
      state.failing = count;
    },
    othersSave: (saved) => {
      const next = commit((tree) => tree);
      rules.set(next, saved);
      return next;
    },
    holdWorkspace: () => {
      let release = () => {};
      held.wait = new Promise<void>((resolve) => {
        release = resolve;
      });
      return () => {
        held.wait = null;
        release();
      };
    },
  };
}

/** Press a control from the keyboard: focus it, then Enter. */
async function press(page: Page, name: string | RegExp, within: Locator | Page = page) {
  const button = within.getByRole('button', { name, exact: true });
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  await button.focus();
  await page.keyboard.press('Enter');
}

/** Keyboard focus 300 ms on, after the frame in which a browser moves it off a removed control. */
async function focusSettled(page: Page): Promise<string> {
  await page.waitForTimeout(300);
  return focused(page);
}

const row = (page: Page, name: string | RegExp) =>
  page.getByRole('treeitem', { name, exact: true });
/** The one pane of documents: the item's own controls are in it, the tree's bar is not. */
const documents = (page: Page) => page.getByRole('region', { name: 'Documents' });
const headLine = (page: Page) => page.locator('.head-line');
const revision = (index: number) => `revision ${(MADE[index] ?? '').slice(0, 10)}`;

/** Open the "notes" folder from the keyboard. */
async function openNotes(page: Page) {
  await row(page, 'notes').focus();
  await page.keyboard.press('ArrowRight');
  await expect(row(page, /Meeting notes/)).toBeVisible();
}

test.describe('changing the tree and the naming rules', () => {
  const server = useExplorerServer();

  test('create a note and a folder by keyboard', async ({ page }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}`);
    await openNotes(page);

    await press(page, 'New note');
    await expect(page.getByRole('textbox', { name: 'Title' })).toBeFocused();
    await page.keyboard.type('Decisions');
    await expect(page.getByRole('textbox', { name: 'File name' })).toHaveValue('Decisions.md');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the new note form');
    await shot(page, '18-new-note-form');
    await page.keyboard.press('Enter');

    // The note opens in its editor, with focus in its text; the tree lists it without a Refresh.
    const box = page.getByRole('textbox', { name: 'Text of Decisions' });
    await expect(box).toBeFocused();
    await page.keyboard.press('Control+End');
    await page.keyboard.type('Written at once.');
    await expect(box).toContainText('Written at once.');
    await expect(row(page, /Decisions, type note/)).toBeVisible();
    await expect(headLine(page)).toContainText(revision(0));
    expect(played.sent.at(0)).toMatchObject({
      path: 'create_item',
      body: { kind: 'note', path: 'notes/Decisions.md', title: 'Decisions', base_revision: HEAD },
    });
    expect(await focusSettled(page)).toBe('textbox:Text of Decisions');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the new note in its editor');
    await shot(page, '19-new-note-open');

    // A folder, inside the folder the tree is in: focus ends on its row.
    await row(page, 'notes').focus();
    await press(page, 'New folder');
    await expect(page.getByRole('textbox', { name: 'Folder name' })).toBeFocused();
    await expect(page.getByRole('textbox', { name: 'In folder' })).toHaveValue('notes');
    await page.keyboard.type('archive');
    await page.keyboard.press('Enter');
    await expect(row(page, 'archive')).toBeFocused();
    expect(played.sent.at(-1)).toMatchObject({
      path: 'create_folder',
      body: { folder: 'notes/archive', base_revision: MADE[0] },
    });
    expect(await focusSettled(page)).toBe('treeitem:archive');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the new folder in the tree');
    await shot(page, '20-new-folder');
  });

  test('rename an open item and keep its tab, then change its status', async ({ page }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toBeVisible();

    await press(page, 'Rename or move', documents(page));
    const name = page.getByRole('textbox', { name: 'File name' });
    await expect(name).toBeFocused();
    await expect(name).toHaveValue('meeting.md');
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('minutes.md');
    await expectNoAxeViolations(page, 'the rename form');
    await shot(page, '21-rename-form');
    await page.keyboard.press('Enter');

    await expect(documents(page).getByText('notes/minutes.md', { exact: true })).toBeVisible();
    await expect(page.getByRole('tab')).toHaveCount(1);
    await expect(page.getByRole('tab', { name: 'Meeting notes' })).toHaveAttribute(
      'data-item-id',
      NOTE,
    );
    await expect(headLine(page)).toContainText(revision(0));
    expect(played.sent.at(0)).toMatchObject({
      path: 'move_item',
      body: { item_id: NOTE, destination: 'notes/minutes.md', base_revision: HEAD },
    });
    expect(await focusSettled(page)).toBe('button:Rename or move');
    await expectVisibleFocus(page);

    // The status word, from the keyboard; the choice keeps focus, and the tree shows the word.
    const status = page.getByRole('combobox', { name: 'Status' });
    await status.focus();
    await status.selectOption('deprecated');
    await expect(page.getByText('The status is now deprecated.')).toBeVisible();
    await expect(status).toHaveValue('deprecated');
    await expect(row(page, /Meeting notes, type meeting-note, status deprecated/)).toBeVisible();
    expect(played.sent.at(-1)).toMatchObject({
      path: 'set_lifecycle',
      body: { item_id: NOTE, status: 'deprecated', base_revision: MADE[0] },
    });
    expect(await focusSettled(page)).toBe('select:draftstabledeprecated');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the renamed item with its new status');
    await shot(page, '22-renamed-and-status');
  });

  test('delete with confirmation', async ({ page }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    await row(page, /Meeting notes/).focus();
    await page.keyboard.press('Delete');

    // The question opens on the choice that keeps the item.
    const question = page.getByRole('group', { name: 'Delete “Meeting notes”?' });
    await expect(question.getByRole('button', { name: 'Keep it' })).toBeFocused();
    await expect(question).toContainText('one item');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the question before a delete');
    await shot(page, '23-delete-question');
    expect(played.sent).toEqual([]);

    await page.keyboard.press('Tab');
    await expect(question.getByRole('button', { name: 'Delete it' })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(row(page, /Meeting notes/)).toHaveCount(0);
    await expect(row(page, /^Plan/)).toBeFocused();
    expect(played.sent).toMatchObject([
      { path: 'delete_item', body: { item_id: NOTE, base_revision: HEAD } },
    ]);
    // The open tab says so, shows no text as current, and offers to close.
    await expect(page.getByRole('tab', { name: 'Meeting notes (deleted)' })).toBeVisible();
    await expect(page.getByText('You deleted this item.')).toBeVisible();
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toHaveCount(0);
    expect(await focusSettled(page)).toMatch(/^treeitem:Plan/);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the tab of a deleted item');
    await shot(page, '24-deleted-tab');

    await press(page, 'Close this tab');
    await expect(page.getByRole('tab')).toHaveCount(0);
    expect(await focusSettled(page)).not.toBe('body');
  });

  test('a naming preview, then apply', async ({ page }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${PLAN}`);
    await expect(page.getByText('Text of Plan.')).toBeVisible();

    await press(page, 'Naming rules');
    await expect(page.getByRole('heading', { name: 'Naming rules' })).toBeFocused();
    const prefix = page.getByRole('textbox', { name: /^prefix/ });
    await prefix.focus();
    await page.keyboard.type('2025-');
    await expectNoAxeViolations(page, 'the naming rules form');
    await shot(page, '25-naming-rules-form');

    await press(page, 'Preview the renames');
    await expect(
      page.getByRole('heading', { name: 'Preview: what would be renamed' }),
    ).toBeFocused();
    const table = page.getByRole('table', { name: 'The renames, one by one' });
    await expect(table.getByRole('row')).toHaveCount(3);
    await expect(table).toContainText('notes/plan.md');
    await expect(table).toContainText('notes/2025-plan.md');
    await expect(headLine(page)).toContainText(`revision ${HEAD.slice(0, 10)}`);
    expect(played.sent).toEqual([]);
    await expectNoAxeViolations(page, 'the naming preview');
    await shot(page, '26-naming-preview');

    await press(page, 'Apply this preview…');
    await expect(page.getByRole('button', { name: 'Keep the names' })).toBeFocused();
    await expectVisibleFocus(page);
    await shot(page, '27-naming-apply-question');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Enter');
    await expect(page.locator('.rules-said')).toContainText('2 items were renamed.');
    await expect(headLine(page)).toContainText(revision(0));
    expect(played.sent).toMatchObject([
      { path: 'apply_names', body: { plan: { base_revision: HEAD } } },
    ]);
    expect(await focusSettled(page)).toBe('h2:Naming rules');
    await expectNoAxeViolations(page, 'the naming rules after the renaming');
    await shot(page, '28-naming-applied');

    // Back to the documents: the open item is the same item, at its new name.
    // The rules that renamed were never saved, so leaving asks first, on the choice that keeps them.
    await press(page, 'Close the naming rules');
    await expect(page.getByRole('button', { name: 'Keep editing' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the question before leaving the naming rules');
    await shot(page, '32-naming-leave-question');
    await press(page, 'Discard my changes');
    await expect(page.getByText('notes/2025-plan.md', { exact: true })).toBeVisible();
    expect(await focusSettled(page)).toBe('button:Naming rules');
    await expectVisibleFocus(page);
  });

  test('the YAML views of the naming rules and of an item’s properties', async ({ page }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toBeVisible();

    // The rules: the YAML tab from the keyboard, then text that cannot be read.
    await press(page, 'Naming rules');
    await page.getByRole('tab', { name: 'Form' }).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Enter');
    const rules = page.getByRole('textbox', { name: 'The naming rules as YAML' });
    await expect(rules).toContainText('schema_version: 1');
    await rules.focus();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type('x-team: [a, b');
    await expect(page.getByRole('alert')).toContainText('This YAML cannot be read.');
    expect(await focusSettled(page)).toBe('textbox:The naming rules as YAML');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the naming rules as YAML, with a problem');
    await shot(page, '30-naming-rules-yaml');
    // Read after the typing paused, the text is no rules at all: nothing became of it, so there is
    // nothing to save, and nothing is sent.
    await expect(page.getByRole('button', { name: 'Save the rules', exact: true })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(played.sent).toEqual([]);
    // Text that cannot be read is an edit too: leaving asks, and the person discards it.
    await press(page, 'Close the naming rules');
    await press(page, 'Discard my changes');

    // The properties of the open note: its editor, then the YAML tab.
    await press(page, 'Edit', documents(page));
    await documents(page).getByRole('tab', { name: 'Form' }).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Enter');
    const properties = page.getByRole('textbox', { name: 'Properties as YAML' });
    await expect(properties).toContainText('type: meeting-note');
    await expect(properties).not.toContainText('okf_jawn');
    await properties.focus();
    await page.keyboard.press('ControlOrMeta+End');
    await page.keyboard.type('reviewed: "yes"');
    await expect(page.locator('.save-state')).toContainText('Saved as a draft at');
    expect(await focusSettled(page)).toBe('textbox:Properties as YAML');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the properties as YAML');
    await shot(page, '31-properties-yaml');
  });

  test('the YAML view of the naming rules follows newer rules where keyboard focus is', async ({
    page,
  }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(row(page, 'notes')).toBeVisible();
    await press(page, 'Naming rules');
    await page.getByRole('tab', { name: 'Form' }).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Enter');
    const box = page.getByRole('textbox', { name: 'The naming rules as YAML' });
    await expect(box).toContainText('prefix: ""');

    // Someone else saves other rules, and the person refreshes: the box holds nothing of theirs,
    // so it shows the newer rules, and focus stays on Refresh, where the person put it.
    played.othersSave({ schema_version: 1, rules: [{ ...rule, prefix: 'theirs-' }] });
    await press(page, 'Refresh');
    await expect(box).toContainText('prefix: theirs-');
    expect(await focusSettled(page)).toBe('button:Refresh');

    // Again, with a Refresh that is slow to answer. Meanwhile the person goes to the YAML box,
    // moves the caret to the end of its text, and types nothing.
    played.othersSave({ schema_version: 1, rules: [{ ...rule, prefix: 'newest-' }] });
    const answer = played.holdWorkspace();
    await press(page, 'Refresh');
    await box.focus();
    await page.keyboard.press('ControlOrMeta+End');
    expect(await focusSettled(page)).toBe('textbox:The naming rules as YAML');
    answer();
    await expect(box).toContainText('prefix: newest-');
    // Focus is where it was, in the box, never on the page body, and the person goes on typing:
    // the text is another one, so the caret is at its start.
    expect(await focusSettled(page)).toBe('textbox:The naming rules as YAML');
    await expectVisibleFocus(page);
    await page.keyboard.type('# mine');
    await page.keyboard.press('Enter');
    await expect(box.locator('.cm-line').first()).toHaveText('# mine');
    await expect(box).toContainText('prefix: newest-');
    expect(await focusSettled(page)).toBe('textbox:The naming rules as YAML');
    await expectNoAxeViolations(page, 'the naming rules as YAML after they followed a Refresh');
    expect(played.sent).toEqual([]);
  });

  test('reading YAML never stalls the page: the worker is ended at its limit, and the text stays', async ({
    page,
  }) => {
    const played = await service();
    await routeApi(page, played.answers);
    // First a reader that never comes back: the worker's script is answered with one that says it
    // is ready and then spins for good on the first text. Everything on the page's side is real:
    // the worker, the limit, and ending it.
    const workerScript = '**/assets/yaml.worker-*.js';
    let stubbed = 0;
    await page.route(workerScript, async (route) => {
      stubbed += 1;
      await route.fulfill({
        status: 200,
        contentType: 'text/javascript',
        headers: { 'cache-control': 'no-store' },
        body: 'addEventListener("message", () => { for (;;) {} }); postMessage({ ready: true });',
      });
    });
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(row(page, 'notes')).toBeVisible();
    await press(page, 'Naming rules');
    await page.getByRole('tab', { name: 'Form' }).focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Enter');
    const rules = page.getByRole('textbox', { name: 'The naming rules as YAML' });
    await expect(rules).toContainText('schema_version: 1');

    // The page's own clock: the longest its thread went without running a timer, and the longest
    // a key or a click waited before the page handled it.
    await page.evaluate(() => {
      const seen = { gap: 0, wait: 0 };
      let last = performance.now();
      /** What was seen since the last look, which starts the count again. */
      const look = () => {
        const was = { ...seen };
        seen.gap = 0;
        seen.wait = 0;
        last = performance.now();
        return was;
      };
      Object.assign(window, { okfStall: look });
      setInterval(() => {
        const now = performance.now();
        seen.gap = Math.max(seen.gap, now - last);
        last = now;
      }, 20);
      for (const kind of ['keydown', 'click']) {
        document.addEventListener(
          kind,
          (event) => {
            seen.wait = Math.max(seen.wait, performance.now() - event.timeStamp);
          },
          true,
        );
      }
    });
    const stall = () =>
      page.evaluate(() =>
        (window as unknown as { okfStall: () => { gap: number; wait: number } }).okfStall(),
      );
    /** Since the last look the page went on: no timer waited a second, and no key or click did. */
    const expectNoStall = async () => {
      const seen = await stall();
      expect(seen.gap).toBeLessThan(1_000);
      expect(seen.wait).toBeLessThan(1_000);
    };
    /** Replace the whole text at once, as a paste does. */
    const paste = async (text: string) => {
      await rules.focus();
      await page.keyboard.press('ControlOrMeta+A');
      await page.keyboard.insertText(text);
    };
    const alert = page.getByRole('alert');
    const tooComplex =
      'This text is too complex to read here: reading it took more than a second and was stopped.';

    await paste('schema_version: 1\nrules: [] # never read\n');
    // While it is read, a press is handled at once: it sends nothing and says why.
    await press(page, 'Preview the renames');
    await expect(page.locator('.rules-said')).toContainText(
      'The text of the YAML view is still being read. Nothing was sent or changed',
      { timeout: 1_000 },
    );
    await expect.poll(() => page.workers().length).toBe(1);
    // At the limit the reading is ended, and the person is told in plain words.
    await expect(alert).toContainText('This YAML cannot be read. Nothing of it is sent.');
    await expect(alert).toContainText(tooComplex);
    expect(stubbed).toBe(1);
    // Ended is ended: the worker that would never have come back is gone.
    await expect.poll(() => page.workers().length).toBe(0);
    // The text stays in the box, and nothing of it is sent at the next press either.
    await expect(rules).toContainText('rules: [] # never read');
    await press(page, 'Preview the renames');
    expect(await focusSettled(page)).toContain('This YAML cannot be read');
    expect(played.sent).toEqual([]);
    await expectNoStall();
    await expectVisibleFocus(page);
    // The accessibility check and the screenshot run on the page's thread themselves: they are
    // not counted.
    await expectNoAxeViolations(page, 'a YAML text that is too complex to read');
    await shot(page, '33-yaml-too-complex');
    await stall();

    // Now the reader itself, in a new worker (the one before was ended). An ordinary text is
    // read at once.
    await page.unroute(workerScript);
    await paste('schema_version: 1\nrules: []\n');
    await expect(page.getByText('These rules differ from the saved ones.')).toBeVisible();
    await expect(alert.filter({ hasText: 'This YAML cannot be read' })).toHaveCount(0);
    // A worker that answered is kept for the next text, and reads it.
    const [kept] = page.workers();
    expect(page.workers()).toHaveLength(1);
    await paste('schema_version: 1\nrules: [\n');
    await expect(alert).toContainText('This YAML cannot be read. Nothing of it is sent.');
    expect(page.workers()).toEqual([kept]);
    await paste('schema_version: 1\nrules: []\n');
    await expect(alert.filter({ hasText: 'This YAML cannot be read' })).toHaveCount(0);

    // And the re-review's text, inside the bound on length and on depth. This engine's reader
    // takes seconds over it, or gives up on its depth in a moment; either way it is refused in
    // the page's words, and the page's thread never waited for it.
    const slow = `[${'a: '.repeat(2730)}]`;
    expect(slow.length).toBe(8192);
    await paste(slow);
    await expect(alert).toContainText(
      /too complex to read here: reading it took more than a second|This text nests deeper than this browser can read/,
    );
    await expect(rules).toContainText('a: a: a: a: ');
    await press(page, 'Preview the renames');
    expect(await focusSettled(page)).toContain('This YAML cannot be read');
    expect(played.sent).toEqual([]);

    await expectNoStall();
  });

  test('Back and Forward ask before unsaved rule edits are lost', async ({ page }) => {
    const played = await service();
    await routeApi(page, {
      ...played.answers,
      '/api/workspaces/list-workspaces': () =>
        ok({ items: [writableWorkspace], next_cursor: null }),
    });
    await page.goto(`${server.origin()}/`);
    await page.getByRole('link', { name: 'Workshop notes' }).click();
    await expect(row(page, 'notes')).toBeVisible();
    await press(page, 'Naming rules');
    const prefix = page.getByRole('textbox', { name: /^prefix/ });
    await prefix.fill('unsaved-');
    const question = page.getByRole('group', { name: 'Leave the naming rules without saving?' });
    const onWorkspace = new RegExp(`/w/${WS}$`);

    // Back: the page asks, and has gone nowhere. (A browser moves its address before it tells the
    // page, so while the question is open the address is the one being gone to; the router puts
    // it back when the person stays.)
    await page.evaluate(() => history.back());
    await expect(question).toBeVisible();
    await expect(prefix).toHaveValue('unsaved-');
    expect(await focusSettled(page)).toBe('button:Keep editing');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the question before Back leaves the naming rules');
    await page.keyboard.press('Enter');
    await expect(question).toHaveCount(0);
    await expect(page).toHaveURL(onWorkspace);
    await expect(prefix).toHaveValue('unsaved-');
    await expect(prefix).toBeFocused();

    // Another way out, taken while the question is open, ends the first: Back is undone, and the
    // question is then about Close.
    await page.evaluate(() => history.back());
    await expect(question).toBeVisible();
    await press(page, 'Close the naming rules');
    await expect(page).toHaveURL(onWorkspace);
    await expect(question).toBeVisible();
    await press(page, 'Keep editing');
    await expect(question).toHaveCount(0);
    await expect(prefix).toHaveValue('unsaved-');

    // Back again, and this time the edits are discarded: the workspace list.
    await page.evaluate(() => history.back());
    await press(page, 'Discard my changes');
    await expect(page.getByRole('heading', { name: 'Your workspaces' })).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`^${server.origin()}/$`));
    // Focus went back into the rules before they left, so the new page's heading takes it.
    expect(await focusSettled(page)).toBe('h1:Your workspaces');

    // Forward asks too. Open an item, go back to the workspace (nothing to lose: no question),
    // edit the rules, then go forward to the item.
    await page.evaluate(() => history.forward());
    await expect(row(page, 'notes')).toBeVisible();
    await openNotes(page);
    await row(page, /Meeting notes/).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    await page.evaluate(() => history.back());
    await expect(page).toHaveURL(onWorkspace);
    await press(page, 'Naming rules');
    await prefix.fill('again-');
    await page.evaluate(() => history.forward());
    await expect(question).toBeVisible();
    await press(page, 'Keep editing');
    await expect(prefix).toHaveValue('again-');
    await expect(page).toHaveURL(onWorkspace);
    await page.evaluate(() => history.forward());
    await press(page, 'Discard my changes');
    // The item is shown, in the place the rules had.
    await expect(page).toHaveURL(new RegExp(`/w/${WS}/i/${NOTE}`));
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    await expect(page.getByRole('region', { name: 'Naming rules' })).toHaveCount(0);
    // What had focus went with the rules: it is on the page's heading, not on the page body.
    expect(await focusSettled(page)).toBe('h1:Workshop notes');
    await expectNoAxeViolations(page, 'the item shown after Forward left the naming rules');
    expect(played.sent).toEqual([]);
  });

  test('the "Naming rules" button asks before it closes rules with edits, and keeps focus', async ({
    page,
  }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(row(page, 'notes')).toBeVisible();
    await press(page, 'Naming rules');
    const prefix = page.getByRole('textbox', { name: /^prefix/ });
    await prefix.fill('unsaved-');
    const question = page.getByRole('group', { name: 'Leave the naming rules without saving?' });

    // The person stays: focus is back on the button that asked.
    await press(page, 'Naming rules');
    await expect(question).toBeVisible();
    expect(await focusSettled(page)).toBe('button:Keep editing');
    await page.keyboard.press('Enter');
    await expect(question).toHaveCount(0);
    await expect(prefix).toHaveValue('unsaved-');
    expect(await focusSettled(page)).toBe('button:Naming rules');

    // The person leaves: the pressed "Discard my changes" goes with the rules, and focus is on the
    // button that opened them, not on the page body.
    await page.keyboard.press('Enter');
    await expect(question).toBeVisible();
    await press(page, 'Discard my changes');
    await expect(page.getByRole('region', { name: 'Naming rules' })).toHaveCount(0);
    expect(await focusSettled(page)).toBe('button:Naming rules');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(
      page,
      'the workspace after the naming rules were left by their button',
    );
    expect(played.sent).toEqual([]);
  });

  test('a refused change is shown, and retried from the keyboard', async ({ page }) => {
    const played = await service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}`);
    await expect(row(page, 'notes')).toBeVisible();
    played.failNext(1);

    await press(page, 'New folder');
    await page.keyboard.type('archive');
    await page.keyboard.press('Enter');
    const alert = page.getByRole('alert');
    await expect(alert).toContainText(
      'Creating the folder: The service is temporarily unavailable.',
    );
    // Nothing reads as done: no such folder, the same version.
    await expect(row(page, 'archive')).toHaveCount(0);
    await expect(headLine(page)).toContainText(`revision ${HEAD.slice(0, 10)}`);
    await expectNoAxeViolations(page, 'a refused change');
    await shot(page, '29-change-refused');

    await press(page, 'Try again');
    await expect(row(page, 'archive')).toBeFocused();
    const [first, second] = played.sent;
    expect(second?.body).toEqual(first?.body);
    expect(await focusSettled(page)).toBe('treeitem:archive');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the folder made by the retry');
  });
});
