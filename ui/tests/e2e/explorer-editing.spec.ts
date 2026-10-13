/**
 * Editing a document from the keyboard, in Chromium and WebKit: typing with autosave and
 * reopening the draft; a Snapshot that succeeds; a Snapshot that conflicts, resolved once each
 * way; a failed save retried; a property changed in its form; properties that do not fit a strict
 * type; one item in two panes. Every journey ends with keyboard focus on a sensible element and
 * zero axe violations, and saves a screenshot of its screen under `.artifacts/ui/` for the owner.
 *
 * The service is played by `page.route` (no server exists in this lane): it remembers the drafts
 * it holds and its head, and it checks the CSRF header of every request it is sent.
 *
 * Needs `bun --bun run build` first: the spec serves the built `dist/` itself.
 */

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
  routeApi,
  shot,
  useExplorerServer,
  WS,
  writableWorkspace,
} from './support/explorer';

type Body = Record<string, unknown>;

const TOKEN = '5f1d7c0e9a4b4c2e8d3f6a1b0c9e8d7f';
const LATER = '2026-01-02T08:15:00.000Z';
/** The revision a Snapshot makes. */
const MADE = 'c'.repeat(40);
/** A revision someone else made meanwhile. */
const THEIRS = 'e'.repeat(40);
const RECEIPT = '55555555-5555-4555-8555-555555555555';

const PATCH = [
  'diff --git a/notes/meeting.md b/notes/meeting.md',
  'index 83db48f..bf269f4 100644',
  '--- a/notes/meeting.md',
  '+++ b/notes/meeting.md',
  '@@ -3,3 +3,4 @@ # Meeting notes',
  ' The group agreed to **publish the budget** after review.',
  '-Revenue grew by 12% in 2025.',
  '+Revenue grew by 14% in 2025.',
  '+The auditors have signed.',
  '',
].join('\n');

const meetingNote = {
  name: 'meeting-note',
  schema_version: 1,
  properties_schema: {
    type: 'object',
    required: ['owner'],
    properties: {
      owner: { type: 'string', title: 'Owner', minLength: 1 },
      meeting_date: { type: 'string', title: 'Meeting date' },
    },
  },
  ui_schema: {},
};

const draftOf = (itemId: string, base: string) => ({
  item_id: itemId,
  base_revision: base,
  content_digest: 'c'.repeat(64),
  editor: 'ada',
  saved_at: LATER,
});

interface Service {
  answers: Answers;
  /** Every `save_draft` and `commit_items` request, in the order they came. */
  sent: Array<{ path: string; body: Body }>;
  /** Make the next saves fail, hold them, or answer them. */
  saving: (mode: 'ok' | 'fail' | 'hold') => void;
  /** Answer the saves that are held. */
  release: () => void;
  held: () => number;
}

/**
 * A service that remembers. `drafts`: the items the person already has a draft of. `conflictOn`:
 * an item someone else changed, so a Snapshot of it conflicts until its draft is saved on their
 * revision. `strict`: the note's type allows no property it does not name.
 */
