/**
 * The naming rules: the form over them, saving, a preview of the renames, and applying a preview
 * the person has seen. The service here keeps the rules and the tree of every revision it made,
 * and refuses an apply whose plan was made on another revision than its head.
 */
import type { RJSFSchema } from '@rjsf/utils';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import namingSchema from '../../../api/forms/naming-rules.schema.json';
import type { ItemSummary, NamingRules } from '../../src/api/generated/types.gen';
import { staleReason } from '../../src/features/conventions/NamingRules';
import { formHolds, RulesForm, rulesProblems } from '../../src/features/conventions/RulesForm';
import { type Preview, sameRules } from '../../src/features/conventions/rules';
import { resetDraftSessions } from '../../src/features/drafts/saving';
import { type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import { findTextBox, replaceText, textIn } from './support/editor';
import {
  DIGEST,
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
  WS,
  workspace,
  writableWorkspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const LIST = '/api/items/list-items';
const SCHEMA = '/api/schemas/naming-rules';
const RULES = '/api/conventions/get-rules';
const SET = '/api/conventions/set-rules';
const PREVIEW = '/api/conventions/preview-names';
const APPLY = '/api/conventions/apply-names';
const MADE = ['c', 'd', 'e'].map((digit) => digit.repeat(40));
const FIRST = MADE.at(0) ?? '';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const schema = namingSchema as RJSFSchema;

const rule: NamingRules['rules'][number] = {
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
const savedRules: NamingRules = { schema_version: 1, rules: [rule] };

afterEach(() => {
  act(() => resetDraftSessions());
  cleanup();
  vi.unstubAllGlobals();
});

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
const folderOf = (path: string) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');

/** What the service's rules do to a path in these tests: the prefix of the first rule goes first. */
const renamed = (path: string, rules: unknown): string => {
  const first = (rules as NamingRules).rules.at(0);
  const prefix = first?.prefix ?? '';
  const cut = path.lastIndexOf('/') + 1;
  return prefix === '' ? path : `${path.slice(0, cut)}${prefix}${path.slice(cut)}`;
};

function service(overrides: Record<string, Handler> = {}, rules: unknown = savedRules) {
  const trees = new Map<string, ItemSummary[]>([
    [HEAD, [noteSummary, otherSummary, sourceSummary]],
  ]);
  const state = {
    head: HEAD,
    made: 0,
    rules: new Map<string, unknown>([[HEAD, rules]]),
    permissions: writableWorkspace.permissions,
  };
  const itemsAt = (revision: string) => trees.get(revision) ?? [];
  const commit = (items: ItemSummary[], nextRules: unknown): string => {
    const next = MADE.at(state.made) ?? '';
    state.made += 1;
    trees.set(next, items);
    state.rules.set(next, nextRules);
    state.head = next;
    return next;
  };
  const current = () => state.rules.get(state.head);
  const api = mockApi({
    [OPEN]: () => ok({ ...writableWorkspace, permissions: state.permissions, head: state.head }),
    [LIST]: (body) => {
      const revision = revisionAsked(body, state.head);
      const folder = text(body, 'folder');
      const items = itemsAt(revision);
      return ok({
        folders: folder === '' ? ['notes'] : [],
        items: items
          .filter((item) => folderOf(item.path) === folder)
          .map((item) => ({ ...item, revision })),
        revision,
        next_cursor: null,
      });
    },
    '/api/items/get-item': (body) => {
      const revision = revisionAsked(body, state.head);
      const item = itemsAt(revision).find((each) => each.id === body.item_id);
      return item === undefined
        ? refuse(404, 'not_found', 'no such item at this revision')
        : ok(itemDocument({ ...item, revision }));
    },
    '/api/reads/read-item': (body) => {
      const revision = revisionAsked(body, state.head);
      const item = itemsAt(revision).find((each) => each.id === body.item_id);
      return item === undefined
        ? refuse(404, 'not_found', 'no such item at this revision')
        : ok(readResponse({ ...item, revision }, `Text of ${item.title}`));
    },
    '/api/reads/get-sources': (body) =>
      ok({ revision: revisionAsked(body, state.head), sources: [], declared: [] }),
    '/api/items/list-drafts': () => ok({ items: [], next_cursor: null }),
    '/api/types/list-types': () => ok({ items: [] }),
    [SCHEMA]: () => ok(namingSchema),
    [RULES]: (body) => ok(state.rules.get(revisionAsked(body, state.head)) ?? savedRules),
    [SET]: (body) => {
      if (text(body, 'base_revision') !== state.head) {
        return refuse(409, 'conflict', 'the workspace head moved since the base revision');
      }
      return ok({
        revision: commit(itemsAt(state.head), body.rules),
        receipt_id: RECEIPT,
        warnings: [],
      });
    },
    [PREVIEW]: (body) => {
      const revision = revisionAsked(body, state.head);
      const ids = Array.isArray(body.item_ids) ? body.item_ids : [];
      return ok({
        base_revision: revision,
        rules_digest: DIGEST,
        entries: itemsAt(revision)
          .filter((item) => ids.includes(item.id))
          .map((item) => ({
            item_id: item.id,
            from: item.path,
            to: item.kind === 'source' ? item.path : renamed(item.path, body.rules),
            warnings:
              item.id === OTHER ? [{ code: 'ambiguous_date', message: 'No date was found.' }] : [],
          })),
        duplicate_observations: [],
        warnings: [{ code: 'note', message: 'Index files are not renamed.' }],
      });
    },
    [APPLY]: (body) => {
      const plan = body.plan as {
        base_revision: string;
        entries: { item_id: string; to: string }[];
      };
      if (plan.base_revision !== state.head) {
        return refuse(409, 'conflict', 'the plan was made on another revision than the head');
      }
      const items = itemsAt(state.head).map((item) => ({
        ...item,
        path: plan.entries.find((entry) => entry.item_id === item.id)?.to ?? item.path,
      }));
      return ok({ revision: commit(items, current()), receipt_id: RECEIPT, warnings: [] });
    },
    ...overrides,
  });
  return {
    api,
    state,
    /** Someone else changes the workspace. */
    moveHead: () => commit(itemsAt(state.head), current()),
  };
}

const button = (name: string | RegExp) => screen.getByRole('button', { name });
const panel = () => screen.getByRole('region', { name: 'Naming rules' });
const prefix = () => within(panel()).getByRole('textbox', { name: /^prefix/ });
const said = () => panel().querySelector('.rules-said')?.textContent ?? '';
/** Replace the YAML and leave the text box, as a person does before pressing anything else: it is read at once. */
const typeYaml = (box: HTMLElement, text: string) => {
  replaceText(box, text);
  act(() => {
    fireEvent.blur(box);
  });
};
const writes = (api: ReturnType<typeof mockApi>) => [...api.callsTo(SET), ...api.callsTo(APPLY)];

/** Open the workspace, then the naming rules, and wait for the form. */
async function openRules(url = `/w/${WS}`) {
  const view = renderAt(url);
  await userEvent.click(await screen.findByRole('button', { name: 'Naming rules' }));
  await within(panel()).findByRole('textbox', { name: /^prefix/ });
  return view;
}

/** Give the first rule the prefix `value`. */
async function setPrefix(value: string) {
  await userEvent.clear(prefix());
  if (value.length > 0) await userEvent.type(prefix(), value);
}

async function previewNow() {
  await userEvent.click(button('Preview the renames'));
  return screen.findByRole('heading', { name: 'Preview: what would be renamed' });
}

describe('whether a preview may still be applied', () => {
  const plan = {
    base_revision: HEAD,
    rules_digest: DIGEST,
    entries: [],
    duplicate_observations: [],
    warnings: [],
  };
  const preview: Preview = { plan, rules: savedRules, folder: '', looked: 3 };

  it('holds while the head and the rules are those of the preview, whatever the key order', () => {
    const reordered = { rules: [{ ...rule }], schema_version: 1 };
    expect(staleReason(preview, HEAD, reordered, null)).toBeNull();
    expect(sameRules(savedRules, reordered)).toBe(true);
    expect(sameRules(savedRules, { ...savedRules, rules: [{ ...rule, prefix: 'x' }] })).toBe(false);
  });

  it('ends when the head moved, when the rules changed, and when the service refused the plan', () => {
    expect(staleReason(preview, FIRST, savedRules, null)).toMatch(/The workspace changed since/);
    expect(staleReason(preview, HEAD, { ...savedRules, rules: [] }, null)).toMatch(
      /The rules were changed since/,
    );
    expect(staleReason(preview, HEAD, savedRules, plan)).toMatch(
      /The service refused this preview/,
    );
    // Another plan refused earlier says nothing about this one.
    expect(staleReason(preview, HEAD, savedRules, { ...plan })).toBeNull();
  });
});

describe('the rules form', () => {
  it('keeps a key it does not know through an edit, and names it as a problem', () => {
    const onChange = vi.fn();
    const withUnknown = { ...savedRules, rules: [{ ...rule, legacy_flag: true }], note: 'kept' };
    render(<RulesForm schema={schema} value={withUnknown} onChange={onChange} />);
    fireEvent.change(screen.getByRole('textbox', { name: /^prefix/ }), { target: { value: 'x-' } });
    expect(onChange).toHaveBeenCalled();
    expect(onChange.mock.lastCall?.at(0)).toEqual({
      ...withUnknown,
      rules: [{ ...rule, legacy_flag: true, prefix: 'x-' }],
    });
    const problems = rulesProblems(schema, withUnknown);
    expect(problems.some((problem) => problem.includes('“legacy_flag”'))).toBe(true);
    expect(problems.some((problem) => problem.includes('“note”'))).toBe(true);
    expect(rulesProblems(schema, savedRules)).toEqual([]);
  });
});

describe('the list of rules in the form', () => {
  function Held() {
    const [value, setValue] = useState<NamingRules>({
      schema_version: 1,
      rules: [
        { ...rule, glob: 'first/*' },
        { ...rule, glob: 'second/*' },
        { ...rule, glob: 'third/*' },
      ],
    });
    return (
      <RulesForm
        schema={schema}
        value={value}
        onChange={(next) => {
          if (formHolds(next)) setValue(next);
        }}
      />
    );
  }
  const globs = () =>
    screen.getAllByRole('textbox', { name: /^glob/ }).map((box) => (box as HTMLInputElement).value);
  const press = (name: string, index: number) => {
    const button = screen.getAllByRole('button', { name }).at(index);
    button?.focus();
    if (button !== undefined) fireEvent.click(button);
    return button;
  };

  it('names each action in words, and keeps focus off the page body when a rule is removed', () => {
    render(<Held />);
    expect(screen.getAllByRole('button', { name: 'Remove this rule' })).toHaveLength(3);
    press('Remove this rule', 2);
    expect(globs()).toEqual(['first/*', 'second/*']);
    // The pressed button left with its rule: focus is on the button that is always there.
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Add a rule' }));
  });

  it('keeps focus on a rule’s button when the rule moves, and ignores one that is unavailable', () => {
    render(<Held />);
    const up = press('Move this rule up', 1);
    expect(globs()).toEqual(['second/*', 'first/*', 'third/*']);
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement?.textContent).toBe('Move this rule up');
    expect(up).toBeDefined();
    // The first rule cannot go further up: its button says so and does nothing.
    const first = screen.getAllByRole('button', { name: 'Move this rule up' }).at(0);
    expect(first?.getAttribute('aria-disabled')).toBe('true');
    expect(first).toHaveProperty('disabled', false);
    press('Move this rule up', 0);
    expect(globs()).toEqual(['second/*', 'first/*', 'third/*']);
  });
});

describe('the naming rules', () => {
  it('opens on its heading with the saved rules of the version shown, and Close gives focus back', async () => {
    const { api } = service();
    await openRules();
    expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Naming rules' }));
    expect(prefix()).toHaveProperty('value', '');
    expect(within(panel()).getByRole('textbox', { name: /^separator/ })).toHaveProperty(
      'value',
      '-',
    );
    expect(panel().textContent).toContain('These are the saved rules, unchanged.');
    expect(api.callsTo(RULES)).toEqual([
      { workspace_id: WS, at: { kind: 'revision', revision: HEAD } },
    ]);
    await userEvent.click(button('Close the naming rules'));
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
    expect(document.activeElement).toBe(button('Naming rules'));
    expect(api.violations).toEqual([]);
  });

  it('saves changed rules on the head shown, and the shell shows the revision that holds them', async () => {
    const { api } = service();
    await openRules();
    const save = button('Save the rules');
    expect(save.getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(save);
    expect(api.callsTo(SET)).toEqual([]);
    await setPrefix('2025-');
    expect(panel().textContent).toContain('These rules differ from the saved ones.');
    await userEvent.click(save);
    await waitFor(() => expect(said()).toContain('The rules are saved.'));
    expect(api.callsTo(SET)).toEqual([
      {
        workspace_id: WS,
        base_revision: HEAD,
        idempotency_key: expect.stringMatching(UUID),
        rules: { schema_version: 1, rules: [{ ...rule, prefix: '2025-' }] },
      },
    ]);
    expect(document.activeElement).toBe(save);
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc');
    // Read again at the head the save made: they are the saved rules now.
    await waitFor(() =>
      expect(panel().textContent).toContain('These are the saved rules, unchanged.'),
    );
    expect(api.callsTo(RULES).at(-1)).toEqual({
      workspace_id: WS,
      at: { kind: 'revision', revision: FIRST },
    });
    expect(api.violations).toEqual([]);
  });

  it('never sends rules that have a problem: it is shown at its field and in the list', async () => {
    const { api } = service();
    await openRules();
    const version = within(panel()).getByRole('spinbutton', { name: /^schema_version/ });
    await userEvent.clear(version);
    await waitFor(() =>
      expect(within(panel()).getByRole('alert').textContent).toContain('1 problem'),
    );
    expect(panel().querySelector('#root_schema_version__error')?.textContent).toMatch(/required/);
    await userEvent.click(button('Save the rules'));
    await userEvent.click(button('Preview the renames'));
    expect(api.callsTo(SET)).toEqual([]);
    expect(api.callsTo(PREVIEW)).toEqual([]);
    // Focus is on the list of what is wrong.
    expect(document.activeElement?.textContent).toContain('Nothing is sent until they are fixed');
  });

  it('keeps an unknown key the saved rules hold, says so by name, and sends nothing while it is there', async () => {
    const { api } = service({}, { ...savedRules, rules: [{ ...rule, legacy_flag: true }] });
    await openRules();
    expect(within(panel()).getByRole('alert').textContent).toContain('“legacy_flag”');
    await setPrefix('x-');
    // The form's edit did not drop it.
    expect(within(panel()).getByRole('alert').textContent).toContain('“legacy_flag”');
    await userEvent.click(button('Save the rules'));
    await userEvent.click(button('Preview the renames'));
    expect(writes(api)).toEqual([]);
    expect(api.callsTo(PREVIEW)).toEqual([]);
  });

  it('previews without changing anything: every item of the folder and below, with old and new names', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    const heading = await previewNow();
    expect(document.activeElement).toBe(heading);
    expect(api.callsTo(PREVIEW)).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'revision', revision: HEAD },
        // The top level, then the folder under it.
        item_ids: [SOURCE, NOTE, OTHER],
        rules: { schema_version: 1, rules: [{ ...rule, prefix: '2025-' }] },
      },
    ]);
    const table = within(panel()).getByRole('table', { name: 'The renames, one by one' });
    const rows = within(table)
      .getAllByRole('row')
      .map((each) => each.textContent);
    expect(rows).toEqual([
      'Name nowName afterNotes',
      'notes/meeting.mdnotes/2025-meeting.md',
      'notes/plan.mdnotes/2025-plan.mdNo date was found.',
    ]);
    expect(panel().textContent).toContain(
      '2 items would be renamed, and 1 keeps its name, of the 3 items in the whole workspace',
    );
    expect(panel().textContent).toContain('Index files are not renamed.');
    expect(panel().textContent).toContain('Nothing has been renamed.');
    // Nothing was written, and the workspace shows the same version.
    expect(writes(api)).toEqual([]);
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');
    expect(api.violations).toEqual([]);
  });

  it('lists every page of a folder before it previews', async () => {
    const { api } = service({
      [LIST]: (body) => {
        const revision = revisionAsked(body, HEAD);
        const page = body.page as { cursor?: string | null };
        const second = page.cursor === 'next';
        return ok({
          folders: [],
          items: [{ ...(second ? otherSummary : noteSummary), revision }],
          revision,
          next_cursor: second ? null : 'next',
        });
      },
    });
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    expect(api.callsTo(PREVIEW).at(0)?.item_ids).toEqual([NOTE, OTHER]);
  });

  it('gives the documents back their place when an item is opened from the folders', async () => {
    service();
    await openRules();
    const tree = screen.getByRole('tree', { name: 'Folders' });
    await userEvent.click(within(tree).getByRole('treeitem', { name: /Annual report/ }));
    expect(await screen.findByText('Text of Annual report')).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
  });

  it('previews one folder when asked, and asks nothing for a folder that holds no item', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    const folder = within(panel()).getByRole('textbox', { name: 'Preview the items in folder' });
    await userEvent.type(folder, 'notes');
    await previewNow();
    expect(api.callsTo(PREVIEW).at(0)?.item_ids).toEqual([NOTE, OTHER]);
    await userEvent.clear(folder);
    await userEvent.type(folder, 'empty');
    await userEvent.click(button('Preview the renames'));
    expect(
      (await screen.findByText('There is nothing to preview.')).closest('[role="alert"]'),
    ).toBeTruthy();
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
    await userEvent.clear(folder);
    await userEvent.type(folder, 'a//b');
    await userEvent.click(button('Preview the renames'));
    expect(document.activeElement).toBe(folder);
    expect(folder.getAttribute('aria-invalid')).toBe('true');
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
  });

  it('applies only after the question, sending exactly the plan and the rules of the preview', async () => {
    const { api } = service();
    // The open tab cites the head as it is before the renaming (a search result's link does).
    await openRules(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    // Nothing to apply before a preview.
    expect(screen.queryByRole('button', { name: /Apply this preview/ })).toBeNull();
    await setPrefix('2025-');
    await previewNow();
    expect(api.callsTo(APPLY)).toEqual([]);
    const apply = button('Apply this preview…');
    await userEvent.click(apply);
    const question = screen.getByRole('group', { name: 'Rename 2 items?' });
    expect(document.activeElement).toBe(
      within(question).getByRole('button', { name: 'Keep the names' }),
    );
    await userEvent.keyboard('{Enter}');
    expect(api.callsTo(APPLY)).toEqual([]);
    expect(document.activeElement).toBe(button('Apply this preview…'));

    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    await waitFor(() => expect(said()).toContain('2 items were renamed.'));
    const previewed = api.callsTo(PREVIEW).at(0);
    expect(api.callsTo(APPLY)).toEqual([
      {
        workspace_id: WS,
        idempotency_key: expect.stringMatching(UUID),
        plan: {
          base_revision: HEAD,
          rules_digest: DIGEST,
          entries: [
            { item_id: NOTE, from: 'notes/meeting.md', to: 'notes/2025-meeting.md', warnings: [] },
            {
              item_id: OTHER,
              from: 'notes/plan.md',
              to: 'notes/2025-plan.md',
              warnings: [{ code: 'ambiguous_date', message: 'No date was found.' }],
            },
            { item_id: SOURCE, from: 'report.pdf.md', to: 'report.pdf.md', warnings: [] },
          ],
          duplicate_observations: [],
          warnings: [{ code: 'note', message: 'Index files are not renamed.' }],
        },
        rules: previewed?.rules,
      },
    ]);
    // The preview is gone with its use; focus is on the panel's heading; the shell shows the
    // revision the renaming made.
    expect(screen.queryByRole('heading', { name: 'Preview: what would be renamed' })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Naming rules' }));
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc');
    // The open item is the same item at its new name, once its tab is back in view: the tab
    // that cited the old head cites the revision the renaming made. The rules that renamed were
    // never saved, so closing asks first.
    await userEvent.click(button('Close the naming rules'));
    await userEvent.click(button('Discard my changes'));
    expect(await screen.findByText('notes/2025-meeting.md')).toBeTruthy();
    expect(screen.queryByText('notes/meeting.md')).toBeNull();
    expect(screen.getByRole('tab', { name: 'Meeting notes' }).getAttribute('data-item-id')).toBe(
      NOTE,
    );
    expect(api.violations).toEqual([]);
  });

  it('marks a preview out of date when the rules change after it, and does not apply it', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await setPrefix('2026-');
    const stale = await within(panel()).findByText(
      /This preview is out of date and cannot be applied/,
    );
    expect(stale.closest('[role="alert"]')?.textContent).toContain(
      'The rules were changed since this preview.',
    );
    const apply = button('Apply this preview…');
    expect(apply.getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(apply);
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    expect(api.callsTo(APPLY)).toEqual([]);
    // Back to the rules of the preview: it is the preview of what the page shows again.
    await setPrefix('2025-');
    await waitFor(() =>
      expect(button('Apply this preview…').getAttribute('aria-disabled')).toBe('false'),
    );
  });

  it('marks a preview out of date when the workspace changed after it, even with the question open', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    expect(screen.getByRole('group', { name: 'Rename 2 items?' })).toBeTruthy();
    // The person saves the rules: the workspace is at another revision than the preview's.
    await userEvent.click(button('Save the rules'));
    const stale = await within(panel()).findByText(
      /This preview is out of date and cannot be applied/,
    );
    expect(stale.closest('[role="alert"]')?.textContent).toContain(
      'it now shows revision cccccccccc, and the preview was made on aaaaaaaaaa',
    );
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    expect(button('Apply this preview…').getAttribute('aria-disabled')).toBe('true');
    expect(api.callsTo(APPLY)).toEqual([]);
    // A new preview is made on the version shown now, and can be applied.
    await previewNow();
    await waitFor(() => expect(api.callsTo(PREVIEW)).toHaveLength(2));
    expect(api.callsTo(PREVIEW).at(1)).toMatchObject({ at: { kind: 'revision', revision: FIRST } });
    await waitFor(() =>
      expect(button('Apply this preview…').getAttribute('aria-disabled')).toBe('false'),
    );
  });

  it('says so when the service refuses a preview as out of date: nothing is renamed, and it cannot be sent again', async () => {
    const { api, moveHead } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    moveHead();
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    const alerts = await screen.findAllByRole('alert');
    await waitFor(() =>
      expect(alerts.map((alert) => alert.textContent).join(' ')).toContain(
        'Renaming: nothing was changed.',
      ),
    );
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(panel().textContent).toContain('The service refused this preview');
    expect(said()).not.toMatch(/(was|were) renamed/);
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    const apply = button('Apply this preview…');
    expect(apply.getAttribute('aria-disabled')).toBe('true');
    expect(document.activeElement).toBe(apply);
    await userEvent.click(apply);
    expect(api.callsTo(APPLY)).toHaveLength(1);
    // The head moved: the way on is Refresh, then a new preview.
    expect(await screen.findByRole('button', { name: 'Refresh the workspace' })).toBeTruthy();
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');
  });

  it('reads "not available in this build" for an unbuilt preview, and repeats a failed one as it was', async () => {
    let answers: Reply[] = [
      refuse(501, 'not_implemented', 'preview_names is not implemented'),
      refuse(503, 'unavailable', 'the index is busy'),
    ];
    const own = service({
      [PREVIEW]: () => {
        const [next, ...rest] = answers;
        answers = rest;
        return next ?? refuse(503, 'unavailable', 'the index is busy');
      },
    });
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(button('Preview the renames'));
    expect((await screen.findByRole('alert')).textContent).toContain(
      'Previewing the renames: Not available in this build.',
    );
    expect(screen.queryByRole('heading', { name: 'Preview: what would be renamed' })).toBeNull();
    await userEvent.click(button('Preview the renames'));
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain(
        'The service is temporarily unavailable.',
      ),
    );
    await userEvent.click(button('Try again'));
    await waitFor(() => expect(own.api.callsTo(PREVIEW)).toHaveLength(3));
    expect(own.api.callsTo(PREVIEW).at(2)).toEqual(own.api.callsTo(PREVIEW).at(1));
  });

  it('shows the same rules as YAML: a change in the form shows in the YAML, and the reverse', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    expect(textIn(box)).toContain('schema_version: 1');
    expect(textIn(box)).toContain('prefix: 2025-');
    // Text another YAML reader would take for a date or a boolean is written in quotes.
    expect(textIn(box)).toContain('date_order: ymd');
    typeYaml(box, textIn(box).replace('prefix: 2025-', 'prefix: draft-'));
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    expect(prefix()).toHaveProperty('value', 'draft-');
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(api.callsTo(SET)).toHaveLength(1));
    expect(api.callsTo(SET).at(0)?.rules).toEqual({
      schema_version: 1,
      rules: [{ ...rule, prefix: 'draft-' }],
    });
    expect(api.violations).toEqual([]);
  });

  it('keeps a key typed in the YAML through an edit in the form, exactly, and sends nothing with it', async () => {
    const { api } = service();
    await openRules();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    typeYaml(box, `${textIn(box)}x-team:\n  owner: ada\n  tags: [a, b]\n`);
    expect(within(panel()).getByRole('alert').textContent).toContain('“x-team”');
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    await setPrefix('2025-');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const again = await findTextBox('The naming rules as YAML');
    expect(textIn(again)).toContain('prefix: 2025-');
    expect(textIn(again)).toContain('x-team:\n  owner: ada\n  tags:\n    - a\n    - b\n');
    await userEvent.click(button('Save the rules'));
    await userEvent.click(button('Preview the renames'));
    expect(writes(api)).toEqual([]);
    expect(api.callsTo(PREVIEW)).toEqual([]);
  });

  it('never sends YAML that cannot be read: the problem is at its line, the rules stay, the text stays', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    const broken = 'schema_version: 1\nrules: [\n';
    typeYaml(box, broken);
    const alert = within(panel()).getByRole('alert');
    expect(alert.textContent).toContain('This YAML cannot be read. Nothing of it is sent.');
    expect(alert.textContent).toMatch(/Line 3, column 1: /);
    // The text box is described by what is wrong with it.
    const described = box.getAttribute('aria-describedby') ?? '';
    expect(document.getElementById(described)?.contains(alert)).toBe(true);
    await userEvent.click(button('Save the rules'));
    await userEvent.click(button('Preview the renames'));
    expect(writes(api)).toEqual([]);
    expect(api.callsTo(PREVIEW)).toEqual([]);
    expect(document.activeElement?.textContent).toContain('This YAML cannot be read');
    // The form shows the last rules that could be read, and says so; the text is kept as typed.
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    expect(prefix()).toHaveProperty('value', '2025-');
    expect(panel().textContent).toContain('The YAML view holds text that cannot be read.');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    expect(textIn(await findTextBox('The naming rules as YAML'))).toBe(broken);
  });

  it('names what is wrong with readable YAML that is not valid rules, and sends nothing', async () => {
    const { api } = service();
    await openRules();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    typeYaml(box, textIn(box).replace('case: kebab', 'case: shouting'));
    expect(within(panel()).getByRole('alert').textContent).toContain('.rules.0.case must');
    typeYaml(box, '- just\n- a list\n');
    expect(within(panel()).getByRole('alert').textContent).toMatch(/must be object/);
    await userEvent.click(button('Save the rules'));
    expect(writes(api)).toEqual([]);
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    expect(panel().textContent).toContain('What the YAML view holds is not a set of rules');
  });

  it('lets a person who may only read preview, and offers them neither Save nor Apply', async () => {
    const { api, state } = service();
    state.permissions = workspace.permissions;
    await openRules();
    expect(screen.queryByRole('button', { name: 'Save the rules' })).toBeNull();
    await setPrefix('2025-');
    await previewNow();
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
    expect(screen.queryByRole('button', { name: /Apply this preview/ })).toBeNull();
  });
});

