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
import { keepOrder, type Preview, sameRules } from '../../src/features/conventions/rules';
import { resetDraftSessions } from '../../src/features/drafts/saving';
import { setYamlEngine, TOO_COMPLEX_WORDS, YAML_LIMIT_MS } from '../../src/lib/yaml-reading';
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
import { type HeldReading, heldEngine } from './support/yaml-engine';

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
/** A turn of the event loop: a reading that was started has answered after it. */
const turn = () => act(() => new Promise<void>((resolve) => setTimeout(resolve, 0)));
/**
 * Replace the YAML and leave the text box, as a person does before pressing anything else: its
 * reading starts at once, and this waits until it has answered.
 */
const typeYaml = async (box: HTMLElement, text: string) => {
  replaceText(box, text);
  act(() => {
    fireEvent.blur(box);
  });
  await turn();
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
    const { router } = await openRules(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
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
    // The address follows the renaming by the page's own correction, which is not the person
    // leaving: the rules, still not saved, stay open and nothing is asked.
    await waitFor(() => expect(router.state.location.search).toMatchObject({ rev: FIRST }));
    expect(
      screen.queryByRole('group', { name: 'Leave the naming rules without saving?' }),
    ).toBeNull();
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
    await typeYaml(box, textIn(box).replace('prefix: 2025-', 'prefix: draft-'));
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
    await typeYaml(box, `${textIn(box)}x-team:\n  owner: ada\n  tags: [a, b]\n`);
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
    await typeYaml(box, broken);
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
    await typeYaml(box, textIn(box).replace('case: kebab', 'case: shouting'));
    expect(within(panel()).getByRole('alert').textContent).toContain('.rules.0.case must');
    await typeYaml(box, '- just\n- a list\n');
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
  const NOT_READ =
    'The text of the YAML view is still being read. Nothing was sent or changed: press again in a moment.';
  /** The readings the page asked for; each stays under way until the test answers it. */
  const hold = () => {
    const held = heldEngine();
    setYamlEngine(held.engine);
    return held.readings;
  };
  const answer = (reading: HeldReading | undefined) =>
    act(() => {
      reading?.began();
      reading?.done();
    });
  /** Change the YAML and press at once: no pause passes, and focus never left the text box. */
  const yamlWithPrefix = async (value: string) => {
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    replaceText(box, textIn(box).replace(/prefix: .*/, `prefix: ${value}`));
  };

  it('sends nothing for a press of Save that finds text not read yet, and says so; read, the text is what is saved', async () => {
    const readings = hold();
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await yamlWithPrefix('2026-');
    expect(readings).toHaveLength(0);
    await userEvent.click(button('Save the rules'));
    // The press started the reading at once, sent nothing, and the page says why.
    expect(readings).toHaveLength(1);
    expect(said()).toBe(NOT_READ);
    await turn();
    expect(api.callsTo(SET)).toEqual([]);
    // Pressed again while the reading is under way: still nothing, and no second reading.
    await userEvent.click(button('Save the rules'));
    await turn();
    expect(api.callsTo(SET)).toEqual([]);
    expect(readings).toHaveLength(1);
    answer(readings.at(0));
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(api.callsTo(SET)).toHaveLength(1));
    const sent = api.callsTo(SET).at(0)?.rules as NamingRules | undefined;
    expect(sent?.rules.at(0)?.prefix).toBe('2026-');
  });

  it('asks no preview for a press that finds text not read yet; read, the preview is of the rules the text holds', async () => {
    const readings = hold();
    const { api } = service();
    await openRules();
    await yamlWithPrefix('2026-');
    await userEvent.click(button('Preview the renames'));
    expect(said()).toBe(NOT_READ);
    await turn();
    expect(api.callsTo(PREVIEW)).toEqual([]);
    answer(readings.at(0));
    await userEvent.click(button('Preview the renames'));
    await waitFor(() => expect(api.callsTo(PREVIEW)).toHaveLength(1));
    const asked = api.callsTo(PREVIEW).at(0)?.rules as NamingRules | undefined;
    expect(asked?.rules.at(0)?.prefix).toBe('2026-');
    // Text that cannot be read: nothing is sent while it is read, and nothing after.
    replaceText(await findTextBox('The naming rules as YAML'), 'rules: [\n');
    await userEvent.click(button('Preview the renames'));
    answer(readings.at(1));
    await userEvent.click(button('Preview the renames'));
    await userEvent.click(button('Save the rules'));
    expect(api.callsTo(PREVIEW)).toHaveLength(1);
    expect(writes(api)).toEqual([]);
    expect(document.activeElement?.textContent).toContain('This YAML cannot be read');
  });

  it('opens no question while the text is not read, and none for a preview the text put out of date', async () => {
    const readings = hold();
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await yamlWithPrefix('2026-');
    await userEvent.click(button('Apply this preview…'));
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    expect(said()).toBe(NOT_READ);
    answer(readings.at(0));
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    await userEvent.click(button('Apply this preview…'));
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    expect(api.callsTo(APPLY)).toEqual([]);
  });

  it('renames nothing when the text changed while the question was open', async () => {
    const readings = hold();
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    await userEvent.click(button('Apply this preview…'));
    const box = await findTextBox('The naming rules as YAML');
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: 2026-'));
    await userEvent.click(button('Rename 2 items'));
    expect(said()).toBe(NOT_READ);
    await turn();
    expect(api.callsTo(APPLY)).toEqual([]);
    // Read, the text holds other rules: the preview is out of date, and the question is gone.
    answer(readings.at(0));
    await within(panel()).findByText(/This preview is out of date and cannot be applied/);
    expect(screen.queryByRole('group', { name: /^Rename / })).toBeNull();
    expect(api.callsTo(APPLY)).toEqual([]);
    expect(said()).not.toMatch(/(was|were) renamed/);
  });

  it('draws no form while the text is not read, and then the form of what the text says', async () => {
    const readings = hold();
    service();
    await openRules();
    await yamlWithPrefix('typed-');
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    // The change of view started the reading; until it answers there is no form to edit.
    expect(readings).toHaveLength(1);
    expect(within(panel()).queryByRole('textbox', { name: /^prefix/ })).toBeNull();
    expect(within(panel()).getByText('Reading the text of the YAML view…')).toBeTruthy();
    answer(readings.at(0));
    expect(prefix()).toHaveProperty('value', 'typed-');
  });
});