function service(
  options: { drafts?: string[]; conflictOn?: string; strict?: boolean } = {},
): Service {
  const state = {
    head: HEAD,
    drafts: new Map<string, { body: string; properties: Body; base: string }>(),
    saving: 'ok' as 'ok' | 'fail' | 'hold',
    waiting: [] as Array<() => void>,
  };
  const properties = (id: string) => ({
    type: id === NOTE ? 'meeting-note' : 'plan',
    owner: 'Ada Lovelace',
    meeting_date: '2025-10-03',
    'x-okf-review': { by: 'grace' },
    okf_jawn: { id },
  });
  for (const id of options.drafts ?? []) {
    state.drafts.set(id, {
      body: `${markdown}\n\nMy draft of it.`,
      properties: properties(id),
      base: HEAD,
    });
  }
  const sent: Service['sent'] = [];
  const answers: Answers = {
    '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head: state.head }),
    '/api/types/list-types': () =>
      ok({
        items: [
          options.strict === true
            ? {
                ...meetingNote,
                properties_schema: {
                  ...meetingNote.properties_schema,
                  additionalProperties: false,
                },
              }
            : meetingNote,
        ],
      }),
    '/api/items/get-item': (body) => {
      const id = String(body.item_id);
      const held = state.drafts.get(id);
      return ok({
        summary: id === NOTE ? note : plan,
        body: markdown,
        content_digest: 'c'.repeat(64),
        properties: properties(id),
        ...(held === undefined
          ? {}
          : {
              draft: {
                body: held.body,
                properties: held.properties,
                draft: draftOf(id, held.base),
              },
            }),
      });
    },
    '/api/items/list-drafts': () =>
      ok({
        items: [...state.drafts].map(([id, held]) => draftOf(id, held.base)),
        next_cursor: null,
      }),
    '/api/items/save-draft': async (body) => {
      sent.push({ path: 'save_draft', body });
      if (state.saving === 'fail') {
        return { status: 503, body: { code: 'unavailable', message: 'the store is busy' } };
      }
      if (state.saving === 'hold')
        await new Promise<void>((resolve) => state.waiting.push(resolve));
      const id = String(body.item_id);
      const before = state.drafts.get(id);
      // The service's rule: a first save records the base the request carries; a later one keeps
      // the draft's base unless it carries the current head, which is the explicit rebase.
      const base =
        before === undefined || body.base_revision === state.head
          ? String(body.base_revision)
          : before.base;
      state.drafts.set(id, {
        body: String(body.body),
        properties: (body.properties ?? {}) as Body,
        base,
      });
      return ok(draftOf(id, base));
    },
    '/api/items/discard-draft': (body) => {
      const id = String(body.item_id);
      const held = state.drafts.get(id);
      state.drafts.delete(id);
      return ok(draftOf(id, held?.base ?? HEAD));
    },
    '/api/history/commit-items': (body): Reply => {
      sent.push({ path: 'commit_items', body });
      const ids = Array.isArray(body.item_ids) ? body.item_ids.map(String) : [];
      const conflicting = ids.filter(
        (id) => id === options.conflictOn && state.drafts.get(id)?.base !== THEIRS,
      );
      if (conflicting.length > 0) {
        state.head = THEIRS;
        return {
          status: 409,
          body: {
            code: 'conflict',
            message: 'a selected item changed since its draft was started',
            detail: {
              kind: 'draft_conflict',
              items: conflicting.map((id) => ({
                item_id: id,
                draft_base: HEAD,
                current_revision: THEIRS,
                deleted: false,
                changes: [
                  {
                    kind: 'modified',
                    binary: false,
                    old_path: 'notes/meeting.md',
                    new_path: 'notes/meeting.md',
                    patch: PATCH,
                  },
                ],
              })),
            },
          },
        };
      }
      for (const id of ids) state.drafts.delete(id);
      state.head = MADE;
      return ok({ revision: MADE, receipt_id: RECEIPT, warnings: [] });
    },
  };
  return {
    answers,
    sent,
    saving: (mode) => {
      state.saving = mode;
    },
    release: () => {
      for (const answer of state.waiting.splice(0)) answer();
    },
    held: () => state.waiting.length,
  };
}

/** The CSRF header of every request the page sent to the service. */
function recordCsrf(page: Page): Array<string | undefined> {
  const seen: Array<string | undefined> = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/api/') && request.method() !== 'GET') {
      seen.push(request.headers()['x-okf-csrf']);
    }
  });
  return seen;
}

const textBox = (page: Page) => page.getByRole('textbox', { name: 'Text of Meeting notes' });
const saveState = (page: Page) => page.locator('.save-state');
const drafts = (page: Page) => page.getByRole('region', { name: /^Drafts/ });
const said = (page: Page) => page.locator('.snapshot-said');

/** Press a control from the keyboard: focus it, then Enter. */
async function press(page: Page, name: string | RegExp) {
  const button = page.getByRole('button', { name });
  // A control that is not available yet (the item is still being read) ignores the press.
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  await button.focus();
  await page.keyboard.press('Enter');
}

/** Keyboard focus 300 ms on, after the frame in which a browser moves it off a removed control. */
async function focusSettled(page: Page): Promise<string> {
  await page.waitForTimeout(300);
  return focused(page);
}

