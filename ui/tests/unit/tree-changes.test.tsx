/**
 * Changing the tree: a new note and a new folder, a move or rename, a delete after its question,
 * the status word and Archive; what each tells the cache; and how each refusal is shown.
 *
 * The service is a small one that remembers: it keeps the tree of every revision it made, so a
 * listing or a read at a revision answers what that revision held, and a write on another base
 * than its head is a `conflict`, as the real one answers.
 *
 * It holds an item as the service does (`crates/core/src/items.rs`, `crates/contract/src/item.rs`):
 * `archived` is in the application's header (`okf_jawn`), and the status word is the file's
 * `status` property. And it keeps drafts as `save_draft` does
 * (`crates/core/src/application/drafts.rs`): `not_found` for an item that is not at the head,
 * `invalid_input` at `/properties/okf_jawn` for a header that is present and is not exactly the
 * stored one, the head as the base of a first save and of a save that names the head, and the
 * draft's own base otherwise.
 */
import { act, cleanup, renderHook, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ItemSummary } from '../../src/api/generated/types.gen';
import { headerBehind, resetDraftSessions, stopSaving } from '../../src/features/drafts/saving';
import { useDeleted, useExplorerStore } from '../../src/features/explorer/store';
import { fieldInWords } from '../../src/features/tree/fields';
import {
  folderProblem,
  joinPath,
  pathProblem,
  segmentProblem,
  splitPath,
} from '../../src/features/tree/names';
import { type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import { findTextBox, textIn } from './support/editor';
import {
  draftOf,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  OTHER,
  otherSummary,
  RECEIPT,
  readResponse,
  renderAt,
  SOURCE,
  sourceSummary,
  summary,
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const LIST = '/api/items/list-items';
const ITEM = '/api/items/get-item';
const READ = '/api/reads/read-item';
const SEARCH = '/api/search/search-items';
const SAVE = '/api/items/save-draft';
const CREATE = '/api/items/create-item';
const FOLDER = '/api/items/create-folder';
const MOVE = '/api/items/move-item';
const DELETE = '/api/items/delete-item';
const LIFE = '/api/items/set-lifecycle';
const NEW = '66666666-6666-4666-8666-666666666666';
/** The revisions the service makes, in order. */
const MADE = ['c', 'd', 'e', 'f'].map((digit) => digit.repeat(40));
const FIRST = MADE.at(0) ?? '';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.unstubAllGlobals();
});

interface Tree {
  items: ItemSummary[];
  folders: string[];
}

const refuse = (status: number, code: string, message: string, field?: string): Reply => ({
  status,
  body: { code, message, ...(field === undefined ? {} : { field }) },
});
const text = (body: Json, key: string): string => {
  const value = body[key];
  return typeof value === 'string' ? value : '';
};
const revisionAsked = (body: Json, head: string): string => {
  const at = body.at;
  return at !== null &&
    typeof at === 'object' &&
    'revision' in at &&
    typeof at.revision === 'string'
    ? at.revision
    : head;
};

/**
 * The service. `drafts`: items the person has a draft of. A handler in `overrides` answers in
 * place of the service's own for that operation.
 */
function service(
  overrides: Record<string, Handler> = {},
  drafts: string[] = [],
  draftBase: string = HEAD,
) {
  const trees = new Map<string, Tree>([
    [HEAD, { items: [noteSummary, otherSummary, sourceSummary], folders: ['notes'] }],
  ]);
  const state = {
    head: HEAD,
    made: 0,
    permissions: writableWorkspace.permissions,
    /** How many saves were refused for their header. */
    refusedSaves: 0,
  };
  const treeAt = (revision: string): Tree => trees.get(revision) ?? { items: [], folders: [] };
  const commit = (change: (tree: Tree) => Tree): string => {
    const next = MADE.at(state.made) ?? '';
    state.made += 1;
    trees.set(next, change(treeAt(state.head)));
    state.head = next;
    return next;
  };
  const stale = (body: Json): Reply | null =>
    text(body, 'base_revision') === state.head
      ? null
      : refuse(409, 'conflict', 'the workspace head moved since the base revision');
  const taken = (path: string, but?: string): Reply | null =>
    treeAt(state.head).items.some((item) => item.path === path && item.id !== but)
      ? refuse(409, 'conflict', `an item already exists at ${path}`)
      : null;
  const done = (revision: string): Reply => ok({ revision, receipt_id: RECEIPT, warnings: [] });
  const found = (body: Json) => {
    const revision = revisionAsked(body, state.head);
    const item = treeAt(revision).items.find((each) => each.id === body.item_id);
    return item === undefined ? null : { ...item, revision };
  };
  const missing = refuse(404, 'not_found', 'no such item at this revision');
  /** The item's properties as the service stores them: the header holds `archived`. */
  const stored = (item: ItemSummary) =>
    itemDocument(item, {
      okf_jawn: { id: item.id, ...(item.archived ? { archived: true } : {}) },
      ...(item.status === 'stable' ? {} : { status: item.status }),
    });
  /** The drafts the service holds, as they were last saved. */
  const saved = new Map<string, { body: string; properties: unknown; base: string }>();
  for (const id of drafts) {
    const item = treeAt(HEAD).items.find((each) => each.id === id);
    if (item !== undefined) {
      saved.set(id, { body: 'My draft', properties: stored(item).properties, base: draftBase });
    }
  }
  const api = mockApi({
    [OPEN]: () => ok({ ...writableWorkspace, permissions: state.permissions, head: state.head }),
    [LIST]: (body) => {
      const revision = revisionAsked(body, state.head);
      const tree = treeAt(revision);
      const folder = text(body, 'folder');
      return ok({
        folders: tree.folders.filter((each) => splitPath(each).folder === folder),
        items: tree.items
          .filter((item) => splitPath(item.path).folder === folder && !item.archived)
          .map((item) => ({ ...item, revision })),
        revision,
        next_cursor: null,
      });
    },
    [ITEM]: (body) => {
      const item = found(body);
      if (item === null) return missing;
      const document = stored(item);
      const draft = drafts.includes(item.id) ? saved.get(item.id) : undefined;
      return ok(
        draft === undefined
          ? document
          : {
              ...document,
              draft: {
                body: draft.body,
                properties: draft.properties,
                draft: draftOf(item.id, draft.base),
              },
            },
      );
    },
    [READ]: (body) => {
      const item = found(body);
      return item === null
        ? missing
        : ok(readResponse(item, `# ${item.title}\n\nText of ${item.title}`));
    },
    '/api/reads/get-sources': (body) =>
      ok({ revision: revisionAsked(body, state.head), sources: [], declared: [] }),
    '/api/items/list-drafts': () =>
      ok({
        items: drafts.map((id) => draftOf(id, saved.get(id)?.base ?? HEAD)),
        next_cursor: null,
      }),
    [SAVE]: (body) => {
      const id = text(body, 'item_id');
      const item = treeAt(state.head).items.find((each) => each.id === id);
      if (item === undefined) return refuse(404, 'not_found', 'no such item at the head');
      const properties = body.properties as Record<string, unknown>;
      const header = properties.okf_jawn;
      if (
        header !== undefined &&
        JSON.stringify(header) !== JSON.stringify(stored(item).properties.okf_jawn)
      ) {
        state.refusedSaves += 1;
        return refuse(
          400,
          'invalid_input',
          'the application header is server-owned and cannot be changed by a write',
          '/properties/okf_jawn',
        );
      }
      const before = drafts.includes(id) ? saved.get(id) : undefined;
      const base =
        before !== undefined && text(body, 'base_revision') !== state.head
          ? before.base
          : state.head;
      saved.set(id, { body: text(body, 'body'), properties, base });
      if (!drafts.includes(id)) drafts.push(id);
      return ok(draftOf(id, base));
    },
    '/api/types/list-types': () => ok({ items: [] }),
    [SEARCH]: (body) => ok({ revision: revisionAsked(body, state.head), hits: [] }),
    [CREATE]: (body) => {
      const path = text(body, 'path');
      const refusal = stale(body) ?? taken(path);
      if (refusal !== null) return refusal;
      const made = summary(NEW, path, text(body, 'title'), text(body, 'type_name'));
      const revision = commit((tree) => ({ ...tree, items: [...tree.items, made] }));
      return ok({ ...itemDocument({ ...made, revision }), body: text(body, 'body') });
    },
    [FOLDER]: (body) => {
      const refusal = stale(body);
      if (refusal !== null) return refusal;
      const folder = text(body, 'folder');
      return done(commit((tree) => ({ ...tree, folders: [...tree.folders, folder] })));
    },
    [MOVE]: (body) => {
      const path = text(body, 'destination');
      const refusal = stale(body) ?? taken(path, text(body, 'item_id'));
      if (refusal !== null) return refusal;
      return done(
        commit((tree) => ({
          ...tree,
          items: tree.items.map((item) => (item.id === body.item_id ? { ...item, path } : item)),
        })),
      );
    },
    [DELETE]: (body) => {
      const refusal = stale(body);
      if (refusal !== null) return refusal;
      return done(
        commit((tree) => ({
          ...tree,
          items: tree.items.filter((item) => item.id !== body.item_id),
        })),
      );
    },
    [LIFE]: (body) => {
      const refusal = stale(body);
      if (refusal !== null) return refusal;
      return done(
        commit((tree) => ({
          ...tree,
          items: tree.items.map((item) =>
            item.id === body.item_id
              ? {
                  ...item,
                  ...(typeof body.status === 'string'
                    ? { status: body.status as ItemSummary['status'] }
                    : {}),
                  ...(typeof body.archived === 'boolean' ? { archived: body.archived } : {}),
                }
              : item,
          ),
        })),
      );
    },
    ...overrides,
  });
  return {
    api,
    state,
    /** Someone else changes the workspace: the head moves to a revision this page has not seen. */
    moveHead: () => commit((tree) => tree),
    /** Someone else brings an item back, under its id. */
    restoreElsewhere: (item: ItemSummary) =>
      commit((tree) => ({ ...tree, items: [...tree.items, item] })),
    /** Someone else archives an item. */
    archiveElsewhere: (itemId: string) =>
      commit((tree) => ({
        ...tree,
        items: tree.items.map((item) => (item.id === itemId ? { ...item, archived: true } : item)),
      })),
  };
}

const tree = () => screen.getByRole('tree', { name: 'Folders' });
const row = async (name: string | RegExp) =>
  within(await screen.findByRole('tree', { name: 'Folders' })).findByRole('treeitem', { name });