describe('a YAML text whose reading does not come back', () => {
  it('is said to be too complex after the limit: nothing is sent, the rules and the text stay', async () => {
    const held = heldEngine();
    setYamlEngine(held.engine);
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    const typed = `[${'a: '.repeat(200)}]`;
    replaceText(box, typed);
    act(() => {
      fireEvent.blur(box);
    });
    act(() => held.readings.at(0)?.began());
    const alert = await within(panel()).findByRole('alert', {}, { timeout: YAML_LIMIT_MS + 2_000 });
    expect(alert.textContent).toContain('This YAML cannot be read. Nothing of it is sent.');
    expect(alert.textContent).toContain(TOO_COMPLEX_WORDS);
    // The reading was ended, and the text is still in the box.
    expect(held.readings.at(0)?.ended).toBe(true);
    expect(textIn(box)).toBe(typed);
    await userEvent.click(button('Save the rules'));
    await userEvent.click(button('Preview the renames'));
    expect(writes(api)).toEqual([]);
    expect(api.callsTo(PREVIEW)).toEqual([]);
    expect(document.activeElement?.textContent).toContain(TOO_COMPLEX_WORDS);
    // The rules are the last that could be read.
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    expect(prefix()).toHaveProperty('value', '2025-');
  });
});

describe('whether there is something to save, a moment after typing', () => {
  const yamlBox = async () => {
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    return findTextBox('The naming rules as YAML');
  };

  it('saves rules the text changed once it is read, though the page showed them as unchanged at the first press', async () => {
    const { api } = service();
    await openRules();
    const box = await yamlBox();
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: 2026-'));
    // Not read yet: the button still says there is nothing to save, and the press reads the text.
    await userEvent.click(button('Save the rules'));
    await turn();
    expect(api.callsTo(SET)).toEqual([]);
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(api.callsTo(SET)).toHaveLength(1));
    const sent = api.callsTo(SET).at(0)?.rules as NamingRules | undefined;
    expect(sent?.rules.at(0)?.prefix).toBe('2026-');
  });

  it('sends nothing when the text has put the rules back to the saved ones', async () => {
    const { api } = service();
    await openRules();
    await setPrefix('2025-');
    const box = await yamlBox();
    replaceText(box, textIn(box).replace(/prefix: .*/, 'prefix: ""'));
    await userEvent.click(button('Save the rules'));
    await waitFor(() =>
      expect(panel().textContent).toContain('These are the saved rules, unchanged.'),
    );
    await userEvent.click(button('Save the rules'));
    await turn();
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
    // The renaming is said as done; the draft's follow is said after it, when it has followed.
    await waitFor(() =>
      expect(said()).toContain(
        '1 draft of yours follows the renaming: the next Snapshot will not ask about it.',
      ),
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
    await waitFor(() =>
      expect(said()).toContain(
        '1 draft of yours could not follow it: the next Snapshot will first ask whether to keep your text.',
      ),
    );
    expect(said()).not.toContain('follows the renaming');
    expect(api.callsTo(APPLY)).toHaveLength(1);
  });

  it('are said to be left on the older version when the renaming is not on the page', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const holder: { commit: () => string } = { commit: () => '' };
    const own = service({
      ...withDraftOfNote((body) => ok({ ...draft, base_revision: text(body, 'base_revision') })),
      [APPLY]: () =>
        new Promise<Reply>((resolve) => {
          gate.release = () =>
            resolve(ok({ revision: holder.commit(), receipt_id: RECEIPT, warnings: [] }));
        }),
    });
    holder.commit = own.moveHead;
    // The note's tab is open on the person's draft of it; then the rules.
    renderAt(`/w/${WS}/i/${NOTE}`);
    await findTextBox('Text of Meeting notes');
    await userEvent.click(await screen.findByRole('button', { name: 'Naming rules' }));
    await within(panel()).findByRole('textbox', { name: /^prefix/ });
    await setPrefix('2025-');
    await previewNow();
    await userEvent.click(button('Apply this preview…'));
    await userEvent.click(button('Rename 2 items'));
    await waitFor(() => expect(own.api.callsTo(APPLY)).toHaveLength(1));
    // While the renaming is asked, the person refreshes to someone else's newer head.
    own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(said()).toContain('2 items were renamed. The renaming made revision dddddddddd.'),
    );
    await waitFor(() =>
      expect(said()).toContain(
        '1 draft of yours is still on the older version this page shows: until you refresh the workspace, saving may be refused, and the next Snapshot will first ask whether to keep your text.',
      ),
    );
    expect(said()).not.toContain('follows the renaming');
    expect(own.api.callsTo(SAVE_DRAFT)).toEqual([]);
  });

  it('are not spoken of when the person has none', async () => {
    const { api } = service();
    await rename();
    await turn();
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
    // Once it is read, it is found to be the saved rules.
    await typeYaml(box, textIn(box).replace(/prefix: .*/, 'prefix: ""'));
    expect(leavingAsks()).toBe(false);
    await userEvent.click(close());
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
  });

  it('asks before an item is opened from the folders, and opens it only after "Discard my changes"', async () => {
    const { api } = service();
    const { router } = await openRules();
    await setPrefix('unsaved-');
    const tree = screen.getByRole('tree', { name: 'Folders' });
    const report = within(tree).getByRole('treeitem', { name: /Annual report/ });
    const readsBefore = api.callsTo('/api/reads/read-item').length;
    await userEvent.click(report);
    expect(question()).not.toBeNull();
    await userEvent.click(button('Keep editing'));
    // Nothing was opened behind the question: the address and the requests are as they were.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    expect(api.callsTo('/api/reads/read-item')).toHaveLength(readsBefore);
    // Back to what asked to leave: the row that was pressed.
    expect(document.activeElement).toBe(report);
    expect(prefix()).toHaveProperty('value', 'unsaved-');
    await userEvent.click(report);
    await userEvent.click(button('Discard my changes'));
    expect(await screen.findByText('Text of Annual report')).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
    // The pressed button went with the rules: focus is back on the row that was opened.
    expect(document.activeElement).toBe(report);
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
    await typeYaml(await findTextBox('The naming rules as YAML'), 'rules: [\n');
    expect(leavingAsks()).toBe(true);
    await userEvent.click(close());
    expect(question()).not.toBeNull();
  });

  it('counts text that cannot be read as an edit though the rules are the saved ones', async () => {
    service();
    await openRules();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    await typeYaml(await findTextBox('The naming rules as YAML'), 'rules: [\n');
    // The rules are still the last that could be read: the saved ones. The text is the edit.
    expect(panel().textContent).toContain('These are the saved rules, unchanged.');
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

describe('leaving the workspace page with rule edits that are not saved', () => {
  const SEARCH = '/api/search/search-items';
  const question = () =>
    screen.queryByRole('group', { name: 'Leave the naming rules without saving?' });
  const home = () => screen.getByRole('link', { name: 'okf-jawn' });
  const found = () =>
    ok({
      revision: HEAD,
      hits: [
        {
          title: 'Meeting notes',
          snippet: '…matching words…',
          score: 1,
          source: {
            workspace_id: WS,
            item_id: NOTE,
            path: 'notes/meeting.md',
            revision: HEAD,
            selection: { kind: 'lines', range: { start: 1, end: 2 } },
            locations: [],
          },
        },
      ],
    });
  const hit = async () =>
    within(await screen.findByRole('region', { name: 'Results for “budget”' })).findByRole('link', {
      name: /Meeting notes/,
    });

  it('asks before a link to the workspace list is followed, stays on "Keep editing", and leaves only when told', async () => {
    service();
    const { router } = await openRules();
    await setPrefix('unsaved-');
    await userEvent.click(home());
    // Still on the workspace, with the question; focus on the choice that loses nothing.
    expect(question()).not.toBeNull();
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    expect(document.activeElement).toBe(button('Keep editing'));
    await userEvent.click(button('Keep editing'));
    expect(question()).toBeNull();
    expect(document.activeElement).toBe(home());
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    expect(prefix()).toHaveProperty('value', 'unsaved-');
    await userEvent.click(home());
    await userEvent.click(button('Discard my changes'));
    await waitFor(() => expect(router.state.location.pathname).toBe('/'));
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
    // The pressed button went with the question: focus is on the link that was followed.
    expect(document.activeElement).toBe(home());
  });

  it('follows a link out at once when the rules hold no edit', async () => {
    service();
    const { router } = await openRules();
    await userEvent.click(home());
    await waitFor(() => expect(router.state.location.pathname).toBe('/'));
    expect(question()).toBeNull();
  });

  it('asks before a search result’s link is followed, and then shows the item in place of the rules', async () => {
    const { api } = service({ [SEARCH]: found });
    const { router } = await openRules(`/w/${WS}?q=budget`);
    await setPrefix('unsaved-');
    const link = await hit();
    const readsBefore = api.callsTo('/api/reads/read-item').length;
    await userEvent.click(link);
    expect(question()).not.toBeNull();
    await userEvent.click(button('Keep editing'));
    // Nothing was opened behind the question.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    expect(api.callsTo('/api/reads/read-item')).toHaveLength(readsBefore);
    expect(document.activeElement).toBe(link);
    await userEvent.click(link);
    await userEvent.click(button('Discard my changes'));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    // The item is on screen: the rules gave the documents their place back.
    expect(await screen.findByText('Text of Meeting notes')).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
    expect(document.activeElement).toBe(link);
  });

  it('shows the item of a search result’s link at once when the rules hold no edit', async () => {
    service({ [SEARCH]: found });
    const { router } = await openRules(`/w/${WS}?q=budget`);
    await userEvent.click(await hit());
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`));
    expect(question()).toBeNull();
    expect(await screen.findByText('Text of Meeting notes')).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull();
  });

  it('shows the item of a search result’s link when it is the item the address already names', async () => {
    service({ [SEARCH]: found });
    // The address names the note, as read at the head; the result cites lines of it.
    const { router } = await openRules(`/w/${WS}/i/${NOTE}?q=budget`);
    // The documents are behind the rules: not on the page for anyone.
    expect(screen.queryByRole('region', { name: 'Documents' })).toBeNull();
    await userEvent.click(await hit());
    await waitFor(() => expect(router.state.location.search).toMatchObject({ rev: HEAD }));
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${NOTE}`);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Naming rules' })).toBeNull());
    expect(screen.getByRole('region', { name: 'Documents' })).toBeTruthy();
  });

  it('asks nothing, and keeps the rules open, when only the search changes', async () => {
    service({ [SEARCH]: found });
    const { router } = await openRules();
    await setPrefix('unsaved-');
    await userEvent.type(
      screen.getByRole('searchbox', { name: 'Search this workspace' }),
      'budget{Enter}',
    );
    await waitFor(() => expect(router.state.location.search).toEqual({ q: 'budget' }));
    await hit();
    expect(question()).toBeNull();
    expect(prefix()).toHaveProperty('value', 'unsaved-');
  });
});

describe('saving over rules someone else saved meanwhile', () => {
  const theirs: NamingRules = {
    schema_version: 1,
    rules: [{ ...rule, prefix: 'theirs-', separator: '_' }],
  };
  const savedNow = () =>
    screen.queryByRole('textbox', { name: 'The rules as they are saved now (read-only)' });
  const replaceQuestion = () =>
    screen.queryByRole('group', { name: 'Replace the saved rules with yours?' });
  /** The person edits; someone else saves other rules; the person's save is refused; they refresh. */
  async function refusedThenRefreshed() {
    const own = service();
    await openRules();
    await setPrefix('mine-');
    const next = own.moveHead();
    own.state.rules.set(next, theirs);
    await userEvent.click(button('Save the rules'));
    await userEvent.click(await screen.findByRole('button', { name: 'Refresh the workspace' }));
    await waitFor(() => expect(savedNow()).not.toBeNull());
    return { ...own, next };
  }

  it('shows the rules as they are saved now beside the person’s own, and names the save for what it does', async () => {
    const { api } = await refusedThenRefreshed();
    const shown = savedNow() as HTMLTextAreaElement;
    expect(shown.readOnly).toBe(true);
    expect(shown.value).toContain('prefix: theirs-');
    expect(shown.value).toContain('separator: _');
    // The person's own rules are still in the form, untouched.
    expect(prefix()).toHaveProperty('value', 'mine-');
    expect(panel().textContent).toContain(
      'The saved rules are no longer the ones you started from.',
    );
    // No plain "Save": the button says that it replaces.
    expect(screen.queryByRole('button', { name: 'Save the rules' })).toBeNull();
    expect(button('Replace the saved rules with mine…')).toBeTruthy();
    expect(api.callsTo(SET)).toHaveLength(1);
  });

  it('asks before it replaces, keeps the saved rules on that choice, and sends the person’s rules whole on the other', async () => {
    const { api, next } = await refusedThenRefreshed();
    const replace = button('Replace the saved rules with mine…');
    await userEvent.click(replace);
    expect(replaceQuestion()).not.toBeNull();
    expect(document.activeElement).toBe(button('Keep the saved rules'));
    expect(api.callsTo(SET)).toHaveLength(1);
    await userEvent.keyboard('{Enter}');
    expect(replaceQuestion()).toBeNull();
    expect(document.activeElement).toBe(replace);
    expect(api.callsTo(SET)).toHaveLength(1);

    await userEvent.click(replace);
    await userEvent.click(button('Replace them with mine'));
    await waitFor(() => expect(said()).toContain('The rules are saved.'));
    expect(api.callsTo(SET).at(1)).toMatchObject({
      base_revision: next,
      rules: { schema_version: 1, rules: [{ ...rule, prefix: 'mine-' }] },
    });
    // Saved, they are the saved rules: nothing is beside them any more, and Save is Save again.
    await waitFor(() => expect(savedNow()).toBeNull());
    expect(button('Save the rules')).toBeTruthy();
  });

  it('takes the saved rules and drops the edits when the person chooses so', async () => {
    const { api } = await refusedThenRefreshed();
    // With the YAML view on screen: its text starts again from the saved rules too.
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    expect(textIn(await findTextBox('The naming rules as YAML'))).toContain('prefix: mine-');
    await userEvent.click(button('Take the saved rules, and drop mine'));
    expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Naming rules' }));
    expect(textIn(await findTextBox('The naming rules as YAML'))).toContain('prefix: theirs-');
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    expect(prefix()).toHaveProperty('value', 'theirs-');
    expect(savedNow()).toBeNull();
    expect(said()).toBe('These are the saved rules now. Your changes were dropped.');
    expect(panel().textContent).toContain('These are the saved rules, unchanged.');
    expect(api.callsTo(SET)).toHaveLength(1);
  });

  /**
   * The same journey, with the rules of the new head answered by `atNewHead`: edit, be refused
   * because someone else moved the head, refresh. (The re-review's probes R10b and R10c.)
   */
  async function refreshedWith(atNewHead: () => Reply | Promise<Reply>) {
    const holder: { next: string } = { next: '' };
    const own = service({
      [RULES]: (body) =>
        holder.next !== '' && revisionAsked(body, HEAD) === holder.next
          ? atNewHead()
          : ok(savedRules),
    });
    await openRules();
    await setPrefix('mine-');
    holder.next = own.moveHead();
    own.state.rules.set(holder.next, theirs);
    await userEvent.click(button('Save the rules'));
    await userEvent.click(await screen.findByRole('button', { name: 'Refresh the workspace' }));
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    return own;
  }
  /** The words beside the save button that say why it is unavailable. */
  const whyNot = (save: HTMLElement) =>
    document.getElementById(save.getAttribute('aria-describedby') ?? '')?.textContent ?? '';

  it('offers no save while the rules of the version shown are still being read, and then the replacing one', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const own = await refreshedWith(
      () =>
        new Promise<Reply>((resolve) => {
          gate.release = () => resolve(ok(theirs));
        }),
    );
    // The rules on screen are the ones read before: saving over rules never shown is not offered.
    const save = button('Save the rules');
    expect(save.getAttribute('aria-disabled')).toBe('true');
    expect(whyNot(save)).toContain(
      'Saving is not available now: the rules of the version shown, revision cccccccccc, are still being read.',
    );
    await userEvent.click(save);
    await turn();
    expect(own.api.callsTo(SET)).toHaveLength(1);
    expect(replaceQuestion()).toBeNull();
    // Read, they are shown beside the person's own, and the button is the replacing one.
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(savedNow()).not.toBeNull());
    expect(screen.queryByRole('button', { name: 'Save the rules' })).toBeNull();
    const replace = button('Replace the saved rules with mine…');
    expect(replace.getAttribute('aria-disabled')).not.toBe('true');
    expect(replace.getAttribute('aria-describedby')).toBeNull();
    expect(panel().textContent).not.toContain('Saving is not available now');
    expect(own.api.callsTo(SET)).toHaveLength(1);
  });

  it('offers no save when the rules of the version shown cannot be read: the failure has its retry', async () => {
    let failing = true;
    const own = await refreshedWith(() =>
      failing ? refuse(500, 'internal', 'the rules file broke') : ok(theirs),
    );
    const alert = await screen.findByRole('alert', {}, { timeout: 4_000 });
    expect(alert.textContent).toContain('Reading the naming rules');
    const save = button('Save the rules');
    expect(save.getAttribute('aria-disabled')).toBe('true');
    expect(whyNot(save)).toContain(
      'Saving is not available now: the rules of the version shown, revision cccccccccc, could not be read.',
    );
    await userEvent.click(save);
    await turn();
    expect(own.api.callsTo(SET)).toHaveLength(1);
    // The person's rules are still on the page.
    expect(prefix()).toHaveProperty('value', 'mine-');
    // Read again, the rules of that version are shown, and the save is the replacing one.
    failing = false;
    await userEvent.click(button('Try again'));
    await waitFor(() => expect(savedNow()).not.toBeNull());
    expect(button('Replace the saved rules with mine…')).toBeTruthy();
    expect(own.api.callsTo(SET)).toHaveLength(1);
  });

  it('is an ordinary save when the version shown moved and the saved rules did not', async () => {
    const own = service();
    await openRules();
    await setPrefix('mine-');
    own.moveHead();
    await userEvent.click(button('Save the rules'));
    await userEvent.click(await screen.findByRole('button', { name: 'Refresh the workspace' }));
    await waitFor(() =>
      expect(screen.getByText(/Showing the workspace as of/).textContent).toContain('cccccccccc'),
    );
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(said()).toContain('The rules are saved.'));
    expect(savedNow()).toBeNull();
    expect(replaceQuestion()).toBeNull();
  });
});

