/**
 * History and review in the browser, from the keyboard, in Chromium and WebKit: the Timeline to
 * a version's Changes to Rewind; a proposal opened, commented on and accepted through its
 * confirmation; an item verified through its confirmation; the graph and its list. Each screen
 * has zero axe violations and visible focus, and a screenshot for the owner under `.artifacts/ui/`
 * (untracked). The API is answered through `page.route` (`support/explorer.ts`).
 */

import { expect, type Locator, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  NOTE,
  note,
  ok,
  PLAN,
  plan,
  routeApi,
  SOURCE,
  shot,
  source,
  useExplorerServer,
  WS,
  workspace,
} from './support/explorer';

const OLD = 'b'.repeat(40);
const OLDER = 'e'.repeat(40);
const FIRST = 'f'.repeat(40);
const REWOUND = '1'.repeat(40);
const CANDIDATE = '2'.repeat(40);
const ACCEPTED = '4'.repeat(40);
const RECEIPT = '55555555-5555-4555-8555-555555555555';
const PROPOSAL_ID = '99999999-1111-4111-8111-111111111111';
const CONFIRMATION_ID = '66666666-6666-4666-8666-666666666666';
const REVIEW_ID = '77777777-7777-4777-8777-777777777777';

/** A person who may do everything M3 offers. */
const everything = {
  ...workspace,
  permissions: ['read', 'write', 'propose', 'approve', 'review'],
};

const commit = (revision: string, message: string, parents: string[], hour: number) => ({
  revision,
  parents,
  message,
  author: 'ada',
  committed_at: `2026-01-01T${String(hour).padStart(2, '0')}:00:00.000Z`,
});
const VERSIONS = [
  commit(HEAD, 'Publish the budget', [OLD], 12),
  commit(OLD, 'Draft the budget', [OLDER], 11),
  commit(OLDER, 'Add the meeting notes', [FIRST], 10),
  commit(FIRST, 'Start the workspace', [], 9),
];

const patch = [
  'diff --git a/notes/meeting.md b/notes/meeting.md',
  'index 1111111..2222222 100644',
  '--- a/notes/meeting.md',
  '+++ b/notes/meeting.md',
  '@@ -3,2 +3,2 @@',
  ' # Meeting notes',
  '-The budget is draft.',
  '+The budget is published.',
  '',
].join('\n');
const diffAnswer = (body: Record<string, unknown>) =>
  ok({
    from: body.from,
    to: body.to,
    changes: [
      {
        old_path: 'notes/meeting.md',
        new_path: 'notes/meeting.md',
        kind: 'modified',
        binary: false,
        patch,
      },
    ],
    warnings: [],
  });

/** Press a control from the keyboard: focus it, then Enter. */
async function press(page: Page, name: string | RegExp, within: Page | Locator = page) {
  const button = within.getByRole('button', { name, exact: true });
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  await button.focus();
  await page.keyboard.press('Enter');
}