const button = (name: string | RegExp) => screen.getByRole('button', { name });
const field = (name: string | RegExp) =>
  // A field with suggestions (the type) is a combobox to assistive technology.
  screen.getByRole(name === 'Type' ? 'combobox' : 'textbox', { name });
/** The whole notice a deleted item's pane shows, in `container`. */
const deletedNotice = (container: HTMLElement) =>
  within(container)
    .getByText(/You deleted this item/)
    .closest('[role="note"]');
const pane = () => screen.getByRole('region', { name: /^Documents/ });
const alertText = async () => (await screen.findByRole('alert')).textContent ?? '';
const type = async (name: string, value: string) => {
  await userEvent.clear(field(name));
  if (value.length > 0) await userEvent.type(field(name), value);
};

/** Open the workspace and the "notes" folder, so its two items show in the tree. */
async function openTree(url = `/w/${WS}`) {
  const view = renderAt(url);
  // A link to an item has opened its folders already.
  const notes = await row('notes');
  if (notes.getAttribute('aria-expanded') !== 'true') await userEvent.click(notes);
  await row(/Meeting notes/);
  return view;
}

describe('the rules of a name', () => {
  it('names what is wrong with a segment, and accepts an ordinary one', () => {
    for (const fine of ['meeting.md', 'Réunion 2025.md', 'a', '.env', 'con-cert.md', 'console']) {
      expect(segmentProblem(fine), fine).toBeNull();
    }
    expect(segmentProblem('')).toMatch(/Give it a name/);
    expect(segmentProblem('a/b')).toMatch(/cannot contain “\/”/);
    for (const refused of ['.', '..', '.git', '.GIT']) {
      expect(segmentProblem(refused), refused).toMatch(/cannot be used as a name/);
    }
    for (const character of ['\\', ':', '<', '>', '"', '|', '?', '*']) {
      expect(segmentProblem(`a${character}b`), character).toMatch(/cannot contain any of/);
    }
    for (const control of ['\u0000', '\u001f', '\u007f', '\u009f']) {
      expect(segmentProblem(`a${control}b`)).toMatch(/control character/);
    }
    expect(segmentProblem('a b')).toBeNull();
    expect(segmentProblem('notes.')).toMatch(/end with a dot or a space/);
    expect(segmentProblem('notes ')).toMatch(/end with a dot or a space/);
    for (const device of ['CON', 'con', 'nul.md', 'Com1.txt', 'LPT9', 'COM¹', 'conin$']) {
      expect(segmentProblem(device), device).toMatch(/Windows keeps for a device/);
    }
    // Only the letters a to z are folded, as the service folds them.
    expect(segmentProblem('con\u0131n$')).toBeNull();
  });

  it('checks every segment of a folder, joins in NFC, and bounds the whole path', () => {
    expect(folderProblem('')).toBeNull();
    expect(folderProblem('notes/2025')).toBeNull();
    expect(folderProblem('notes//2025')).toMatch(/no “\/” at its start or end/);
    expect(folderProblem('/notes')).toMatch(/no “\/” at its start or end/);
    expect(folderProblem('notes/con')).toMatch(/In “con”/);
    expect(joinPath('', 'a.md')).toBe('a.md');
    expect(joinPath('notes', 'a.md')).toBe('notes/a.md');
    // "é" as e + combining accent is sent as the one character.
    expect(joinPath('notes', 'é.md')).toBe('notes/é.md');
    expect(splitPath('notes/2025/a.md')).toEqual({ folder: 'notes/2025', name: 'a.md' });
    expect(splitPath('a.md')).toEqual({ folder: '', name: 'a.md' });
    // The whole path, in the bytes it is sent as: exactly the bound passes, one more does not.
    expect(pathProblem(`${'a'.repeat(4093)}.md`)).toBeNull();
    expect(pathProblem(`${'a'.repeat(4094)}.md`)).toBe(
      'The folder and the name together are longer than 4096 bytes: they take 4097. Letters outside the basic alphabet take more than one.',
    );
    // A letter of two bytes counts as two, though the client's own schema counts it as one.
    expect(pathProblem('é'.repeat(2048))).toBeNull();
    expect(pathProblem('é'.repeat(2049))).toMatch(/longer than 4096 bytes: they take 4098/);
    // Typed as a letter and a combining accent, it is sent as the one letter, and counted so.
    expect(pathProblem('e\u0301'.repeat(2048))).toBeNull();
    expect(pathProblem('a\u0000b')).toBe('This is not a path the workspace can hold.');
  });

  it('checks a name as it is sent: in NFC', () => {
    // "<" and a combining stroke are, in NFC, the one character U+226E: nothing reserved is sent.
    expect(joinPath('', 'a<\u0338b.md')).toBe('a\u226eb.md');
    expect(segmentProblem('a<\u0338b.md')).toBeNull();
    expect(segmentProblem('a>\u0338b.md')).toBeNull();
    expect(folderProblem('notes/a<\u0338b')).toBeNull();
    // What is still reserved in NFC is refused, with or without an accent beside it.
    expect(segmentProblem('a<b.md')).toMatch(/cannot contain any of/);
    expect(segmentProblem('e\u0301<.md')).toMatch(/cannot contain any of/);
    // What a name is checked as is what is sent, for every name here.
    for (const name of ['a<\u0338b.md', 'e\u0301.md', 'plain.md', 'con\u0131n$']) {
      expect(segmentProblem(name), name).toBe(segmentProblem(joinPath('', name)));
    }
  });
});