describe('a refusal that names a field of the rules or of a preview', () => {
  /** The alert that holds `words`, once it is on the page. */
  const refusalWith = async (words: string) => {
    await waitFor(() =>
      expect(
        screen.queryAllByRole('alert').some((alert) => alert.textContent?.includes(words)),
      ).toBe(true),
    );
    const alert = screen.getAllByRole('alert').find((each) => each.textContent?.includes(words));
    return alert?.textContent ?? '';
  };

  it('set_rules: the field is named in the notice, and nothing reads as saved', async () => {
    const { api } = service({
      [SET]: () =>
        refuse(400, 'invalid_input', 'the glob does not parse: unclosed [', '/rules/0/glob'),
    });
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(button('Save the rules'));
    const words = await refusalWith('the glob does not parse: unclosed [');
    expect(words).toContain('Saving the rules: nothing was changed.');
    expect(words).toContain('The service did not accept “glob” of rule 1.');
    expect(said()).not.toContain('The rules are saved');
    expect(panel().textContent).toContain('These rules differ from the saved ones.');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');
    expect(api.callsTo(SET)).toHaveLength(1);
  });

  it('apply_names: the field is named in the notice, and nothing reads as renamed', async () => {
    const { api } = service({
      [APPLY]: () =>
        refuse(
          400,
          'invalid_input',
          'the plan names a path that is not a workspace path',
          '/plan/entries/0/to',
        ),
    });
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    const words = await refusalWith('the plan names a path that is not a workspace path');
    expect(words).toContain('Renaming: nothing was changed.');
    expect(words).toContain(
      'The service did not accept the name after of rename 1 of the preview.',
    );
    expect(said()).not.toMatch(/(was|were) renamed/);
    expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('aaaaaaaaaa');
    expect(api.callsTo(APPLY)).toHaveLength(1);
  });
});