describe('what the two views of the rules say and keep', () => {
  it('keeps keys in the order they had: shared keys stay, new ones go last, lists place by place', () => {
    expect(
      Object.keys(keepOrder({ b: 1, a: 2, gone: 3 }, { a: 20, b: 10, added: 5 }) as object),
    ).toEqual(['b', 'a', 'added']);
    expect(keepOrder({ b: 1, a: 2 }, { a: 20, b: 10 })).toEqual({ b: 10, a: 20 });
    const kept = keepOrder(
      { rules: [{ prefix: '', glob: 'x' }], schema_version: 1 },
      {
        schema_version: 1,
        rules: [
          { glob: 'x', prefix: 'p' },
          { glob: 'y', prefix: '' },
        ],
      },
    ) as { rules: object[] };
    expect(Object.keys(kept)).toEqual(['rules', 'schema_version']);
    expect(kept.rules.map((each) => Object.keys(each))).toEqual([
      ['prefix', 'glob'],
      ['glob', 'prefix'],
    ]);
    // Nothing but the order differs, and what is no mapping is given back as it is.
    expect(keepOrder('x', { a: [1, { b: null }] })).toEqual({ a: [1, { b: null }] });
    expect(keepOrder({ a: 1 }, 'text')).toBe('text');
    expect(keepOrder({ a: 1 }, null)).toBeNull();
  });

  it('leaves every key where the YAML had it when the form changes one value', async () => {
    service();
    await openRules();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const box = await findTextBox('The naming rules as YAML');
    // The person's own order: the rules first, and the prefix first in the rule.
    const mine = [
      'rules:',
      '  - prefix: ""',
      '    glob: "**/*.md"',
      '    collision: reject',
      '    case: kebab',
      '    separator: "-"',
      '    date_order: ymd',
      '    date_format: "%Y-%m-%d"',
      '    strip_version_suffix: true',
      '    extension_policy: preserve',
      '    alias_preservation: true',
      'schema_version: 1',
      '',
    ].join('\n');
    await typeYaml(box, mine);
    await userEvent.click(screen.getByRole('tab', { name: 'Form' }));
    await userEvent.type(prefix(), 'x-');
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    const after = textIn(await findTextBox('The naming rules as YAML'));
    const keys = (text: string) => [...text.matchAll(/^\s*-?\s*([a-z_]+):/gm)].map((m) => m[1]);
    expect(keys(after)).toEqual(keys(mine));
    expect(after).toContain('prefix: x-');
  });

  it('says in the YAML view, before anything is typed, that comments are not kept, and why', async () => {
    service();
    await openRules();
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }));
    await findTextBox('The naming rules as YAML');
    const note = within(panel()).getByText(/Comments are not kept\./);
    expect(note.textContent).toContain('The service stores the rules as data, not as text');
    expect(note.getAttribute('role')).toBe('note');
  });

  it('says in a preview that archived items are left out, and that their number cannot be told', async () => {
    service();
    await openRules();
    await setPrefix('2025-');
    await previewNow();
    const preview = screen.getByRole('region', { name: 'Preview: what would be renamed' });
    expect(preview.textContent).toContain(
      'Archived items are not listed in the workspace as it is now, so they are not in this preview and would not be renamed. How many there are cannot be told here.',
    );
  });
});