describe('a new note and a new folder', () => {
  it('offers no change to a person who may only read', async () => {
    const { state } = service();
    state.permissions = workspace.permissions;
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    expect(screen.queryByRole('toolbar', { name: 'Change the folders' })).toBeNull();
    for (const name of [/New note/, /New folder/, /Rename or move/, /^Delete/, /Archive/]) {
      expect(screen.queryByRole('button', { name })).toBeNull();
    }
    expect(screen.queryByRole('combobox', { name: 'Status' })).toBeNull();
  });

  it('creates a note in the current folder, shows it without a Refresh, and opens its editor', async () => {
    const { api } = service();
    await openTree();
    await userEvent.click(button('New note'));
    // The form opens in the folder of the tree's current entry, with focus in its first field.
    expect(document.activeElement).toBe(field('Title'));
    expect(field('In folder')).toHaveProperty('value', 'notes');
    await userEvent.type(field('Title'), 'Decisions');
    expect(field('File name')).toHaveProperty('value', 'Decisions.md');
    await userEvent.keyboard('{Enter}');

    const box = await findTextBox('Text of Decisions');
    await waitFor(() => expect(document.activeElement).toBe(box));
    expect(textIn(box)).toBe('# Decisions\n');
    const sent = api.callsTo(CREATE);
    expect(sent).toHaveLength(1);
    expect(sent.at(0)).toEqual({
      workspace_id: WS,
      base_revision: HEAD,
      idempotency_key: expect.stringMatching(UUID),
      kind: 'note',
      path: 'notes/Decisions.md',
      title: 'Decisions',
      type_name: 'note',
      body: '# Decisions\n',
      properties: {},
    });
    // The shell shows the revision the service made, and the tree lists it: no Refresh.
    expect(await row(/Decisions, type note/)).toBeTruthy();
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc');
    expect(api.callsTo(LIST).some((body) => revisionAsked(body, '') === FIRST)).toBe(true);
    // The committed note is what `get_item` answers at that revision: it is not asked for again.
    expect(api.callsTo(ITEM).filter((body) => body.item_id === NEW)).toEqual([]);
    expect(api.callsTo(OPEN)).toHaveLength(2); // the opening, and the check for a newer head
    expect(screen.getByText(/Created the note “Decisions” at notes\/Decisions.md/)).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('checks the name before anything is sent, and says what is wrong at the field', async () => {
    const { api } = service();
    await openTree();
    await userEvent.click(button('New note'));
    await userEvent.keyboard('{Enter}');
    expect(field('Title').getAttribute('aria-invalid')).toBe('true');
    expect(document.activeElement).toBe(field('Title'));
    expect(screen.getByText('Give the note a title.')).toBeTruthy();
    await userEvent.type(field('Title'), 'What? When');
    await userEvent.keyboard('{Enter}');
    // The file name followed the title, and a file name cannot hold "?".
    const name = field('File name');
    expect(document.activeElement).toBe(name);
    expect(name.getAttribute('aria-invalid')).toBe('true');
    const described = name.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(described)?.textContent).toMatch(/cannot contain any of/);
    await type('File name', 'when.md');
    await type('In folder', 'notes//x');
    await userEvent.keyboard('{Enter}');
    expect(document.activeElement).toBe(field('In folder'));
    await type('In folder', 'notes');
    await type('Type', '');
    await userEvent.keyboard('{Enter}');
    expect(document.activeElement).toBe(field('Type'));
    expect(api.callsTo(CREATE)).toEqual([]);
    await type('Type', 'question');
    await userEvent.keyboard('{Enter}');
    await findTextBox('Text of What? When');
    expect(api.callsTo(CREATE).at(0)).toMatchObject({
      path: 'notes/when.md',
      title: 'What? When',
      type_name: 'question',
    });
  });

  it('shows an invalid_input answer at the field it names, in the service’s words', async () => {
    const { api } = service({
      [CREATE]: () => refuse(400, 'invalid_input', 'path must be in Unicode NFC', '/path'),
    });
    await openTree();
    await userEvent.click(button('New note'));
    await userEvent.type(field('Title'), 'Decisions');
    await userEvent.click(button('Create the note'));
    const name = field('File name');
    await waitFor(() => expect(name.getAttribute('aria-invalid')).toBe('true'));
    const described = name.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(described)?.textContent).toBe(
      'The service refused this: path must be in Unicode NFC',
    );
    // Shown at the field, not as a notice; the form is still there, and nothing reads as done.
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByText(/Created the note/)).toBeNull();
    // Changing the value takes the refusal away.
    await userEvent.type(name, 'x');
    expect(name.getAttribute('aria-invalid')).toBe('false');
    expect(api.callsTo(CREATE)).toHaveLength(1);
  });

  it('creates a folder and puts keyboard focus on its row', async () => {
    const { api } = service();
    renderAt(`/w/${WS}`);
    await row('notes');
    await userEvent.click(button('New folder'));
    expect(document.activeElement).toBe(field('Folder name'));
    expect(field('In folder')).toHaveProperty('value', '');
    await userEvent.type(field('Folder name'), 'archive');
    await userEvent.keyboard('{Enter}');
    const made = await row('archive');
    await waitFor(() => expect(document.activeElement).toBe(made));
    expect(api.callsTo(FOLDER)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
        folder: 'archive',
      },
    ]);
    expect(screen.getByText('Created the folder archive.')).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('gives focus back to what opened the form on Cancel and on Escape', async () => {
    service();
    await openTree();
    await userEvent.click(button('New folder'));
    await userEvent.click(button('Cancel'));
    expect(document.activeElement).toBe(button('New folder'));
    await userEvent.click(button('New note'));
    await userEvent.keyboard('{Escape}');
    expect(document.activeElement).toBe(button('New note'));
    expect(screen.queryByRole('group', { name: 'New note' })).toBeNull();
  });

  it('reads "not available in this build" for an unbuilt operation, and creates nothing', async () => {
    const { api } = service({
      [CREATE]: () => refuse(501, 'not_implemented', 'create_item is not implemented'),
    });
    await openTree();
    await userEvent.click(button('New note'));
    await userEvent.type(field('Title'), 'Decisions');
    await userEvent.keyboard('{Enter}');
    expect(await alertText()).toContain('Creating the note: Not available in this build.');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(screen.queryByText(/Created the note/)).toBeNull();
    expect(within(tree()).queryByRole('treeitem', { name: /Decisions/ })).toBeNull();
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');
    expect(field('Title')).toHaveProperty('value', 'Decisions');
    expect(api.callsTo(LIST).every((body) => revisionAsked(body, '') === HEAD)).toBe(true);
  });

  it('repeats a failed request under the same key, and sends changed values under a new one', async () => {
    const real = service({ [FOLDER]: () => refuse(503, 'unavailable', 'the store is busy') });
    renderAt(`/w/${WS}`);
    await row('notes');
    await userEvent.click(button('New folder'));
    await userEvent.type(field('Folder name'), 'archive');
    await userEvent.keyboard('{Enter}');
    expect(await alertText()).toContain(
      'Creating the folder: The service is temporarily unavailable.',
    );
    await userEvent.click(button('Try again'));
    await waitFor(() => expect(real.api.callsTo(FOLDER)).toHaveLength(2));
    const [first, second] = real.api.callsTo(FOLDER);
    expect(second).toEqual(first);
    // The form's own button pressed again, with nothing changed, is that request once more.
    await screen.findByRole('button', { name: 'Try again' });
    await userEvent.click(button('Create the folder'));
    await waitFor(() => expect(real.api.callsTo(FOLDER)).toHaveLength(3));
    expect(real.api.callsTo(FOLDER).at(2)).toEqual(first);
    // Another name is another request.
    await screen.findByRole('button', { name: 'Try again' });
    await userEvent.type(field('Folder name'), '-2');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(real.api.callsTo(FOLDER)).toHaveLength(4));
    const third = real.api.callsTo(FOLDER).at(3);
    expect(third?.folder).toBe('archive-2');
    expect(third?.idempotency_key).not.toBe(first?.idempotency_key);
  });

  it('says that the workspace has a newer version, offers Refresh, and then asks on the new head', async () => {
    const { api, moveHead } = service();
    renderAt(`/w/${WS}`);
    await row('notes');
    const theirs = moveHead();
    await userEvent.click(button('New folder'));
    await userEvent.type(field('Folder name'), 'archive');
    await userEvent.keyboard('{Enter}');
    const alert = await screen.findByRole('alert');
    await waitFor(() =>
      expect(alert.textContent).toContain('has a newer version than the one shown'),
    );
    expect(alert.textContent).toContain('Creating the folder: nothing was changed.');
    expect(alert.textContent).toContain('the workspace head moved since the base revision');
    expect(within(tree()).queryByRole('treeitem', { name: 'archive' })).toBeNull();
    // The shell's own notice says so too; the shown head has not moved.
    expect(await screen.findByText(/A newer version of this workspace exists/)).toBeTruthy();
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');

    await userEvent.click(button('Refresh the workspace'));
    await screen.findByText(/The workspace was refreshed. Nothing was changed yet/);
    // The notice left with focus on its button: focus is on the form's own button.
    expect(document.activeElement).toBe(button('Create the folder'));
    expect(screen.queryByRole('alert')).toBeNull();
    expect(field('Folder name')).toHaveProperty('value', 'archive');
    await userEvent.keyboard('{Enter}');
    await row('archive');
    const [refused, accepted] = api.callsTo(FOLDER);
    expect(refused?.base_revision).toBe(HEAD);
    expect(accepted?.base_revision).toBe(theirs);
    expect(accepted?.idempotency_key).not.toBe(refused?.idempotency_key);
  });

  it('says that the name is taken when the head has not moved, and offers no Refresh', async () => {
    const { api } = service();
    await openTree();
    await userEvent.click(button('New note'));
    await userEvent.type(field('Title'), 'Plan');
    await type('File name', 'plan.md');
    await userEvent.keyboard('{Enter}');
    const alert = await screen.findByRole('alert');
    await waitFor(() => expect(alert.textContent).toContain('A name that is already taken'));
    expect(alert.textContent).toContain('an item already exists at notes/plan.md');
    expect(screen.queryByRole('button', { name: 'Refresh the workspace' })).toBeNull();
    expect(screen.queryByText(/Created the note/)).toBeNull();
    expect(api.callsTo(CREATE)).toHaveLength(1);
  });

  it('says so when it could not be checked whether the head moved', async () => {
    let opened = 0;
    service({
      [OPEN]: () => {
        opened += 1;
        return opened === 1
          ? ok(writableWorkspace)
          : refuse(503, 'unavailable', 'the store is busy');
      },
      [FOLDER]: () => refuse(409, 'conflict', 'refused'),
    });
    renderAt(`/w/${WS}`);
    await row('notes');
    await userEvent.click(button('New folder'));
    await userEvent.type(field('Folder name'), 'archive');
    await userEvent.keyboard('{Enter}');
    const alert = await screen.findByRole('alert');
    await waitFor(() => expect(alert.textContent).toContain('it could not be checked whether'));
    expect(button('Refresh the workspace')).toBeTruthy();
  });
});

describe('a move or a rename', () => {
  it('renames the open item from its tab: the tab stays, and tree and pane show the new place', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    const name = field('File name');
    expect(document.activeElement).toBe(name);
    expect(name).toHaveProperty('value', 'meeting.md');
    expect(field('In folder')).toHaveProperty('value', 'notes');
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');

    await within(pane()).findByText('notes/minutes.md');
    expect(api.callsTo(MOVE)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
        item_id: NOTE,
        destination: 'notes/minutes.md',
      },
    ]);
    // The same tab, on the same item, with focus back on what opened the form.
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    expect(screen.getByRole('tab', { name: /Meeting notes/ }).getAttribute('data-item-id')).toBe(
      NOTE,
    );
    expect(document.activeElement).toBe(
      within(pane()).getByRole('button', { name: 'Rename or move' }),
    );
    expect(screen.getByText(/is now at notes\/minutes.md. This tab stays on it/)).toBeTruthy();
    // Read at the head the move made, without a Refresh.
    expect(await screen.findByText(/Read from notes\/minutes.md at/)).toBeTruthy();
    expect(api.callsTo(READ).some((body) => revisionAsked(body, '') === FIRST)).toBe(true);
    expect(api.violations).toEqual([]);
  });

  it('moves an item to another folder from its row with F2, and focuses the row where it now is', async () => {
    const { api } = service();
    await openTree();
    const plan = await row(/^Plan/);
    await userEvent.click(plan);
    await screen.findByText('Text of Plan');
    act(() => plan.focus());
    await userEvent.keyboard('{F2}');
    expect(document.activeElement).toBe(field('File name'));
    await type('In folder', '');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(api.callsTo(MOVE)).toHaveLength(1));
    expect(api.callsTo(MOVE).at(0)).toMatchObject({ item_id: OTHER, destination: 'plan.md' });
    // At the top level now: a sibling of the folder it was in.
    await waitFor(async () => {
      const moved = await row(/^Plan/);
      expect(moved.getAttribute('aria-level')).toBe('1');
      expect(document.activeElement).toBe(moved);
    });
    expect(screen.getByRole('tab', { name: /Plan/ }).getAttribute('data-item-id')).toBe(OTHER);
  });

  it('does not send a destination that is where the item is', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    await userEvent.keyboard('{Enter}');
    expect(screen.getByText(/This is where it is now/)).toBeTruthy();
    expect(document.activeElement).toBe(field('File name'));
    expect(api.callsTo(MOVE)).toEqual([]);
  });

  it('moves a tab that cited the old head to the revision the move made, in every pane', async () => {
    service();
    const { router } = await openTree(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(button('Split right'));
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));
    // A tab that cites a revision offers no change; the tree's own row does.
    act(() =>
      screen
        .getAllByRole('treeitem', { name: /Meeting notes/ })
        .at(0)
        ?.focus(),
    );
    await userEvent.keyboard('{F2}');
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    // Both panes read the item where it now is; neither went on citing the old head.
    await waitFor(() => expect(screen.getAllByText('notes/minutes.md')).toHaveLength(2));
    expect(screen.queryByText('notes/meeting.md')).toBeNull();
    expect(router.state.location.search).toMatchObject({ rev: FIRST });
  });

  it('asks the search again under the head the move made', async () => {
    const { api } = service();
    // The search results stand where the folders were.
    renderAt(`/w/${WS}/i/${NOTE}?q=budget`);
    await screen.findByText('Text of Meeting notes');
    await waitFor(() => expect(api.callsTo(SEARCH)).toHaveLength(1));
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(api.callsTo(SEARCH)).toHaveLength(2));
    expect(api.callsTo(SEARCH).map((body) => revisionAsked(body, ''))).toEqual([HEAD, FIRST]);
  });
});