describe('the retry of a renaming or a save that failed', () => {
  const busy = () => refuse(503, 'unavailable', 'the service is busy');
  const renamed2 = () => ok({ revision: FIRST, receipt_id: RECEIPT, warnings: [] });
  /** Preview with the prefix "2025-", confirm, and wait for the failure with its retry. */
  async function failedApply() {
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    return screen.findByRole('button', { name: 'Try again' });
  }
  const renamingNotice = () =>
    screen.getAllByRole('alert').find((alert) => alert.textContent?.includes('Renaming: '))
      ?.textContent ?? '';

  it('repeats the renaming under the same key while its preview is the one on screen', async () => {
    let failing = true;
    const { api } = service({ [APPLY]: () => (failing ? busy() : renamed2()) });
    const retry = await failedApply();
    expect(renamingNotice()).toContain('Renaming: The service is temporarily unavailable.');
    failing = false;
    await userEvent.click(retry);
    await waitFor(() => expect(said()).toContain('2 items were renamed.'));
    const [first, second] = api.callsTo(APPLY);
    expect(second).toEqual(first);
    expect(api.callsTo(APPLY)).toHaveLength(2);
  });

  it('withdraws the retry when the rules are edited: a preview that is out of date is not applied', async () => {
    let failing = true;
    const { api } = service({ [APPLY]: () => (failing ? busy() : renamed2()) });
    await failedApply();
    failing = false;
    // The person changes the rules: the preview on screen is out of date.
    await setPrefix('2026-');
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    // The failure is still said, with what to do in place of the retry.
    expect(renamingNotice()).toContain('Renaming: The service is temporarily unavailable.');
    expect(renamingNotice()).toContain('so it is not tried again. Preview again');
    expect(renamingNotice()).toContain('the service is busy');
    expect(api.callsTo(APPLY)).toHaveLength(1);
    expect(said()).not.toMatch(/(was|were) renamed/);
    // Put back, the preview is the preview of what the page shows again, and so is its retry.
    await setPrefix('2025-');
    await userEvent.click(await screen.findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(said()).toContain('2 items were renamed.'));
    expect(api.callsTo(APPLY).at(1)).toEqual(api.callsTo(APPLY).at(0));
  });

  it('withdraws the retry when another preview is on screen: the older plan is never applied', async () => {
    let failing = true;
    const { api } = service({ [APPLY]: () => (failing ? busy() : renamed2()) });
    await failedApply();
    failing = false;
    await setPrefix('2026-');
    await userEvent.click(button('Preview the renames'));
    await waitFor(() => expect(panel().textContent).toContain('notes/2026-meeting.md'));
    // The new preview is not out of date, and the failed renaming was not for it.
    expect(panel().textContent).not.toContain('out of date');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    expect(renamingNotice()).toContain('so it is not tried again. Preview again');
    expect(api.callsTo(APPLY)).toHaveLength(1);
    // Applying the preview on screen sends its own plan, as a new request.
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    await waitFor(() => expect(said()).toContain('2 items were renamed.'));
    const [older, newer] = api.callsTo(APPLY);
    const firstTo = (call: Json | undefined) =>
      (call?.plan as { entries: { to: string }[] } | undefined)?.entries.at(0)?.to;
    expect(firstTo(older)).toBe('notes/2025-meeting.md');
    expect(firstTo(newer)).toBe('notes/2026-meeting.md');
    expect(newer?.idempotency_key).not.toBe(older?.idempotency_key);
  });

  it('withdraws the retry of a save when the rules are edited: the rules sent are the ones on screen', async () => {
    let failing = true;
    const { api } = service({
      [SET]: () => (failing ? busy() : ok({ revision: FIRST, receipt_id: RECEIPT, warnings: [] })),
    });
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(button('Save the rules'));
    await screen.findByRole('button', { name: 'Try again' });
    failing = false;
    await setPrefix('2026-');
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    const notice = screen.getByRole('alert').textContent ?? '';
    expect(notice).toContain('Saving the rules: The service is temporarily unavailable.');
    expect(notice).toContain('are not sent again by themselves');
    expect(api.callsTo(SET)).toHaveLength(1);
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(said()).toContain('The rules are saved.'));
    const sent = api.callsTo(SET).at(1)?.rules as NamingRules | undefined;
    expect(sent?.rules.at(0)?.prefix).toBe('2026-');
  });
});

