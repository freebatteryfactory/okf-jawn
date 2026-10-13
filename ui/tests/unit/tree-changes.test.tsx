/**
 * Changing the tree: a new note and a new folder, a move or rename, a delete after its question,
 * the status word and Archive; what each tells the cache; and how each refusal is shown.
 *
 * The service is a small one that remembers: it keeps the tree of every revision it made, so a
 * listing or a read at a revision answers what that revision held, and a write on another base
 * than its head is a `conflict`, as the real one answers.
 */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ItemSummary } from '../../src/api/generated/types.gen';
import { resetDraftSessions } from '../../src/features/drafts/saving';
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
function service(overrides: Record<string, Handler> = {}, drafts: string[] = []) {
  const trees = new Map<string, Tree>([
    [HEAD, { items: [noteSummary, otherSummary, sourceSummary], folders: ['notes'] }],
  ]);
  const state = { head: HEAD, made: 0, permissions: writableWorkspace.permissions };
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
      const document = itemDocument(item);
      return ok(
        drafts.includes(item.id)
          ? {
              ...document,
              draft: {
                body: 'My draft',
                properties: document.properties,
                draft: draftOf(item.id),
              },
            }
          : document,
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
      ok({ items: drafts.map((id) => draftOf(id)), next_cursor: null }),
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
    expect(pathProblem(`${'a'.repeat(4092)}.md`)).toBeNull();
    expect(pathProblem(`${'a'.repeat(4094)}.md`)).toMatch(/longer than 4096/);
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

  it('warns that a draft of the item will ask at its next Snapshot', async () => {
    service({}, [NOTE]);
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Rename or move' }));
    expect(
      screen.getByText(
        /You have a draft of this item. It is kept. Because the item will have changed/,
      ),
    ).toBeTruthy();
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

  it('warns what happens to a draft, by what the service does with it', async () => {
    service({}, [NOTE]);
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.click(within(pane()).getByRole('button', { name: 'Delete' }));
    const question = screen.getByRole('group', { name: 'Delete “Meeting notes”?' });
    expect(question.textContent).toContain('Deleting the item does not delete your draft');
    expect(question.textContent).toContain('only be discarded');
    await userEvent.click(button('Delete it'));
    await within(pane()).findByText(/You deleted this item/);
    expect(deletedNotice(pane())?.textContent).toContain(
      'Your draft of it is still in your Drafts',
    );
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

  it('says what a status change means for the person’s draft of the item', async () => {
    service({}, [NOTE]);
    await openTree(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'draft');
    const said = await screen.findByText(/The status is now draft\./);
    expect(said.textContent).toContain(
      'its next Snapshot will first ask whether to keep your text',
    );
  });
});
