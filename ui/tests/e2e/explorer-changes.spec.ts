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
import { expect, type Page, test } from '@playwright/test';
import {
  type Answers,
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  markdown,
  NOTE,
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
}

async function service(): Promise<Service> {
  const schema: unknown = JSON.parse(await readFile(schemaFile, 'utf8'));
  const trees = new Map<string, { items: Item[]; folders: string[] }>([
    [HEAD, { items: [note, plan, source], folders: ['notes'] }],
  ]);
  const rules = new Map<string, unknown>([[HEAD, { schema_version: 1, rules: [rule] }]]);
  const state = { head: HEAD, made: 0, failing: 0 };
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
    '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: state.head }),
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
  };
}

/** Press a control from the keyboard: focus it, then Enter. */
async function press(page: Page, name: string | RegExp) {
  const button = page.getByRole('button', { name });
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  await button.focus();
  await page.keyboard.press('Enter');
}

/** Keyboard focus 300 ms on, after the frame in which a browser moves it off a removed control. */
async function focusSettled(page: Page): Promise<string> {
  await page.waitForTimeout(300);
  return focused(page);
}

const row = (page: Page, name: string | RegExp) => page.getByRole('treeitem', { name });
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

    await press(page, 'Rename or move');
    const name = page.getByRole('textbox', { name: 'File name' });
    await expect(name).toBeFocused();
    await expect(name).toHaveValue('meeting.md');
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type('minutes.md');
    await expectNoAxeViolations(page, 'the rename form');
    await shot(page, '21-rename-form');
    await page.keyboard.press('Enter');

    const pane = page.getByRole('region', { name: 'Documents' });
    await expect(pane.getByText('notes/minutes.md', { exact: true })).toBeVisible();
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
    await press(page, 'Close the naming rules');
    await expect(page.getByText('notes/2025-plan.md', { exact: true })).toBeVisible();
    expect(await focusSettled(page)).toBe('button:Naming rules');
    await expectVisibleFocus(page);
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