describe('rules typed as YAML a moment before a press', () => {
  /** Change the YAML and press at once: no pause passes, and focus never left the text box. */
  const yamlWithPrefix = async (value: string) => {
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    replaceText(box, textIn(box).replace(/prefix: .*/, `prefix: ${value}`));
  };

  it('saves the rules the text holds, not the ones read before it', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await yamlWithPrefix('2026-');
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(api.callsTo(SET)).toHaveLength(1));
    const sent = api.callsTo(SET).at(0)?.rules as NamingRules | undefined;
    expect(sent?.rules.at(0)?.prefix).toBe('2026-');
  });

  it('previews with the rules the text holds, and sends nothing while it cannot be read', async () => {
    const { api } = service();
    await openRules();
    await yamlWithPrefix('2026-');
    await userEvent.click(button('Preview the renames'));
    await waitFor(() => expect(api.callsTo(PREVIEW)).toHaveLength(1));
    const asked = api.callsTo(PREVIEW).at(0)?.rules as NamingRules | undefined;
    expect(asked?.rules.at(0)?.prefix).toBe('2026-');
    replaceText(await findTextBox('The naming rules as YAML'), 'rules: [\n');
    await userEvent.click(button('Preview the renames'));
    await userEvent.click(button('Save the rules'));
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
    expect(writes(api)).toEqual([]);
    expect(document.activeElement?.textContent).toContain('This YAML cannot be read');
  });

  it('opens no question for a preview the text has just put out of date', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await yamlWithPrefix('2026-');
    await userEvent.click(button('Apply this preview…'));
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(api.callsTo(APPLY)).toEqual([]);
  });

  it('renames nothing when the text changed the rules while the question was open', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    await userEvent.click(button('Apply this preview…'));
    const box = await findTextBox('The naming rules as YAML');
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: 2026-'));
    await userEvent.click(button('Rename 2 items'));
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(api.callsTo(APPLY)).toEqual([]);
    expect(said()).not.toMatch(/(was|were) renamed/);
  });

  it('shows in the form what the text says when the view changes before the pause has passed', async () => {
    service();
    await openRules();
    await yamlWithPrefix('typed-');
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    expect(prefix()).toHaveProperty('value', 'typed-');
  });
});