describe('a delete', () => {
  it('asks first, with focus on the choice that keeps the item, and keeps it on that choice', async () => {
    const { api } = service();
    await openTree();
    const note = await row(/Meeting notes/);
    await userEvent.click(note);
    await screen.findByText('Text of Meeting notes');
    act(() => note.focus());
    await userEvent.keyboard('{Delete}');
    const question = screen.getByRole('group', { name: 'Delete “Meeting notes”?' });
    expect(question.textContent).toContain(
      'notes/meeting.md is removed from the workspace as it is now: one item.',
    );
    expect(question.textContent).toContain('Its history is kept');
    expect(document.activeElement).toBe(button('Keep it'));
    await userEvent.keyboard('{Enter}');
    // Kept: nothing was sent, the row is there and has focus again.
    expect(api.callsTo(DELETE)).toEqual([]);
    expect(screen.queryByRole('group', { name: /^Delete “/ })).toBeNull();
    expect(document.activeElement).toBe(await row(/Meeting notes/));
  });

  it('deletes after the question, moves focus to the entry now at that place, and says so in the tab', async () => {
    const { api } = service();
    await openTree();
    const note = await row(/Meeting notes/);
    await userEvent.click(note);
    await screen.findByText('Text of Meeting notes');
    act(() => note.focus());
    await userEvent.keyboard('{Delete}');
    await userEvent.click(button('Delete it'));

    await waitFor(() =>
      expect(within(tree()).queryByRole('treeitem', { name: /Meeting notes/ })).toBeNull(),
    );
    expect(api.callsTo(DELETE)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
        item_id: NOTE,
      },
    ]);
    // The entry now where the deleted one was.
    const plan = await row(/^Plan/);
    await waitFor(() => expect(document.activeElement).toBe(plan));
    expect(screen.getByText('Deleted “Meeting notes”. Its history is kept.')).toBeTruthy();
    // Its open tab says so and offers to close; the text it showed is gone.
    expect(screen.getByRole('tab', { name: 'Meeting notes (deleted)' })).toBeTruthy();
    expect(deletedNotice(pane())?.textContent).toContain('There is no current text to show here.');
    expect(screen.queryByText('Text of Meeting notes')).toBeNull();
    // Nothing is asked about the item at the head that no longer holds it.
    const asked = [...api.callsTo(ITEM), ...api.callsTo(READ)].filter(
      (body) => body.item_id === NOTE && revisionAsked(body, '') === FIRST,
    );
    expect(asked).toEqual([]);
    await userEvent.click(within(pane()).getByRole('button', { name: 'Close this tab' }));
    expect(screen.queryByRole('tab', { name: /Meeting notes/ })).toBeNull();
    expect(api.violations).toEqual([]);
  });

  it('from the tab: focus goes to "Close this tab", and a tab that cites a revision says what it shows', async () => {
    service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(button('Split right'));
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));
    const second = screen.getByRole('region', { name: /pane 2 of 2/ });
    await userEvent.click(within(second).getByRole('button', { name: 'Delete' }));
    expect(document.activeElement).toBe(button('Keep it'));
    await userEvent.click(button('Delete it'));
    await waitFor(() => expect(screen.getAllByText(/You deleted this item/)).toHaveLength(2));
    expect(document.activeElement).toBe(
      within(second).getByRole('button', { name: 'Close this tab' }),
    );
    expect(screen.queryByText('Text of Meeting notes')).toBeNull();
  });

  it('keeps a cited revision readable under the notice that the item was deleted', async () => {
    service();
    await openTree(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    await screen.findByText('Text of Meeting notes');
    const note = await row(/Meeting notes/);
    act(() => note.focus());
    await userEvent.keyboard('{Delete}');
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    expect(deletedNotice(pane())?.textContent).toContain(
      'how it was at the revision this tab cites, not something current',
    );
    expect(screen.getByText('Text of Meeting notes')).toBeTruthy();
  });

  it('never shows a refused delete as done, and repeats it under the same key', async () => {
    let fails = 1;
    const made = { revision: FIRST, receipt_id: RECEIPT, warnings: [] };
    const { api } = service({
      [DELETE]: () => {
        if (fails === 0) return ok(made);
        fails -= 1;
        return refuse(503, 'unavailable', 'the store is busy');
      },
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    await userEvent.click(button('Delete it'));
    expect(await alertText()).toContain(
      'Deleting this item: The service is temporarily unavailable.',
    );
    // Still there, everywhere: the row, the tab, the text.
    expect(await row(/Meeting notes/)).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeTruthy();
    expect(screen.getByText('Text of Meeting notes')).toBeTruthy();
    expect(screen.queryByText(/You deleted this item/)).toBeNull();
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');
    await userEvent.click(button('Try again'));
    await screen.findByText(/You deleted this item/);
    const [first, second] = api.callsTo(DELETE);
    expect(second).toEqual(first);
  });

  it('offers no rename, move or delete of a folder, and says why', async () => {
    const { api } = service();
    await openTree();
    const folder = await row('notes');
    act(() => folder.focus());
    await userEvent.keyboard('{Delete}');
    expect(screen.getByText(/A folder cannot be renamed, moved or deleted here/)).toBeTruthy();
    expect(screen.queryByRole('group', { name: /^Delete “/ })).toBeNull();
    const rename = button('Rename or move (choose an item in the folders first)');
    expect(rename.getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(rename);
    expect(screen.queryByRole('group', { name: /^Rename or move/ })).toBeNull();
    expect(api.callsTo(DELETE)).toEqual([]);
    expect(api.callsTo(MOVE)).toEqual([]);
  });
});

describe('the status word and Archive', () => {
  it('sets the status word, keeps focus on the choice, and shows it in the tab and the tree', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    const status = screen.getByRole('combobox', { name: 'Status' });
    expect(status).toHaveProperty('value', 'stable');
    // The three words the contract takes; never `other`.
    expect(
      within(status)
        .getAllByRole('option')
        .map((option) => option.textContent),
    ).toEqual(['draft', 'stable', 'deprecated']);
    status.focus();
    await userEvent.selectOptions(status, 'deprecated');
    await screen.findByText('The status is now deprecated.');
    expect(api.callsTo(LIFE)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
        item_id: NOTE,
        status: 'deprecated',
      },
    ]);
    expect(document.activeElement).toBe(screen.getByRole('combobox', { name: 'Status' }));
    await waitFor(() =>
      expect(screen.getByRole('combobox', { name: 'Status' })).toHaveProperty(
        'value',
        'deprecated',
      ),
    );
    expect(await within(pane()).findByText(/status deprecated/)).toBeTruthy();
    expect(await row(/Meeting notes, type meeting-note, status deprecated/)).toBeTruthy();
    expect(api.violations).toEqual([]);
  });

  it('archives: the item leaves the folders, its tab stays and offers Unarchive', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    const archive = button('Archive');
    archive.focus();
    await userEvent.click(archive);
    await screen.findByText(/Archived. It no longer shows in the folders/);
    expect(api.callsTo(LIFE)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
        item_id: NOTE,
        archived: true,
      },
    ]);
    const unarchive = await screen.findByRole('button', { name: 'Unarchive' });
    expect(document.activeElement).toBe(unarchive);
    await waitFor(() =>
      expect(within(tree()).queryByRole('treeitem', { name: /Meeting notes/ })).toBeNull(),
    );
    expect(within(pane()).getByText(/· archived/)).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeTruthy();
    await waitFor(() => expect(unarchive.getAttribute('aria-disabled')).toBe('false'));
    await userEvent.click(unarchive);
    await screen.findByText('Unarchived. It shows in the folders again.');
    expect(api.callsTo(LIFE).at(1)).toMatchObject({ archived: false, base_revision: FIRST });
    expect(await row(/Meeting notes/)).toBeTruthy();
  });

  it('shows another status word as it is, and does not offer it as a choice', async () => {
    const other = { ...sourceSummary, status: 'other' as const };
    service({
      [ITEM]: (body) => ok(itemDocument({ ...other, revision: revisionAsked(body, HEAD) })),
      [READ]: () => ok(readResponse(other, 'Text of the report')),
    });
    renderAt(`/w/${WS}/i/${SOURCE}`);
    await screen.findByText('Text of the report');
    const status = screen.getByRole('combobox', { name: 'Status' });
    const options = within(status).getAllByRole('option');
    expect(options.map((option) => option.textContent)).toEqual([
      'another word (set in the file)',
      'draft',
      'stable',
      'deprecated',
    ]);
    expect(options.at(0)).toHaveProperty('disabled', true);
  });

  it('reads a refused status change as refused: the word shown is the one the service holds', async () => {
    const { api } = service({
      [LIFE]: () => refuse(501, 'not_implemented', 'set_lifecycle is not implemented'),
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'draft');
    expect(await alertText()).toContain('Changing “Meeting notes”: Not available in this build.');
    expect(screen.getByRole('combobox', { name: 'Status' })).toHaveProperty('value', 'stable');
    expect(screen.queryByText(/The status is now/)).toBeNull();
    expect(api.callsTo(LIFE)).toHaveLength(1);
  });
});