test.describe('editing a document', () => {
  const server = useExplorerServer();

  test('type, autosave and reopen', async ({ page, context }) => {
    const played = service();
    await routeApi(page, played.answers);
    await context.addCookies([{ name: 'okf-csrf', value: TOKEN, url: server.origin() }]);
    const csrf = recordCsrf(page);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);

    await press(page, 'Edit');
    await expect(textBox(page)).toBeFocused();
    await expect(saveState(page)).toHaveText('No changes to save.');
    await page.keyboard.press('Control+End');
    // Appended to the last line: Enter in a list would continue the list, as a Markdown editor does.
    await page.keyboard.type(' Typed by a person.');
    await expect(saveState(page)).toHaveText('Unsaved changes.');
    await expect(saveState(page)).toContainText('Saved as a draft at');
    expect(played.sent.at(-1)).toMatchObject({
      path: 'save_draft',
      body: { item_id: NOTE, base_revision: HEAD, body: `${markdown} Typed by a person.` },
    });
    await expect(page.getByRole('tab', { name: 'Meeting notes (draft)' })).toBeVisible();
    await expect(drafts(page)).toHaveAccessibleName('Drafts (1)');
    expect(await focusSettled(page)).toBe('textbox:Text of Meeting notes');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the draft editor');
    await shot(page, '11-draft-editor');

    // Every request to the service carried the cookie's value in the header, reads included.
    // (Counted as words: `toEqual` reads an array of missing values as an empty one.)
    expect(csrf.length).toBeGreaterThan(5);
    expect(csrf.filter((value) => value !== TOKEN).map((value) => value ?? 'no header')).toEqual(
      [],
    );

    // Reload: the item opens on the draft the service holds, marked as one.
    await page.reload();
    await expect(textBox(page)).toContainText('Typed by a person.');
    await expect(page.getByText('Draft.', { exact: true })).toBeVisible();
    await expect(saveState(page)).toContainText('Saved as a draft at');
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toHaveCount(1);

    // Reopen from the keyboard: close the tab, then open the item from the list of drafts.
    await page.getByRole('tab', { name: 'Meeting notes (draft)' }).focus();
    await page.keyboard.press('Delete');
    await expect(page.getByRole('tab')).toHaveCount(0);
    await drafts(page).getByRole('button', { name: 'Meeting notes' }).focus();
    await page.keyboard.press('Enter');
    await expect(textBox(page)).toContainText('Typed by a person.');
    expect(await focusSettled(page)).toBe('button:Meeting notes');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the reopened draft');
  });

  test('a Snapshot that succeeds', async ({ page }) => {
    const played = service({ drafts: [NOTE, PLAN] });
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(drafts(page)).toHaveAccessibleName('Drafts (2)');
    await expect(textBox(page)).toBeVisible();

    await page.getByRole('textbox', { name: 'Snapshot name' }).focus();
    await page.keyboard.type('Tidy the notes');
    await expectNoAxeViolations(page, 'the drafts with a Snapshot named');
    await shot(page, '12-snapshot-form');
    await press(page, 'Snapshot 2 drafts');
    await expect(said(page)).toHaveText(
      `Snapshot “Tidy the notes” taken: 2 items. The workspace now shows revision ${MADE.slice(0, 10)}, which the Snapshot made.`,
    );
    expect(played.sent.at(-1)).toMatchObject({
      path: 'commit_items',
      body: { item_ids: [NOTE, PLAN], message: 'Tidy the notes' },
    });
    // The shell shows the revision the Snapshot made, without a Refresh.
    await expect(page.locator('.head-line')).toContainText(`revision ${MADE.slice(0, 10)}`);
    await expect(drafts(page)).toHaveAccessibleName('Drafts (0)');
    await expect(page.getByRole('tab', { name: 'Meeting notes', exact: true })).toBeVisible();
    // The button never left: it has focus, and says there is nothing to snapshot.
    expect(await focusSettled(page)).toBe('button:Snapshot 0 drafts');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'after a Snapshot');
    await shot(page, '13-snapshot-taken');
  });

  /** A Snapshot of the note and the plan that conflicts on the note; its notice is on screen. */
  async function conflict(page: Page) {
    const played = service({ drafts: [NOTE, PLAN], conflictOn: NOTE });
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(drafts(page)).toHaveAccessibleName('Drafts (2)');
    await page.getByRole('textbox', { name: 'Snapshot name' }).focus();
    await page.keyboard.type('Tidy');
    await page.keyboard.press('Enter');
    await expect(drafts(page).getByRole('alert')).toContainText('Nothing was snapshotted.');
    const notice = page.getByRole('region', {
      name: 'Meeting notes changed since your draft was started',
    });
    await expect(notice).toBeVisible();
    return { played, notice };
  }

  test('a Snapshot that conflicts: keep mine', async ({ page }) => {
    const { played, notice } = await conflict(page);
    await expect(notice.locator('del')).toHaveText('Removed: − Revenue grew by 12% in 2025.');
    await expect(notice.locator('ins')).toHaveText([
      'Added: + Revenue grew by 14% in 2025.',
      'Added: + The auditors have signed.',
    ]);
    await expect(notice).not.toContainText('diff --git');
    // Someone else's head is announced, not adopted.
    await expect(page.locator('.head-line')).toContainText(
      'A newer version of this workspace exists',
    );
    await expect(page.locator('.head-line')).toContainText(`revision ${HEAD.slice(0, 10)}`);
    // Enter in the name field took the Snapshot: focus is still there.
    expect(await focusSettled(page)).toBe('input:');
    await expectNoAxeViolations(page, 'a conflict');
    await shot(page, '14-conflict');

    const before = played.sent.length;
    await press(page, 'Keep mine');
    await expect(said(page)).toContainText('Snapshot “Tidy” taken: 1 item.');
    const after = played.sent.slice(before);
    expect(after.map((request) => request.path)).toEqual(['save_draft', 'commit_items']);
    expect(after.at(0)?.body).toMatchObject({ item_id: NOTE, base_revision: THEIRS });
    expect(after.at(1)?.body).toMatchObject({ item_ids: [NOTE], message: 'Tidy' });
    await expect(notice).toHaveCount(0);
    // The pane reads what was committed, and focus is on its switch.
    expect(await focusSettled(page)).toBe('button:Edit');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'after keeping mine');
  });

  test('a Snapshot that conflicts: take theirs', async ({ page }) => {
    const { played, notice } = await conflict(page);
    const commits = played.sent.filter((request) => request.path === 'commit_items').length;
    await press(page, 'Take theirs');
    const question = notice.getByRole('group', {
      name: 'Discard your draft and take the newer version?',
    });
    await expect(question).toBeVisible();
    expect(await focusSettled(page)).toBe('button:Keep my draft');
    await expectNoAxeViolations(page, 'the question before taking theirs');
    await shot(page, '15-take-theirs-question');
    await press(page, 'Discard my draft');
    await expect(notice).toHaveCount(0);
    await expect(said(page)).toHaveText(
      'Your draft of Meeting notes was discarded. The newer version is shown once the workspace is refreshed.',
    );
    expect(played.sent.filter((request) => request.path === 'commit_items')).toHaveLength(commits);
    await expect(drafts(page)).toHaveAccessibleName('Drafts (1)');
    expect(await focusSettled(page)).toBe('button:Edit');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'after taking theirs');
  });

  test('a failed save retried by keyboard', async ({ page }) => {
    const played = service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await press(page, 'Edit');
    await expect(textBox(page)).toBeFocused();
    played.saving('fail');
    await page.keyboard.press('Control+End');
    await page.keyboard.type(' Words to keep.');
    await expect(
      page.getByText('Saving this draft: The service is temporarily unavailable.'),
    ).toBeVisible();
    await expect(saveState(page)).toHaveText('Not saved. Your text is kept here.');
    await expect(page.getByRole('tab', { name: 'Meeting notes (not saved)' })).toBeVisible();
    await expect(textBox(page)).toContainText('Words to keep.');

    // Tab leaves the text box (it is not trapped) and lands on the notice's retry.
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Try again' })).toBeFocused();
    played.saving('hold');
    await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Trying again…' })).toBeFocused();
    await expect.poll(() => played.held()).toBe(1);
    expect(await focusSettled(page)).toBe('button:Trying again…');
    await expectNoAxeViolations(page, 'a save being tried again');
    await shot(page, '16-save-trying-again');

    played.saving('ok');
    played.release();
    await expect(saveState(page)).toContainText('Saved as a draft at');
    await expect(page.getByRole('button', { name: /Try again|Trying again/ })).toHaveCount(0);
    expect(await focusSettled(page)).toBe('textbox:Text of Meeting notes');
    await expectVisibleFocus(page);
    // The retry repeated the failed request: the same text under the same key.
    const [failed, retried] = played.sent.slice(-2);
    expect(retried?.body).toEqual(failed?.body);
    await expectNoAxeViolations(page, 'after the save was retried');
  });

  test('a property changed in its form', async ({ page }) => {
    const played = service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await press(page, 'Edit');
    const owner = page.getByLabel(/^Owner/);
    await expect(owner).toHaveValue('Ada Lovelace');
    await owner.focus();
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.type('Grace Hopper');
    await expect(saveState(page)).toContainText('Saved as a draft at');
    expect(played.sent.at(-1)?.body.properties).toEqual({
      type: 'meeting-note',
      owner: 'Grace Hopper',
      meeting_date: '2025-10-03',
      'x-okf-review': { by: 'grace' },
      okf_jawn: { id: NOTE },
    });
    // Emptied, the required field shows its error where it is, and nothing more is sent.
    const sent = played.sent.length;
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.press('Delete');
    await expect(page.locator('#root_owner__error')).toBeVisible();
    await page.waitForTimeout(1_500);
    expect(played.sent).toHaveLength(sent);
    expect(await focusSettled(page)).toMatch(/^input:/);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the properties form with an error');
    await shot(page, '17-properties-form');
  });

  test('properties that do not fit a strict type', async ({ page }) => {
    const played = service({ strict: true });
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await press(page, 'Edit');
    // The item holds properties its type does not allow: said above the form, where no field is.
    const above = page.locator('.form-said');
    await expect(above).toContainText('These properties do not fit the type meeting-note:');
    await expect(above).toContainText('“x-okf-review” is not a property this type allows.');
    await expect(above).not.toContainText('held back');
    const owner = page.getByLabel(/^Owner/);
    await owner.focus();
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.type('Grace Hopper');
    await expect(above).toContainText(
      'What you changed in the properties is held back, and is not saved until the properties fit the type.',
    );
    await page.waitForTimeout(1_500);
    expect(played.sent.filter((request) => request.path === 'save_draft')).toHaveLength(0);
    await expect(owner).toHaveValue('Grace Hopper');
    // Said once, above the form: not again under it in the validator's words.
    await expect(page.locator('.properties')).not.toContainText('must NOT have additional');
    expect(await focusSettled(page)).toMatch(/^input:/);
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'properties that do not fit their type');
    await shot(page, '18-properties-misfits');
  });

  test('one item in two panes: one editor, moved with "Edit here"', async ({ page }) => {
    const played = service();
    await routeApi(page, played.answers);
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await press(page, 'Edit');
    await expect(textBox(page)).toBeFocused();
    await page.keyboard.press('Control+End');
    await page.keyboard.type(' First words.');
    await expect(saveState(page)).toContainText('Saved as a draft at');
    await press(page, 'Split right');
    const second = page.getByRole('region', { name: /^Documents, pane 2 of 2/ });
    await expect(
      second.getByText('This item is being edited in another pane. Below is the committed text.'),
    ).toBeVisible();
    await expect(second.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    await expect(textBox(page)).toHaveCount(1);
    await expectNoAxeViolations(page, 'two panes of one item, one editor');
    await shot(page, '19-edit-here');

    await press(page, 'Edit here');
    await expect(second.getByRole('textbox', { name: 'Text of Meeting notes' })).toBeFocused();
    await expect(textBox(page)).toHaveCount(1);
    await expect(textBox(page)).toContainText('First words.');
    await expect(saveState(page)).toContainText('Saved as a draft at');
    expect(await focusSettled(page)).toBe('textbox:Text of Meeting notes');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'after "Edit here"');
    expect(played.sent.filter((request) => request.path === 'save_draft')).toHaveLength(1);
  });
});