describe('whether there is something to save, a moment after typing', () => {
  const yamlBox = async () => {
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    return findTextBox('The naming rules as YAML');
  };

  it('saves rules the text has just changed, though the page still showed them as unchanged', async () => {
    const { api } = service();
    await openRules();
    const box = await yamlBox();
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: 2026-'));
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(api.callsTo(SET)).toHaveLength(1));
    const sent = api.callsTo(SET).at(0)?.rules as NamingRules | undefined;
    expect(sent?.rules.at(0)?.prefix).toBe('2026-');
  });

  it('sends nothing when the text has just put the rules back to the saved ones', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    const box = await yamlBox();
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: ""'));
    await userEvent.click(button('Save the rules'));
    await waitFor(() =>
      expect(panel().textContent).toContain('These are the saved rules, unchanged.'),
    );
    expect(api.callsTo(SET)).toEqual([]);
  });
});

describe('the question before a renaming, when it leaves by itself', () => {
  const question = () => screen.queryByRole('group', { name: /^Rename / });

  it('puts keyboard focus on the button that opened it when a save in flight lands under it', async () => {
    let release: (reply: Reply) => void = () => {};
    const holder: { move: () => string } = { move: () => '' };
    const own = service({
      [SET]: () =>
        new Promise<Reply>((resolve) => {
          release = resolve;
        }),
    });
    holder.move = own.moveHead;
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    // The save is pressed first and is still in flight when the question opens.
    await userEvent.click(button('Save the rules'));
    await userEvent.click(button('Apply this preview…'));
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Keep the names' }));
    await act(async () => {
      release(ok({ revision: holder.move(), receipt_id: RECEIPT, warnings: [] }));
      await Promise.resolve();
    });
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(question()).toBeNull();
    // Not the page body: the button that opened the question, which now says it is unavailable.
    const apply = button('Apply this preview…');
    expect(document.activeElement).toBe(apply);
    expect(apply.getAttribute('aria-disabled')).toBe('true');
    expect(own.api.callsTo(APPLY)).toEqual([]);
  });

  it('does not open again by itself when the rules are put back', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    expect(question()).not.toBeNull();
    await setPrefix('2026-');
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(question()).toBeNull();
    // Back to the rules of the preview: it can be applied again, and that is asked for again.
    await setPrefix('2025-');
    await waitFor(() =>
      expect(button('Apply this preview…').getAttribute('aria-disabled')).toBe('false'),
    );
    expect(question()).toBeNull();
    await userEvent.click(button('Apply this preview…'));
    expect(question()).not.toBeNull();
    expect(api.callsTo(APPLY)).toEqual([]);
  });

  it('does not open unasked for a new preview after the service refused the one before', async () => {
    const { api, moveHead } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    moveHead();
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    await within(panel()).findByText(/The service refused this preview/);
    await userEvent.click(await screen.findByRole('button', { name: 'Refresh the workspace' }));
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    await userEvent.click(button('Preview the renames'));
    await waitFor(() => expect(api.callsTo(PREVIEW)).toHaveLength(2));
    await waitFor(() =>
      expect(button('Apply this preview…').getAttribute('aria-disabled')).toBe('false'),
    );
    // A preview that can be applied, and nobody asked to apply it.
    expect(question()).toBeNull();
    expect(api.callsTo(APPLY)).toHaveLength(1);
  });
});