describe('a refusal that names a field', () => {
  it('puts each step of the field into words, the innermost first', () => {
    expect(fieldInWords('/status')).toBe('the status word');
    expect(fieldInWords('/item_id')).toBe('the item');
    expect(fieldInWords('/properties/owner')).toBe('“owner” of the properties');
    expect(fieldInWords('/rules/0/glob')).toBe('“glob” of rule 1');
    expect(fieldInWords('/rules/rules/2/date_format')).toBe('“date_format” of rule 3 of the rules');
    expect(fieldInWords('/plan/entries/0/to')).toBe('the name after of rename 1 of the preview');
    // A list this table has no word for, and a name it does not know (one of an object's own too).
    expect(fieldInWords('/tags/4')).toBe('entry 5 of “tags”');
    expect(fieldInWords('/constructor')).toBe('“constructor”');
    expect(fieldInWords('')).toBe('the request');
  });

  /** The one alert on the page, once it holds the service's words. */
  const refusalWith = async (words: string) => {
    const alert = await screen.findByRole('alert');
    await waitFor(() => expect(alert.textContent).toContain(words));
    return alert.textContent ?? '';
  };
  const describedBy = (element: HTMLElement) =>
    document.getElementById(element.getAttribute('aria-describedby') ?? '')?.textContent;

  it('create_item: a field the form does not draw is named in the form’s notice', async () => {
    const { api } = service({
      [CREATE]: () =>
        refuse(400, 'invalid_input', 'a note of this type needs an owner', '/properties'),
    });
    await openTree();
    await userEvent.click(button('New note'));
    await userEvent.type(field('Title'), 'Fresh');
    await userEvent.click(button('Create the note'));
    const said = await refusalWith('a note of this type needs an owner');
    expect(said).toContain('Creating the note: nothing was changed.');
    expect(said).toContain('The service did not accept the properties.');
    // Not a failure that a repeat cures: the person changes something first.
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    for (const name of ['Title', 'In folder', 'File name', 'Type']) {
      expect(field(name).getAttribute('aria-invalid'), name).toBe('false');
    }
    expect(screen.queryByText(/Created the note/)).toBeNull();
    expect(api.callsTo(CREATE)).toHaveLength(1);
  });

  it('create_folder: shown at the name for the folder, and in the notice for any other field', async () => {
    let refusedField = '/folder';
    const { api } = service({
      [FOLDER]: () => refuse(400, 'invalid_input', 'this folder cannot be made', refusedField),
    });
    renderAt(`/w/${WS}`);
    await row('notes');
    await userEvent.click(button('New folder'));
    await userEvent.type(field('Folder name'), 'archive');
    await userEvent.keyboard('{Enter}');
    const name = field('Folder name');
    await waitFor(() => expect(name.getAttribute('aria-invalid')).toBe('true'));
    expect(describedBy(name)).toBe('The service refused this: this folder cannot be made');
    expect(screen.queryByRole('alert')).toBeNull();
    // A folder has no title: a refusal about one is not at any field of this form.
    refusedField = '/title';
    await userEvent.type(name, '2');
    await userEvent.keyboard('{Enter}');
    const said = await refusalWith('this folder cannot be made');
    expect(said).toContain('Creating the folder: nothing was changed.');
    expect(said).toContain('The service did not accept the title.');
    expect(name.getAttribute('aria-invalid')).toBe('false');
    expect(api.callsTo(FOLDER)).toHaveLength(2);
  });

  it('move_item: shown at the name for the destination, and in the notice for any other field', async () => {
    let refusedField = '/destination';
    service({
      [MOVE]: () => refuse(400, 'invalid_input', 'an index file keeps its name', refusedField),
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    const name = field('File name');
    await waitFor(() => expect(name.getAttribute('aria-invalid')).toBe('true'));
    expect(describedBy(name)).toBe('The service refused this: an index file keeps its name');
    expect(screen.queryByRole('alert')).toBeNull();
    refusedField = '/item_id';
    await userEvent.type(name, 'x');
    await userEvent.keyboard('{Enter}');
    const said = await refusalWith('an index file keeps its name');
    expect(said).toContain('Renaming or moving this item: nothing was changed.');
    expect(said).toContain('The service did not accept the item.');
    expect(screen.queryByText(/is now at/)).toBeNull();
  });

  it('delete_item: the refusal is said in the question, and the item stays', async () => {
    const { api } = service({
      [DELETE]: () =>
        refuse(400, 'invalid_input', 'the folder index cannot be deleted', '/item_id'),
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    await userEvent.click(button('Delete it'));
    const said = await refusalWith('the folder index cannot be deleted');
    expect(said).toContain('Deleting this item: nothing was changed.');
    expect(said).toContain('The service did not accept the item.');
    expect(screen.getByRole('group', { name: 'Delete “Meeting notes”?' })).toBeTruthy();
    expect(screen.queryByText(/You deleted this item/)).toBeNull();
    expect(screen.getByText('Text of Meeting notes')).toBeTruthy();
    expect(api.callsTo(DELETE)).toHaveLength(1);
  });

  it('set_lifecycle: the refusal is said under the status choice, and the word stays', async () => {
    const { api } = service({
      [LIFE]: () =>
        refuse(400, 'invalid_input', 'this item cannot be given that status word', '/status'),
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'draft');
    const said = await refusalWith('this item cannot be given that status word');
    expect(said).toContain('Changing “Meeting notes”: nothing was changed.');
    expect(said).toContain('The service did not accept the status word.');
    expect(screen.getByRole('combobox', { name: 'Status' })).toHaveProperty('value', 'stable');
    expect(screen.queryByText(/The status is now/)).toBeNull();
    expect(api.callsTo(LIFE)).toHaveLength(1);
  });
});

describe('the draft of an item the person deleted', () => {
  const DISCARD = '/api/items/discard-draft';
  const leftText = () =>
    screen.queryByRole('textbox', { name: 'Text of your draft of “Meeting notes” (read-only)' });
  const draftsPanel = () => screen.getByRole('region', { name: /^Drafts/ });
  /** A service that holds a draft of the note until it is discarded. */
  const withDraft = (overrides: Record<string, Handler> = {}) => {
    const held = [NOTE];
    return service(
      {
        [DISCARD]: () => {
          held.length = 0;
          return ok(draftOf(NOTE));
        },
        ...overrides,
      },
      held,
    );
  };

  it('says in the question what the tab then shows, and the tab shows the draft read-only', async () => {
    const { api } = withDraft();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    const question = screen.getByRole('group', { name: 'Delete “Meeting notes”?' });
    expect(question.textContent).toContain('Deleting the item does not delete your draft');
    expect(question.textContent).toContain(
      'The deleted item’s tab, which the Drafts list opens too, shows the draft read-only, to copy from, and offers to discard it.',
    );
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    // What the question said: the draft's text is on the page, and cannot be changed there.
    const text = leftText();
    expect(text).toHaveProperty('value', 'My draft');
    expect(text).toHaveProperty('readOnly', true);
    expect(deletedNotice(pane())?.textContent).toContain(
      'Your draft of it is below, read-only: it can be copied from and discarded, but not snapshotted.',
    );
    expect(within(pane()).getByText(/This is the draft the service holds for you\./)).toBeTruthy();
    // The editor is gone: nothing here can be typed into or saved.
    expect(screen.queryByRole('textbox', { name: 'Text of Meeting notes' })).toBeNull();
    // The Drafts list names the draft as the tab does, not by a piece of its identity.
    expect(
      within(draftsPanel()).getByRole('button', { name: 'Meeting notes (deleted)' }),
    ).toBeTruthy();
    expect(api.callsTo(DISCARD)).toEqual([]);
  });

  it('discards the draft after its question, and puts focus on "Close this tab"', async () => {
    const { api } = withDraft();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    await userEvent.click(within(pane()).getByRole('button', { name: 'Discard draft' }));
    expect(document.activeElement).toBe(button('Keep the draft'));
    await userEvent.keyboard('{Enter}');
    expect(document.activeElement).toBe(
      within(pane()).getByRole('button', { name: 'Discard draft' }),
    );
    expect(api.callsTo(DISCARD)).toEqual([]);
    await userEvent.click(within(pane()).getByRole('button', { name: 'Discard draft' }));
    await userEvent.click(button('Discard the draft'));
    await waitFor(() => expect(leftText()).toBeNull());
    expect(api.callsTo(DISCARD)).toEqual([
      { workspace_id: WS, item_id: NOTE, idempotency_key: expect.stringMatching(UUID) },
    ]);
    expect(document.activeElement).toBe(
      within(pane()).getByRole('button', { name: 'Close this tab' }),
    );
    expect(deletedNotice(pane())?.textContent).not.toContain('Your draft');
    expect(screen.getByRole('tab', { name: 'Meeting notes (deleted)' })).toBeTruthy();
  });

  it('keeps text that was typed and never saved on the page, read-only, and says it is only there', async () => {
    const { typeInto } = await import('./support/editor');
    // As the service answers: the item is not at the head, and the person has no draft of it.
    service({
      [SAVE]: () => refuse(404, 'not_found', 'no such item at the head'),
      [DISCARD]: () => refuse(404, 'not_found', 'the caller has no draft of this item'),
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    typeInto(await findTextBox(/Text of Meeting notes/), '\nWORDS-I-TYPED');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    const question = screen.getByRole('group', { name: 'Delete “Meeting notes”?' });
    expect(question.textContent).toContain(
      'You have text of this item that is not saved as a draft. After the delete it can no longer be saved',
    );
    expect(question.textContent).toContain('The deleted item’s tab shows it read-only');
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    const text = leftText();
    expect(text).toHaveProperty('readOnly', true);
    expect((text as HTMLTextAreaElement).value).toContain('WORDS-I-TYPED');
    expect(
      within(pane()).getByText(/Not all of it is saved as a draft, and it can no longer be saved/),
    ).toBeTruthy();
    await userEvent.click(within(pane()).getByRole('button', { name: 'Discard draft' }));
    await userEvent.click(button('Discard the draft'));
    await waitFor(() => expect(leftText()).toBeNull());
    expect(document.body.textContent).not.toContain('WORDS-I-TYPED');
    expect(document.activeElement).toBe(
      within(pane()).getByRole('button', { name: 'Close this tab' }),
    );
  });

  it('shows a draft that was not open when its item was deleted, read where the item last was', async () => {
    // The item cannot be read at first: the Drafts list could not learn its title, nor its draft.
    let failing = true;
    const own = withDraft({
      [ITEM]: (body) => {
        if (failing) return refuse(500, 'internal', 'the store broke');
        const item = { ...noteSummary, revision: revisionAsked(body, HEAD) };
        return body.item_id === NOTE && revisionAsked(body, HEAD) === HEAD
          ? ok({
              ...itemDocument(item),
              draft: {
                body: 'My draft',
                properties: itemDocument(item).properties,
                draft: draftOf(NOTE),
              },
            })
          : refuse(404, 'not_found', 'no such item at this revision');
      },
    });
    await openTree();
    const note = await row(/Meeting notes/);
    act(() => note.focus());
    await userEvent.keyboard('{Delete}');
    await userEvent.click(button('Delete it'));
    await waitFor(() =>
      expect(within(tree()).queryByRole('treeitem', { name: /Meeting notes/ })).toBeNull(),
    );
    // The Drafts list still holds the draft, and opens the deleted item's tab.
    const askedBefore = own.api.callsTo(ITEM).filter((body) => body.item_id === NOTE).length;
    await userEvent.click(
      await within(draftsPanel()).findByRole('button', { name: 'Meeting notes (deleted)' }),
    );
    await within(pane()).findByText(/You deleted this item/);
    // Whether there is a draft could not be read: that is said, with its retry.
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Looking for your draft of this item');
    // The tab asked for it itself: the failure is its own, not one left from before.
    expect(own.api.callsTo(ITEM).filter((body) => body.item_id === NOTE).length).toBeGreaterThan(
      askedBefore,
    );
    expect(leftText()).toBeNull();
    failing = false;
    await userEvent.click(button('Try again'));
    await waitFor(() => expect(leftText()).toHaveProperty('value', 'My draft'));
    // Asked where the item last was, never at the head that no longer holds it.
    const asked = own.api
      .callsTo(ITEM)
      .filter((body) => body.item_id === NOTE)
      .map((body) => revisionAsked(body, ''));
    expect(asked.every((revision) => revision === HEAD)).toBe(true);
  });

  it('shows no draft part for a deleted item the person has no draft of', async () => {
    service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    await findTextBox(/Text of Meeting notes/);
    // Editing was started and nothing typed: there is no draft to speak of.
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    expect(leftText()).toBeNull();
    expect(screen.queryByRole('button', { name: 'Discard draft' })).toBeNull();
    expect(deletedNotice(pane())?.textContent).not.toContain('Your draft');
  });
});

describe('a draft of an item the person changes themselves', () => {
  const statusLine = () =>
    within(pane())
      .getAllByRole('status')
      .map((each) => each.textContent ?? '')
      .join(' ');
  const FOLLOWS = 'Your draft of this item follows the change: it now starts from this version';
  /** Open the note on its editor, type, and wait for the save of what was typed. */
  async function editAndSave(api: ReturnType<typeof mockApi>, words: string) {
    const { typeInto } = await import('./support/editor');
    const before = api.callsTo(SAVE).length;
    typeInto(await findTextBox(/Text of Meeting notes/), words);
    await waitFor(() => expect(api.callsTo(SAVE).length).toBeGreaterThan(before), {
      timeout: 4000,
    });
    await waitFor(() => expect(within(pane()).getByText(/Saved as a draft at/)).toBeTruthy());
    return api.callsTo(SAVE).at(-1) ?? {};
  }
  const headerOf = (save: Json) => (save.properties as Record<string, unknown>).okf_jawn;
  /** A moment in which a follow that has nothing to wait for has ended. */
  const settled = () => act(() => new Promise<void>((resolve) => setTimeout(resolve, 50)));

  it('carries the header as the item has it at the head shown, through Archive and after it', async () => {
    const { api, state } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    const first = await editAndSave(api, '\nfirst words');
    expect(first).toMatchObject({ base_revision: HEAD });
    expect(headerOf(first)).toEqual({ id: NOTE });

    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    // The draft followed: saved once more, on the revision the change made, with its header.
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    const followed = api.callsTo(SAVE).at(-1) ?? {};
    expect(api.callsTo(SAVE)).toHaveLength(2);
    expect(followed).toMatchObject({ base_revision: FIRST, body: first.body });
    expect(headerOf(followed)).toEqual({ id: NOTE, archived: true });
    // The header is where it was among the properties, and nothing else of the draft changed.
    expect(Object.keys(followed.properties as object)).toEqual(
      Object.keys(first.properties as object),
    );

    const later = await editAndSave(api, '\nmore words');
    expect(headerOf(later)).toEqual({ id: NOTE, archived: true });
    expect(later).toMatchObject({ base_revision: FIRST });
    // The service, which refuses a header that is not the stored one, refused none.
    expect(state.refusedSaves).toBe(0);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(api.violations).toEqual([]);
  }, 20000);

  it('takes the status word the person set, so the draft never puts an old one back', async () => {
    const { api, state } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    const first = await editAndSave(api, '\nfirst words');
    expect((first.properties as Record<string, unknown>).status).toBeUndefined();
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'deprecated');
    await waitFor(() => expect(statusLine()).toContain('The status is now deprecated.'));
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    const followed = api.callsTo(SAVE).at(-1) ?? {};
    expect(followed).toMatchObject({ base_revision: FIRST, body: first.body });
    expect(followed.properties).toEqual({ ...(first.properties as object), status: 'deprecated' });
    // Archive sets no status word: the draft keeps the one it has.
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    const archived = api.callsTo(SAVE).at(-1) ?? {};
    expect((archived.properties as Record<string, unknown>).status).toBe('deprecated');
    // The header changed where it stood: no property moved.
    expect(Object.keys(archived.properties as object)).toEqual(['type', 'okf_jawn', 'status']);
    expect(headerOf(archived)).toEqual({ id: NOTE, archived: true });
    expect(archived).toMatchObject({ base_revision: MADE.at(1) });
    expect(state.refusedSaves).toBe(0);
  }, 20000);

  it('leaves a status word the person typed in the draft when the change sets none', async () => {
    // The draft holds a status word of its own; a move does not touch it.
    const { api } = service({}, [NOTE]);
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const yaml = await findTextBox('Properties as YAML');
    const { replaceText } = await import('./support/editor');
    replaceText(yaml, 'type: meeting-note\nstatus: draft\n');
    await waitFor(() => expect(api.callsTo(SAVE)).toHaveLength(1), { timeout: 4000 });
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(statusLine()).toContain('is now at notes/minutes.md'));
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    const followed = api.callsTo(SAVE).at(-1) ?? {};
    expect(followed).toMatchObject({ base_revision: FIRST, body: 'My draft' });
    expect((followed.properties as Record<string, unknown>).status).toBe('draft');
  }, 20000);

  it('keeps the base of a draft started on an older version, and says its Snapshot will ask', async () => {
    const OLDER = 'b'.repeat(40);
    const { api } = service({}, [NOTE], OLDER);
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'draft');
    await waitFor(() => expect(statusLine()).toContain('The status is now draft.'));
    await waitFor(() =>
      expect(statusLine()).toContain(
        'It was started on an older version than the one you changed, so its next Snapshot will first ask whether to keep your text.',
      ),
    );
    expect(statusLine()).not.toContain(FOLLOWS);
    // Saved once, with the status word and the header, on the base it had: no silent rebase.
    expect(api.callsTo(SAVE)).toHaveLength(1);
    expect(api.callsTo(SAVE).at(0)).toMatchObject({
      base_revision: OLDER,
      properties: { status: 'draft', okf_jawn: { id: NOTE } },
    });
  });

  it('makes no draft of the change when editing was started and nothing typed', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    await findTextBox(/Text of Meeting notes/);
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'deprecated');
    await waitFor(() => expect(statusLine()).toContain('The status is now deprecated.'));
    await settled();
    expect(statusLine()).not.toContain('Your draft');
    expect(api.callsTo(SAVE)).toEqual([]);
    expect(within(pane()).getByText('No changes to save.')).toBeTruthy();
  });

  it('follows a rename made from the folders for a draft whose tab is not open', async () => {
    const { api, state } = service({}, [NOTE]);
    await openTree();
    const note = await row(/Meeting notes/);
    act(() => note.focus());
    await userEvent.keyboard('{F2}');
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    const said = await screen.findByText(/is now at notes\/minutes.md\./);
    await waitFor(() => expect(said.textContent).toContain(FOLLOWS));
    expect(api.callsTo(SAVE)).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        base_revision: FIRST,
        body: 'My draft',
        properties: { type: 'meeting-note', okf_jawn: { id: NOTE } },
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
    expect(state.refusedSaves).toBe(0);
  });

  it('asks nothing of the drafts for an item the person has no draft of', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    await settled();
    expect(statusLine()).not.toContain('Your draft');
    expect(api.callsTo(SAVE)).toEqual([]);
  });

  it('says so when the draft could not be saved on the new version', async () => {
    let failing = false;
    const { api } = service(
      {
        [SAVE]: (body) =>
          failing
            ? refuse(500, 'internal', 'the store broke')
            : ok(draftOf(text(body, 'item_id'), text(body, 'base_revision'))),
      },
      [NOTE],
    );
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    failing = true;
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'draft');
    await waitFor(() => expect(statusLine()).toContain('The status is now draft.'));
    await waitFor(() =>
      expect(statusLine()).toContain(
        'Your draft of this item is kept, but it could not be saved on this version: its own notice says why.',
      ),
    );
    expect(api.callsTo(SAVE)).toHaveLength(1);
    // The change itself was made, and reads as made; the draft's own notice has the retry.
    expect(screen.getByRole('combobox', { name: 'Status' })).toHaveProperty('value', 'draft');
    expect((await screen.findByRole('alert')).textContent).toContain('Saving this draft');
  });

  it('carries the header of the head shown after a Refresh to someone else’s change', async () => {
    const { api, state, archiveElsewhere } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    await editAndSave(api, '\nfirst words');
    archiveElsewhere(NOTE);
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    const later = await editAndSave(api, '\nmore words');
    expect(headerOf(later)).toEqual({ id: NOTE, archived: true });
    // Not the person's own change: the draft keeps its base, and its Snapshot will ask.
    expect(later).toMatchObject({ base_revision: HEAD });
    expect(state.refusedSaves).toBe(0);
  }, 20000);
});