describe('the rules on screen when the version shown changes', () => {
  const headLine = () => screen.getByText(/Showing the workspace as of/).textContent ?? '';

  it('keeps the form, and focus in it, when the rules cannot be read at the new head, and says which rules these are', async () => {
    let failing = false;
    const own = service({
      [RULES]: () => (failing ? refuse(500, 'internal', 'the store broke') : ok(savedRules)),
    });
    await openRules();
    failing = true;
    own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    act(() => prefix().focus());
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Reading the naming rules');
    // Nothing on screen was replaced: the form is there, with focus where it was.
    expect(prefix()).toHaveProperty('value', '');
    expect(document.activeElement).toBe(prefix());
    expect(panel().textContent).toContain(
      'These are the rules as they were read at revision aaaaaaaaaa: the rules of the version shown, revision cccccccccc, could not be read.',
    );
    // Read again, they are the rules of the version shown.
    failing = false;
    await userEvent.click(button('Try again'));
    await waitFor(() =>
      expect(panel().textContent).toContain(
        'These are the rules of the version shown, revision cccccccccc.',
      ),
    );
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('says that the rules were saved at a revision the page does not show, when the head moved on meanwhile', async () => {
    const gate: { release: () => void } = { release: () => {} };
    const holder: { commit: () => string } = { commit: () => '' };
    const own = service({
      [SET]: () =>
        new Promise<Reply>((resolve) => {
          gate.release = () =>
            resolve(ok({ revision: holder.commit(), receipt_id: RECEIPT, warnings: [] }));
        }),
    });
    holder.commit = own.moveHead;
    await openRules();
    await setPrefix('2025-');
    await userEvent.click(button('Save the rules'));
    await waitFor(() => expect(own.api.callsTo(SET)).toHaveLength(1));
    own.moveHead();
    await userEvent.click(await screen.findByRole('button', { name: /^Refresh/ }));
    await waitFor(() => expect(headLine()).toContain('cccccccccc'));
    await act(async () => {
      gate.release();
      await Promise.resolve();
    });
    await waitFor(() => expect(said()).toBe('The rules are saved, at revision dddddddddd.'));
    expect(headLine()).toContain('cccccccccc');
    expect(panel().parentElement?.textContent).toContain(
      'The change was made, but the workspace shown here moved on while it was asked',
    );
  });
});