describe('the question before a renaming, when another preview is made', () => {
  it('is closed by the new preview: what was asked was asked of the one before', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    expect(screen.getByRole('group', { name: 'Rename 2 items?' })).toBeTruthy();
    // The same rules on the same version, previewed again: another preview all the same.
    await userEvent.click(button('Preview the renames'));
    await waitFor(() => expect(api.callsTo(PREVIEW)).toHaveLength(2));
    await waitFor(() => expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull());
    expect(document.activeElement).toBe(
      screen.getByRole('heading', { name: 'Preview: what would be renamed' }),
    );
    expect(button('Apply this preview…').getAttribute('aria-disabled')).toBe('false');
    expect(api.callsTo(APPLY)).toEqual([]);
  });

  it('asks nothing more for a second press of Preview while the first is in flight', async () => {
    let release: (reply: Reply) => void = () => {};
    const lists: Json[] = [];
    const { api } = service({
      [PREVIEW]: () =>
        new Promise<Reply>((resolve) => {
          release = resolve;
        }),
    });
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(button('Preview the renames'));
    await waitFor(() => expect(api.callsTo(PREVIEW)).toHaveLength(1));
    lists.push(...api.callsTo(LIST));
    const working = screen.getByRole('button', { name: 'Previewing…' });
    expect(working.getAttribute('aria-disabled')).toBe('true');
    await userEvent.click(working);
    await userEvent.click(working);
    // Neither a second walk of the folders nor a second preview was started.
    expect(api.callsTo(LIST)).toHaveLength(lists.length);
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
    await act(async () => {
      release(refuse(503, 'unavailable', 'the index is busy'));
      await Promise.resolve();
    });
    await screen.findByRole('button', { name: 'Try again' });
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
  });
});