describe('a press repeated while a change is in flight', () => {
  /** A handler that answers when `release` is called. */
  const held = () => {
    const gate: { release: (reply: Reply) => void } = { release: () => {} };
    const handler: Handler = () =>
      new Promise<Reply>((resolve) => {
        gate.release = resolve;
      });
    return { gate, handler };
  };
  const made = (revision: string) => ok({ revision, receipt_id: RECEIPT, warnings: [] });

  it('sends one request for two presses of a form’s button and Enter', async () => {
    const { gate, handler } = held();
    const { api } = service({ [FOLDER]: handler });
    await openTree();
    await userEvent.click(button('New folder'));
    await userEvent.type(field('Folder name'), 'fresh');
    await userEvent.click(button('Create the folder'));
    const working = screen.getByRole('button', { name: 'Creating…' });
    expect(working.getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(working);
    await userEvent.keyboard('{Enter}');
    expect(api.callsTo(FOLDER)).toHaveLength(1);
    await act(async () => {
      gate.release(made(FIRST));
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.queryByRole('group', { name: 'New folder' })).toBeNull());
    expect(api.callsTo(FOLDER)).toHaveLength(1);
  });

  it('sends neither a second status nor Archive while a status change is in flight', async () => {
    const { gate, handler } = held();
    const { api } = service({ [LIFE]: handler });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    const status = screen.getByRole('combobox', { name: 'Status' });
    await userEvent.selectOptions(status, 'deprecated');
    await waitFor(() => expect(api.callsTo(LIFE)).toHaveLength(1));
    expect(status.getAttribute('aria-disabled')).toBe('true');
    // Another word chosen meanwhile, and Archive pressed: neither is sent.
    await userEvent.selectOptions(status, 'draft');
    const archive = within(pane()).getByRole('button', { name: 'Archive' });
    expect(archive.getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(archive);
    expect(api.callsTo(LIFE)).toEqual([expect.objectContaining({ status: 'deprecated' })]);
    await act(async () => {
      gate.release(made(FIRST));
      await Promise.resolve();
    });
    await screen.findByText(/The status is now deprecated\./);
    expect(api.callsTo(LIFE)).toHaveLength(1);
  });
});

describe('what is asked about an item the person deleted', () => {
  const SOURCES = '/api/reads/get-sources';
  const aboutNoteAt = (api: ReturnType<typeof mockApi>, path: string, revision: string) =>
    api
      .callsTo(path)
      .filter((body) => body.item_id === NOTE && revisionAsked(body, '') === revision);

  it('asks neither its text nor its sources at the head that no longer holds it', async () => {
    const { api } = service();
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    // The person chose to read: the reader is not waiting to learn whether there is a draft.
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    await findTextBox(/Text of Meeting notes/);
    await userEvent.click(within(pane()).getByRole('button', { name: 'Read the committed text' }));
    await screen.findByText('Text of Meeting notes');
    // Asked at the head shown before the delete, so the mock is seen to be asked at all.
    await waitFor(() => expect(aboutNoteAt(api, SOURCES, HEAD).length).toBeGreaterThan(0));
    expect(aboutNoteAt(api, READ, HEAD).length).toBeGreaterThan(0);
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    // Time for a request that should not be made to be made.
    await act(() => new Promise((resolve) => setTimeout(resolve, 100)));
    expect(aboutNoteAt(api, READ, FIRST)).toEqual([]);
    expect(aboutNoteAt(api, SOURCES, FIRST)).toEqual([]);
    expect(aboutNoteAt(api, ITEM, FIRST)).toEqual([]);
  });

  it('remembers a deleted item for its workspace only', () => {
    const ELSEWHERE = '77777777-7777-4777-8777-777777777777';
    act(() => {
      useExplorerStore.setState({ deleted: {} });
      useExplorerStore
        .getState()
        .markDeleted(WS, NOTE, { title: 'Meeting notes', at: HEAD, made: FIRST });
    });
    const here = renderHook(() => useDeleted(WS, NOTE));
    const there = renderHook(() => useDeleted(ELSEWHERE, NOTE));
    expect(here.result.current).toBe('Meeting notes');
    expect(there.result.current).toBeUndefined();
  });
});

describe('the item’s controls while its pane reads it again', () => {
  it('send neither a status nor Archive for the item as it was last read', async () => {
    const waiting: { release: () => void } = { release: () => {} };
    let hold = false;
    const { api, moveHead } = service({
      [ITEM]: (body) => {
        const reply = ok(itemDocument({ ...noteSummary, revision: revisionAsked(body, HEAD) }));
        if (!hold || revisionAsked(body, HEAD) === HEAD) return reply;
        return new Promise<Reply>((resolve) => {
          waiting.release = () => resolve(reply);
        });
      },
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    // Someone else moved the head; after Refresh the pane reads the item again, and waits.
    hold = true;
    moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    const status = screen.getByRole('combobox', { name: 'Status' });
    const archive = within(pane()).getByRole('button', { name: 'Archive' });
    await waitFor(() => expect(archive.getAttribute('aria-disabled')).toBe('true'));
    expect(status.getAttribute('aria-disabled')).toBe('true');
    await userEvent.selectOptions(status, 'deprecated');
    await userEvent.click(archive);
    expect(api.callsTo(LIFE)).toEqual([]);
    // Read again, the controls act on the item as it is.
    await act(async () => {
      waiting.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(archive.getAttribute('aria-disabled')).toBe('false'));
    await userEvent.click(archive);
    await waitFor(() => expect(api.callsTo(LIFE)).toHaveLength(1));
    expect(api.callsTo(LIFE).at(0)).toMatchObject({ archived: true, base_revision: FIRST });
    // The change is followed to its end before the test leaves. It reads the item at the revision
    // it made; left held, that read is stopped when the page goes and asked again after it, of
    // whatever service the next tests have.
    hold = false;
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('dddddddddd'),
    );
    await act(async () => {
      waiting.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(archive.getAttribute('aria-disabled')).toBe('false'));
  });
});

describe('state that must not outlive its truth', () => {
  const headLine = () => screen.getByText(/Showing the workspace as of/).textContent ?? '';
  const refresh = async (shows: string) => {
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain(shows));
  };
  async function deleteOpenNote() {
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
  }

  it('forgets that an item was deleted when a head the page refreshes to holds it again', async () => {
    const { restoreElsewhere } = service();
    await deleteOpenNote();
    // Someone brings the item back, under its id.
    restoreElsewhere(noteSummary);
    await refresh('dddddddddd');
    // The tab reads it as any item: its text, its own name, no word of a delete.
    expect(await screen.findByText('Text of Meeting notes')).toBeTruthy();
    expect(screen.queryByText(/You deleted this item/)).toBeNull();
    expect(screen.getByRole('tab', { name: 'Meeting notes' })).toBeTruthy();
    expect(await row(/Meeting notes/)).toBeTruthy();
    expect(within(pane()).getByRole('button', { name: 'Delete' })).toBeTruthy();
  });

  it('goes on saying so while a later head does not hold it, and shows no failure for having asked', async () => {
    const { api, moveHead } = service();
    await deleteOpenNote();
    moveHead();
    await refresh('dddddddddd');
    // Asked once at the new head whether it is back; it is not.
    const second = MADE.at(1) ?? '';
    await waitFor(() =>
      expect(
        api
          .callsTo(ITEM)
          .filter((body) => body.item_id === NOTE && revisionAsked(body, '') === second),
      ).toHaveLength(1),
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(within(pane()).getByText(/You deleted this item/)).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Meeting notes (deleted)' })).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('does not adopt the revision of an answer that comes after the head shown moved on', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const holder: { commit: () => string } = { commit: () => '' };
    const { api, moveHead } = service({
      [FOLDER]: () =>
        new Promise<Reply>((resolve) => {
          gate.release = () =>
            resolve(ok({ revision: holder.commit(), receipt_id: RECEIPT, warnings: [] }));
        }),
    });
    holder.commit = moveHead;
    renderAt(`/w/${WS}`);
    await row('notes');
    await userEvent.click(button('New folder'));
    await userEvent.type(field('Folder name'), 'fresh');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(api.callsTo(FOLDER)).toHaveLength(1));
    // While the change is asked, the person refreshes to someone else's newer head.
    moveHead();
    await refresh('cccccccccc');
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.queryByRole('group', { name: 'New folder' })).toBeNull());
    // The answer names revision d; the page showed c when it came, and goes on showing c.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(headLine()).toContain('cccccccccc');
    expect(headLine()).not.toContain('dddddddddd');
    // That a newer version exists is said by the shell, with its Refresh.
    expect(await screen.findByText(/A newer version of this workspace exists/)).toBeTruthy();
  });
});

describe('a draft whose item is ahead of the page, or whose own save is in the way', () => {
  const statusLine = () =>
    within(pane())
      .getAllByRole('status')
      .map((each) => each.textContent ?? '')
      .join(' ');
  const headLine = () => screen.getByText(/Showing the workspace as of/).textContent ?? '';
  const headerOf = (save: Json | undefined) =>
    (save?.properties as Record<string, unknown> | undefined)?.okf_jawn;
  const FOLLOWS = 'Your draft of this item follows the change: it now starts from this version';
  const typeMore = async (words: string) => {
    const { typeInto } = await import('./support/editor');
    typeInto(await findTextBox(/Text of Meeting notes/), words);
  };
  /** Open the note in its editor. */
  async function editing() {
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    await findTextBox(/Text of Meeting notes/);
  }
  /** A service whose saves of the draft are held until the test lets them answer. */
  function holdingSaves() {
    const held: Array<{ body: Json; answer: () => void }> = [];
    const gate = { holding: false };
    const own = service({
      [SAVE]: (body) => {
        const reply = ok(draftOf(NOTE, text(body, 'base_revision')));
        if (!gate.holding) return reply;
        return new Promise<Reply>((resolve) => {
          held.push({ body, answer: () => resolve(reply) });
        });
      },
    });
    return { ...own, held, gate };
  }

  it('says that the item was changed elsewhere, and that Refresh lets the save go on, when a save is refused for the header', async () => {
    const { api, state, archiveElsewhere } = service();
    await editing();
    archiveElsewhere(NOTE);
    await typeMore('\nwords after their archive');
    await waitFor(() => expect(state.refusedSaves).toBe(1), { timeout: 4000 });
    const alert = await within(pane()).findByRole('alert');
    expect(alert.textContent).toContain(
      'Saving this draft: This item was changed in the workspace since this page was refreshed.',
    );
    expect(alert.textContent).toContain(
      'Nothing was saved; your text is kept here. Press “Refresh” at the top of the page, then “Try again” here: the save then goes on.',
    );
    // Not the words for a request the person got wrong.
    expect(alert.textContent).not.toContain('The request was not valid');
    expect(alert.textContent).not.toContain('Check the address');
    expect(textIn(await findTextBox(/Text of Meeting notes/))).toContain(
      'words after their archive',
    );
    // Refreshed, the save goes on with the header the item has now.
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    await userEvent.click(await within(pane()).findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(within(pane()).getByText(/Saved as a draft at/)).toBeTruthy());
    expect(headerOf(api.callsTo(SAVE).at(-1))).toEqual({ id: NOTE, archived: true });
    expect(state.refusedSaves).toBe(1);
    expect(within(pane()).queryByRole('alert')).toBeNull();
  }, 20000);

  it('says a change as done when the service has answered it, and the draft’s follow after it, when its save has answered', async () => {
    const { api, held, gate } = holdingSaves();
    await editing();
    gate.holding = true;
    await typeMore('\nslow words');
    await waitFor(() => expect(held).toHaveLength(1), { timeout: 4000 });
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    // The change is done: said, shown in the head line, and its controls are available again,
    // though the draft's save has not answered.
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    expect(statusLine()).not.toContain('Changing…');
    expect(statusLine()).not.toContain('Your draft');
    expect(headLine()).toContain('cccccccccc');
    const unarchive = await within(pane()).findByRole('button', { name: 'Unarchive' });
    expect(unarchive.getAttribute('aria-disabled')).not.toBe('true');
    expect(api.callsTo(LIFE)).toHaveLength(1);
    // The save answers: the draft follows, once, on the revision the change made, and says so.
    await act(async () => {
      held.at(0)?.answer();
      await Promise.resolve();
    });
    await waitFor(() => expect(held).toHaveLength(2));
    expect(statusLine()).not.toContain('Your draft');
    await act(async () => {
      held.at(1)?.answer();
      await Promise.resolve();
    });
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    expect(statusLine()).toContain('Archived.');
    expect(held.map((save) => [save.body.base_revision, headerOf(save.body)])).toEqual([
      [HEAD, { id: NOTE }],
      [FIRST, { id: NOTE, archived: true }],
    ]);
  }, 20000);

  it('says a move as done when the service has answered it, before the draft has followed', async () => {
    const { api, held, gate } = holdingSaves();
    await editing();
    gate.holding = true;
    await typeMore('\nslow words');
    await waitFor(() => expect(held).toHaveLength(1), { timeout: 4000 });
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    // Done and said, with the form closed, though the draft's save has not answered.
    await waitFor(() => expect(statusLine()).toContain('is now at notes/minutes.md'));
    expect(screen.queryByRole('group', { name: /^Rename or move/ })).toBeNull();
    expect(statusLine()).not.toContain('Your draft');
    expect(api.callsTo(MOVE)).toHaveLength(1);
    gate.holding = false;
    await act(async () => {
      held.at(0)?.answer();
      await Promise.resolve();
    });
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    expect(statusLine()).toContain('is now at notes/minutes.md');
    expect(api.callsTo(SAVE).at(-1)).toMatchObject({ base_revision: FIRST });
  }, 20000);

  it('says the draft’s follow only of the change last made', async () => {
    const { held, gate } = holdingSaves();
    await editing();
    gate.holding = true;
    await typeMore('\nslow words');
    await waitFor(() => expect(held).toHaveLength(1), { timeout: 4000 });
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    // A second change before the draft has followed the first.
    await userEvent.click(await within(pane()).findByRole('button', { name: 'Unarchive' }));
    await waitFor(() => expect(statusLine()).toContain('Unarchived.'));
    gate.holding = false;
    await act(async () => {
      held.at(0)?.answer();
      await Promise.resolve();
    });
    await waitFor(() => expect(statusLine()).toContain(FOLLOWS));
    // One sentence about the draft, after the words of the last change; none of the first.
    expect(statusLine()).toContain('Unarchived.');
    expect(statusLine()).not.toContain('Archived. It no longer shows');
    expect(statusLine().split(FOLLOWS)).toHaveLength(2);
  }, 20000);

  it('says nothing of a draft’s follow once something else was said in its place', async () => {
    const held: Array<() => void> = [];
    service(
      {
        [SAVE]: (body) =>
          new Promise<Reply>((resolve) => {
            held.push(() => resolve(ok(draftOf(NOTE, text(body, 'base_revision')))));
          }),
      },
      [NOTE],
    );
    const line = () => document.querySelector('.tree-said')?.textContent ?? '';
    await openTree();
    const note = await row(/Meeting notes/);
    act(() => note.focus());
    await userEvent.keyboard('{F2}');
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(line()).toContain('is now at notes/minutes.md.'));
    // The person's draft of the note follows the move: its save is under way.
    await waitFor(() => expect(held).toHaveLength(1));
    // Another form is opened meanwhile, which clears what was said.
    await userEvent.click(button('New folder'));
    expect(line()).toBe('');
    await act(async () => {
      held.at(0)?.();
      await Promise.resolve();
    });
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 50)));
    // The move's words do not come back with the draft's sentence.
    expect(line()).toBe('');
  }, 20000);

  it('sends nothing for the draft after "Stop" when the person changes the item, and says where the retry is', async () => {
    const { api, held, gate } = holdingSaves();
    await editing();
    gate.holding = true;
    await typeMore('\nslow words');
    await waitFor(() => expect(held).toHaveLength(1), { timeout: 4000 });
    // The person stops the save: what the editor's "Stop" does.
    act(() => stopSaving(WS, NOTE));
    await within(pane()).findByText(/You stopped this before the service answered/);
    const saves = api.callsTo(SAVE).length;
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    await waitFor(() =>
      expect(statusLine()).toContain(
        'Your draft of this item is kept, but it could not be saved on this version: its own notice says why.',
      ),
    );
    // Nothing was sent by itself: the stopped save's notice still has the retry.
    expect(api.callsTo(SAVE)).toHaveLength(saves);
    // Asked for, the save carries the change: the new base and the header.
    gate.holding = false;
    await userEvent.click(within(pane()).getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(api.callsTo(SAVE)).toHaveLength(saves + 1));
    expect(api.callsTo(SAVE).at(-1)).toMatchObject({ base_revision: FIRST });
    expect(headerOf(api.callsTo(SAVE).at(-1))).toEqual({ id: NOTE, archived: true });
  }, 20000);

  it('takes only a refusal at the application’s header for an item changed elsewhere', () => {
    const refusedAt = (field: string | undefined, code = 'invalid_input') => ({
      code,
      message: 'refused',
      ...(field === undefined ? {} : { field }),
    });
    expect(headerBehind(refusedAt('/properties/okf_jawn'))).toBe(true);
    // Another field, no field, another code, and a failure that is no answer of the service.
    expect(headerBehind(refusedAt('/properties/owner'))).toBe(false);
    expect(headerBehind(refusedAt('/body'))).toBe(false);
    expect(headerBehind(refusedAt(undefined))).toBe(false);
    expect(headerBehind(refusedAt('/properties/okf_jawn', 'conflict'))).toBe(false);
    expect(headerBehind(new Error('the network is down'))).toBe(false);
  });

  it('says nothing of a draft when the change is not on the page and nothing was typed', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const holder: { commit: () => string } = { commit: () => '' };
    const own = service({
      [LIFE]: () =>
        new Promise<Reply>((resolve) => {
          gate.release = () =>
            resolve(ok({ revision: holder.commit(), receipt_id: RECEIPT, warnings: [] }));
        }),
    });
    holder.commit = own.moveHead;
    // The editor is open, and nothing is typed: there is no draft.
    await editing();
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(own.api.callsTo(LIFE)).toHaveLength(1));
    own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 50)));
    expect(statusLine()).not.toContain('Your draft');
    expect(own.api.callsTo(SAVE)).toEqual([]);
  }, 20000);

  it('says that the draft is left on the older version when the change is not on the page', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const holder: { commit: () => string } = { commit: () => '' };
    const own = service({
      [LIFE]: () =>
        new Promise<Reply>((resolve) => {
          gate.release = () =>
            resolve(ok({ revision: holder.commit(), receipt_id: RECEIPT, warnings: [] }));
        }),
    });
    holder.commit = own.moveHead;
    await editing();
    await typeMore('\nmy words');
    await waitFor(() => expect(own.api.callsTo(SAVE)).toHaveLength(1), { timeout: 4000 });
    await userEvent.click(within(pane()).getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(own.api.callsTo(LIFE)).toHaveLength(1));
    // While the change is asked, the person refreshes to someone else's newer head.
    own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(statusLine()).toContain('Archived.'));
    await waitFor(() =>
      expect(statusLine()).toContain(
        'Your draft of this item is still on the older version this page shows. Until you refresh the workspace, saving it may be refused, and its next Snapshot will first ask whether to keep your text.',
      ),
    );
    expect(statusLine()).not.toContain(FOLLOWS);
    // The page goes on showing the version it showed, and says that the change is not on it.
    expect(headLine()).toContain('cccccccccc');
    expect(pane().textContent).toContain(
      'The change was made, but the workspace shown here moved on while it was asked',
    );
    // Nothing of the draft was sent for it.
    expect(own.api.callsTo(SAVE)).toHaveLength(1);
  }, 20000);
});