test.describe('history and review', () => {
  const server = useExplorerServer();

  test('the Timeline, a version’s Changes and Rewind, from the keyboard', async ({ page }) => {
    let rewound = false;
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(everything),
      '/api/history/log-items': () => ok({ commits: VERSIONS, next_cursor: null }),
      '/api/history/diff-items': diffAnswer,
      '/api/history/restore-items': () => {
        rewound = true;
        return ok({ revision: REWOUND, receipt_id: RECEIPT, warnings: [] });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/timeline`);
    const timeline = page.getByRole('region', { name: 'Timeline' });
    await expect(timeline.getByText('Draft the budget')).toBeVisible();
    await expectNoAxeViolations(page, 'the Timeline');
    await shot(page, 'm3-01-timeline');

    const entry = timeline.getByRole('listitem').filter({ hasText: 'Draft the budget' });
    await press(page, 'Show its changes', entry);
    const changes = entry.getByRole('region', { name: 'Its changes' });
    await expect(changes.locator('ins')).toContainText('The budget is published.');
    await expect(changes).not.toContainText('diff --git');
    expect(await focused(page)).toBe('button:Hide its changes');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a version’s Changes');
    await shot(page, 'm3-02-changes');

    await press(page, 'Rewind to this version…', entry);
    const panel = entry.getByRole('region', { name: /^Rewind the workspace to revision/ });
    await expect(panel.getByRole('region', { name: 'What Rewind will change' })).toBeVisible();
    await expect(panel.getByRole('heading', { name: /^Rewind the workspace/ })).toBeFocused();
    await expect(panel).toContainText('No version is erased');
    await expectNoAxeViolations(page, 'Rewind, before it is done');
    await shot(page, 'm3-03-rewind');
    // Escape leaves it; focus goes back to what opened it.
    await page.keyboard.press('Escape');
    expect(await focused(page)).toBe('button:Rewind to this version…');
    await press(page, 'Rewind to this version…', entry);
    await expect(panel.getByRole('region', { name: 'What Rewind will change' })).toBeVisible();
    await press(page, 'Rewind', panel);
    await expect(panel.getByText(/^Rewound:/)).toBeVisible();
    expect(rewound).toBe(true);
    // The workspace shows the version the Rewind made; focus is on what was done.
    await expect(page.locator('.head-line code')).toHaveText(REWOUND.slice(0, 10));
    expect(await focused(page)).not.toBe('body');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'Rewind, done');
    await shot(page, 'm3-04-rewound');
  });

  test('a proposal: its changes, a comment, and Approve through the confirmation, from the keyboard', async ({
    page,
  }) => {
    const state = { status: 'open', comments: [] as Array<Record<string, unknown>> };
    const proposal = () => ({
      id: PROPOSAL_ID,
      workspace_id: WS,
      base_revision: HEAD,
      proposal_revision: CANDIDATE,
      content_digest: '3'.repeat(64),
      title: 'Publish the budget',
      description: 'The board approved it on Monday.',
      changes: [
        {
          kind: 'edit',
          item_id: NOTE,
          body: '# Meeting notes\n\nThe budget is published.\n',
          properties: { type: 'meeting-note' },
        },
      ],
      kind: 'content',
      status: state.status,
      created_by: 'claude-agent',
      created_via: 'mcp_delegation',
      created_at: '2026-01-01T09:30:00.000Z',
      warnings: [],
    });
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(everything),
      '/api/proposals/list-proposals': () => ok({ items: [proposal()], next_cursor: null }),
      '/api/proposals/get-proposal': () => ok(proposal()),
      '/api/proposals/list-comments': () => ok({ items: state.comments, next_cursor: null }),
      '/api/proposals/add-comment': (body) => {
        const added = {
          id: `c${state.comments.length + 1}`,
          author: 'ada',
          text: body.text,
          created_at: '2026-01-01T09:45:00.000Z',
        };
        state.comments.push(added);
        return ok(added);
      },
      '/api/history/diff-items': diffAnswer,
      '/api/reviews/create-confirmation': (body) =>
        ok({
          id: CONFIRMATION_ID,
          revision: body.revision,
          expires_at: '2026-01-01T10:25:00.000Z',
        }),
      '/api/proposals/accept-proposal': () => {
        state.status = 'accepted';
        return ok({ revision: ACCEPTED, receipt_id: RECEIPT, warnings: [] });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/proposals`);
    const list = page.getByRole('region', { name: 'Proposals' });
    const link = list.getByRole('link', { name: 'Publish the budget' });
    await expect(link).toBeVisible();
    await expectNoAxeViolations(page, 'the proposals');
    await shot(page, 'm3-05-proposals');
    await link.focus();
    await page.keyboard.press('Enter');
    const detail = page.getByRole('region', { name: 'Publish the budget' });
    await expect(detail.getByRole('region', { name: 'What it changes' })).toBeVisible();
    await expectNoAxeViolations(page, 'one proposal');
    await shot(page, 'm3-06-proposal');

    const box = detail.getByRole('textbox', { name: 'Your comment' });
    await box.focus();
    await page.keyboard.type('Looks right to me.');
    await press(page, 'Add the comment', detail);
    await expect(detail.getByRole('list', { name: 'Discussion' })).toContainText(
      'Looks right to me.',
    );

    await press(page, 'Approve…', detail);
    const dialog = page.getByRole('alertdialog', { name: 'Accept “Publish the budget”?' });
    await expect(dialog).toBeVisible();
    // Focus is in the dialog, on Cancel, the action that changes nothing.
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expectVisibleFocus(page);
    await expect(dialog.getByRole('region', { name: 'What it changes' })).toBeVisible();
    await expectNoAxeViolations(page, 'the confirmation of an acceptance');
    await shot(page, 'm3-07-approve-dialog');
    await page.keyboard.press('Shift+Tab');
    expect(await focused(page)).toBe('button:Accept this proposal');
    await page.keyboard.press('Enter');
    await expect(dialog).toHaveCount(0);
    await expect(detail.getByText('Accepted', { exact: true })).toBeVisible();
    await expect(page.locator('.head-line code')).toHaveText(ACCEPTED.slice(0, 10));
    await expect(detail.getByText(/^Accepted: its changes are in a new version/)).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'an accepted proposal');
    await shot(page, 'm3-08-accepted');
  });

  test('Verify an item through the confirmation, from the keyboard', async ({ page }) => {
    let reviewed = false;
    const review = {
      id: REVIEW_ID,
      source: {
        workspace_id: WS,
        item_id: NOTE,
        path: 'notes/meeting.md',
        revision: HEAD,
        selection: { kind: 'all' },
        locations: [],
      },
      content_digest: 'c'.repeat(64),
      reviewer_subject: 'grace',
      reviewed_at: '2026-01-01T10:15:00.000Z',
      coverage: 'current',
    };
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(everything),
      '/api/reviews/list-reviews': () =>
        ok({
          items: reviewed
            ? [review]
            : [{ ...review, coverage: 'changed', source: { ...review.source, revision: OLD } }],
          imported: [{ by: 'Dr. Smith', at: '2024-03-01' }],
        }),
      '/api/reviews/create-confirmation': (body) =>
        ok({
          id: CONFIRMATION_ID,
          revision: body.revision,
          expires_at: '2026-01-01T10:25:00.000Z',
        }),
      '/api/reviews/create-review': () => {
        reviewed = true;
        return ok(review);
      },
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    const reviews = page.getByRole('region', { name: 'Reviews' });
    await expect(reviews.getByText(/^Not verified at this revision\./)).toBeVisible();
    await expect(reviews).toContainText('imported with the file, not a review recorded here');
    await expectNoAxeViolations(page, 'reviews beside the item');
    await shot(page, 'm3-09-reviews');
    const verify = reviews.getByRole('button', { name: 'Verify…' });
    await press(page, 'Verify…', reviews);
    const dialog = page.getByRole('alertdialog', { name: /^Verify Meeting notes at revision/ });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
    await expect(dialog.getByRole('textbox', { name: 'The text you verify' })).toHaveValue(
      /Meeting notes/,
    );
    await expectNoAxeViolations(page, 'the confirmation of a review');
    await shot(page, 'm3-10-verify-dialog');
    await page.keyboard.press('Shift+Tab');
    expect(await focused(page)).toBe('button:Verify this revision');
    await page.keyboard.press('Enter');
    await expect(dialog).toHaveCount(0);
    await expect(reviews.getByText('Verified at this revision')).toBeVisible();
    await expect(verify).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a verified item');
    await shot(page, 'm3-11-verified');
  });

  test('an item’s Timeline and Who, Propose a change, and Decline, from the keyboard', async ({
    page,
  }) => {
    const proposed = {
      id: PROPOSAL_ID,
      workspace_id: WS,
      base_revision: HEAD,
      proposal_revision: CANDIDATE,
      content_digest: '3'.repeat(64),
      title: 'Publish the budget',
      description: '',
      changes: [{ kind: 'edit', item_id: NOTE, body: 'x', properties: {} }],
      kind: 'content',
      status: 'open',
      created_by: 'ada',
      created_via: 'browser_session',
      created_at: '2026-01-01T09:30:00.000Z',
      warnings: [],
    };
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(everything),
      '/api/history/log-items': () => ok({ commits: VERSIONS.slice(0, 3), next_cursor: null }),
      '/api/history/diff-items': diffAnswer,
      '/api/history/blame-item': (body) => {
        const lines = body.lines as { start: number; end: number };
        const answered = [];
        for (let line = lines.start; line <= lines.end; line += 1) {
          answered.push({
            line,
            text: `Line ${line}`,
            commit: line < 4 ? VERSIONS.at(1) : VERSIONS.at(0),
          });
        }
        return ok({
          lines: answered,
          source: {
            workspace_id: WS,
            item_id: NOTE,
            path: 'notes/meeting.md',
            revision: HEAD,
            selection: { kind: 'lines', range: lines },
            locations: [],
          },
        });
      },
      '/api/proposals/open-proposal': () => ok(proposed),
      '/api/proposals/get-proposal': () => ok(proposed),
      '/api/proposals/list-comments': () => ok({ items: [], next_cursor: null }),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByRole('heading', { level: 2, name: 'Meeting notes' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Show this item’s Timeline' })).toBeVisible();
    await press(page, 'Show this item’s Timeline');
    const timeline = page.getByRole('region', { name: 'Timeline of Meeting notes' });
    await expect(timeline.getByText('Draft the budget')).toBeVisible();
    expect(await focused(page)).toBe('button:Hide this item’s Timeline');
    await press(page, 'Show who changed each line');
    const who = page.getByRole('region', { name: 'Who changed each line' });
    await expect(who.getByRole('table')).toBeVisible();
    await expect(who).toContainText('the same version as the line above');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'an item’s Timeline and Who');
    await shot(page, 'm3-13-item-history');

    await press(page, 'Propose a change…');
    await expect(page.getByRole('textbox', { name: 'Title' })).toBeFocused();
    await page.keyboard.type('Publish the budget');
    await expectNoAxeViolations(page, 'proposing a change');
    await shot(page, 'm3-14-propose');
    await press(page, 'Propose');
    const made = page.getByText(/^Proposed: “Publish the budget”/);
    await expect(made).toBeFocused();
    await made.getByRole('link', { name: 'Open the proposal' }).focus();
    await page.keyboard.press('Enter');
    const detail = page.getByRole('region', { name: 'Publish the budget' });
    await expect(detail.getByRole('region', { name: 'What it changes' })).toBeVisible();
    await press(page, 'Decline…', detail);
    await expect(detail.getByRole('textbox', { name: 'Why it is declined' })).toBeFocused();
    await expectNoAxeViolations(page, 'declining a proposal');
    await shot(page, 'm3-15-decline');
    await page.keyboard.press('Escape');
    expect(await focused(page)).toBe('button:Decline…');
  });

  test('the graph, drawn, and its list, from the keyboard', async ({ page }) => {
    await routeApi(page, {
      '/api/search/get-graph': () =>
        ok({
          revision: HEAD,
          nodes: [note, plan, source],
          edges: [
            { from: NOTE, to: PLAN, to_path: 'notes/plan.md', label: 'the plan', line: 4 },
            { from: NOTE, to: SOURCE, to_path: 'annual-report.md', label: 'the report', line: 6 },
            { from: PLAN, to_path: 'notes/gone.md', label: 'old minutes', line: 2 },
          ],
          truncated: true,
        }),
    });
    await page.goto(`${server.origin()}/w/${WS}/graph`);
    const graph = page.getByRole('region', { name: 'Graph' });
    await expect(graph.getByRole('table', { name: 'Items in the graph' })).toBeVisible();
    // Drawn on a canvas, as one image whose name points to the list.
    await expect(graph.locator('.graph-drawing canvas').first()).toBeAttached();
    await expect(graph.getByRole('img')).toHaveAttribute(
      'aria-label',
      /the table and the list below/,
    );
    await expect(graph).toContainText('The graph is cut short');
    await expect(graph.getByRole('list', { name: 'Links in the graph' })).toContainText(
      'leads nowhere',
    );
    await expectNoAxeViolations(page, 'the graph');
    await shot(page, 'm3-12-graph');
    const plansRow = graph.getByRole('table').getByRole('link', { name: 'Plan' });
    await plansRow.focus();
    await expectVisibleFocus(page);
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(new RegExp(`/w/${WS}/i/${PLAN}\\?rev=${HEAD}`));
    await expect(page.getByRole('heading', { level: 2, name: 'Plan' })).toBeVisible();
  });
});