describe('the person’s drafts of items a renaming renames', () => {
  const SAVE_DRAFT = '/api/items/save-draft';
  const draft = {
    item_id: NOTE,
    base_revision: HEAD,
    content_digest: DIGEST,
    editor: 'ada',
    saved_at: '2026-01-01T23:30:00.000Z',
  };
  const withDraftOfNote = (save: Handler): Record<string, Handler> => ({
    '/api/items/list-drafts': () => ok({ items: [draft], next_cursor: null }),
    '/api/items/get-item': (body) => {
      const revision = revisionAsked(body, HEAD);
      const item = body.item_id === NOTE ? noteSummary : otherSummary;
      const document = itemDocument({ ...item, revision });
      return ok(
        body.item_id === NOTE
          ? { ...document, draft: { body: 'My draft', properties: document.properties, draft } }
          : document,
      );
    },
    [SAVE_DRAFT]: save,
  });
  async function rename() {
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    await waitFor(() => expect(said()).toContain('2 items were renamed.'));
  }

  it('follow it: each is saved on the revision the renaming made, and that is said', async () => {
    const { api } = service(
      withDraftOfNote((body) => ok({ ...draft, base_revision: text(body, 'base_revision') })),
    );
    await rename();
    expect(said()).toContain(
      '1 draft of yours follows the renaming: the next Snapshot will not ask about it.',
    );
    expect(said()).not.toContain('could not follow');
    // The one renamed item the person has a draft of, and no other.
    expect(api.callsTo(SAVE_DRAFT)).toEqual([
      {
        workspace_id: WS,
        item_id: NOTE,
        base_revision: FIRST,
        body: 'My draft',
        properties: { type: 'meeting-note', okf_jawn: { id: NOTE } },
        idempotency_key: expect.stringMatching(UUID),
      },
    ]);
  });

  it('say so when one could not follow: the renaming is made all the same', async () => {
    const { api } = service(withDraftOfNote(() => refuse(500, 'internal', 'the store broke')));
    await rename();
    expect(said()).toContain(
      '1 draft of yours could not follow it: the next Snapshot will first ask whether to keep your text.',
    );
    expect(said()).not.toContain('follows the renaming');
    expect(api.callsTo(APPLY)).toHaveLength(1);
  });

  it('are not spoken of when the person has none', async () => {
    const { api } = service();
    await rename();
    expect(said()).not.toContain('draft');
    expect(api.callsTo(SAVE_DRAFT)).toEqual([]);
    // Nor is any renamed item read for a draft: at 10,000 items that would be 10,000 requests.
    expect(
      api.callsTo('/api/items/get-item').filter((body) => revisionAsked(body, '') === FIRST),
    ).toEqual([]);
  });
});