describe('two rules whose mutants survived the re-review', () => {
  const headLine = () => screen.getByText(/Showing the workspace as of/).textContent ?? '';

  it('sends no save when the header cannot be read at the head shown: the draft’s notice says why', async () => {
    const own = service({
      // The item can be read at the first head, and at no later one.
      [ITEM]: (body) =>
        revisionAsked(body, HEAD) === HEAD
          ? ok(itemDocument({ ...noteSummary, revision: HEAD }, { okf_jawn: { id: NOTE } }))
          : refuse(500, 'internal', 'the store broke'),
    });
    await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Edit' }));
    const box = await findTextBox(/Text of Meeting notes/);
    // Someone else moves the head and the person refreshes: the item cannot be read there, so
    // the header a save would carry is not known.
    own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    const { typeInto } = await import('./support/editor');
    typeInto(box, '\nwords typed after the refresh');
    const notice = await within(pane()).findByText(/Saving this draft/, {}, { timeout: 5000 });
    expect(notice.closest('.failure-notice')?.textContent).toContain('the store broke');
    // Nothing was sent with the header the session had.
    expect(own.api.callsTo(SAVE)).toEqual([]);
    expect(textIn(box)).toContain('words typed after the refresh');
  }, 20000);

  it('moves no tab to the revision of a change the page does not show', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const holder: { commit: () => string } = { commit: () => '' };
    const own = service({
      [MOVE]: () =>
        new Promise<Reply>((resolve) => {
          gate.release = () =>
            resolve(ok({ revision: holder.commit(), receipt_id: RECEIPT, warnings: [] }));
        }),
    });
    holder.commit = own.moveHead;
    const { router } = await openTree(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    const note = await row(/Meeting notes/);
    act(() => note.focus());
    await userEvent.keyboard('{F2}');
    await type('File name', 'minutes.md');
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(own.api.callsTo(MOVE)).toHaveLength(1));
    // While the move is asked, the person refreshes to someone else's newer head, and their tab
    // comes to cite that head (as a link from a search made there does).
    const theirs = own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    await act(async () => {
      await router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
        search: { rev: theirs },
        replace: true,
      });
    });
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    // The move is done: its form closes.
    await waitFor(() =>
      expect(screen.queryByRole('group', { name: /^Rename or move/ })).toBeNull(),
    );
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 50)));
    // The tab goes on citing the head shown; it is not moved to the revision the move made.
    expect(headLine()).toContain('cccccccccc');
    expect(router.state.location.search).toMatchObject({ rev: theirs });
    const view = useExplorerStore.getState().views[WS];
    expect(JSON.stringify(view)).not.toContain(MADE.at(1));
  }, 20000);
});