describe('leaving the naming rules with edits that are not saved', () => {
  const question = () =>
    screen.queryByRole('group', { name: 'Leave the naming rules without saving?' });
  const close = () => button('Close the naming rules');
  const leavingAsks = () => {
    const event = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  };

  it('asks before Close, keeps the edits on "Keep editing", and discards them only when told', async () => {
    service();
    await openRules();
    await setPrefix('unsaved-');
    await userEvent.click(close());
    // Still open, with the question; focus on the choice that loses nothing.
    expect(question()).not.toBeNull();
    expect(document.activeElement).toBe(button('Keep editing'));
    await userEvent.keyboard('{Enter}');
    expect(question()).toBeNull();
    expect(document.activeElement).toBe(close());
    expect(prefix()).toHaveProperty('value', 'unsaved-');
    await userEvent.click(close());
    await userEvent.click(button('Discard my changes'));
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
    expect(document.activeElement).toBe(button('Naming rules'));
    await userEvent.click(button('Naming rules'));
    await within(panel()).findByRole('textbox', { name: /^prefix/ });
    expect(prefix()).toHaveProperty('value', '');
  });

  it('asks nothing when there is nothing to lose: rules put back to the saved ones', async () => {
    service();
    await openRules();
    await setPrefix('x');
    // Put back in the YAML view (the form drops a text it is emptied of).
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    typeYaml(box, textIn(box).replace(/prefix: .*/, 'prefix: ""'));
    expect(leavingAsks()).toBe(false);
    await userEvent.click(close());
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
  });

  it('asks before an item is opened from the folders, and opens it only after "Discard my changes"', async () => {
    service();
    await openRules();
    await setPrefix('unsaved-');
    const tree = screen.getByRole('tree', { name: 'Folders' });
    const report = within(tree).getByRole('treeitem', { name: /Annual report/ });
    await userEvent.click(report);
    expect(question()).not.toBeNull();
    expect(screen.queryByText('Text of Annual report')).toBeNull();
    await userEvent.click(button('Keep editing'));
    // Back to what asked to leave: the row that was pressed.
    expect(document.activeElement).toBe(report);
    expect(prefix()).toHaveProperty('value', 'unsaved-');
    await userEvent.click(report);
    await userEvent.click(button('Discard my changes'));
    expect(await screen.findByText('Text of Annual report')).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
  });

  it('asks before the shell’s own button closes the panel', async () => {
    service();
    await openRules();
    await setPrefix('unsaved-');
    await userEvent.click(button('Naming rules'));
    expect(question()).not.toBeNull();
    expect(screen.getByRole('region', { name: 'Naming rules' })).toBeTruthy();
    await userEvent.click(button('Keep editing'));
    expect(document.activeElement).toBe(button('Naming rules'));
  });

  it('counts YAML that cannot be read, and text typed a moment ago, as edits', async () => {
    service();
    await openRules();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    // Typed, and not yet read: the pause has not passed.
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: typed-'));
    await userEvent.click(close());
    expect(question()).not.toBeNull();
    await userEvent.click(button('Keep editing'));
    typeYaml(await findTextBox('The naming rules as YAML'), 'rules: [\n');
    expect(leavingAsks()).toBe(true);
    await userEvent.click(close());
    expect(question()).not.toBeNull();
  });

  it('lets the browser ask before a reload while the rules hold edits, and not after they are saved', async () => {
    service();
    await openRules();
    expect(leavingAsks()).toBe(false);
    await setPrefix('2025-');
    expect(leavingAsks()).toBe(true);
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(said()).toContain('The rules are saved.'));
    await waitFor(() =>
      expect(panel().textContent).toContain('These are the saved rules, unchanged.'),
    );
    expect(leavingAsks()).toBe(false);
  });
});
