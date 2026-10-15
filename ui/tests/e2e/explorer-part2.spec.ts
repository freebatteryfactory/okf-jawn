/**
 * The screens contract batch 2 brought to the workspace, from the keyboard, in Chromium and
 * WebKit: a rules file that cannot be read, in the YAML view with its problems, repaired and
 * saved; the uploads the service holds, and the question before one is discarded; a new note's
 * commit with its warning and receipt; jobs with their times and a failed one that cannot be tried
 * again; Attention rows with their place, and the uploads waiting. Each screen has zero axe
 * violations, and a screenshot for the owner under `.artifacts/ui/` (untracked).
 */

import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { expect, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  focused,
  HEAD,
  NOTE,
  note,
  ok,
  routeApi,
  shot,
  summary,
  useExplorerServer,
  WS,
  writableWorkspace,
} from './support/explorer';

const RECEIPT = '55555555-5555-4555-8555-555555555555';
const NEW = '66666666-6666-4666-8666-666666666666';
const JOB = '88888888-8888-4888-8888-888888888888';
const schemaFile = fileURLToPath(
  new URL('../../../api/forms/naming-rules.schema.json', import.meta.url),
);

/** Press a control from the keyboard: focus it, then Enter. */
async function press(page: Page, name: string | RegExp) {
  const button = page.getByRole('button', { name, exact: true });
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  await button.focus();
  await page.keyboard.press('Enter');
}

const brokenText = [
  'schema_version: 2',
  'rules:',
  '  - glob: notes/**x',
  '    case: kebab',
  '    separator: "-"',
  '    prefix: ""',
  '    date_order: ymd',
  '    date_format: ""',
  '    strip_version_suffix: true',
  '    extension_policy: preserve',
  '    alias_preservation: true',
  '    collision: reject',
  '',
].join('\n');

const upload = (number: number, filename: string, extra: Record<string, unknown> = {}) => ({
  id: `66666666-6666-4666-8666-${String(number).padStart(12, '0')}`,
  workspace_id: WS,
  upload_path: `/api/workspaces/${WS}/uploads/${number}/content`,
  received_bytes: '2048',
  complete: true,
  filename,
  relative_path: 'papers',
  size: '2048',
  created_at: '2026-01-02T08:15:00.000Z',
  ...extra,
});

test.describe('the screens of contract batch 2', () => {
  const server = useExplorerServer();

  test('a rules file that cannot be read, repaired in the YAML view', async ({ page }) => {
    const schema: unknown = JSON.parse(await readFile(schemaFile, 'utf8'));
    // The revision the repair's save makes, and what it then holds: rules that can be read.
    const repairedAt = 'c'.repeat(40);
    const repairedText = brokenText
      .replace('schema_version: 2', 'schema_version: 1')
      .replace('notes/**x', 'notes/**');
    const saved: Array<Record<string, unknown>> = [];
    let head = HEAD;
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok({ ...writableWorkspace, head }),
      '/api/schemas/naming-rules': () => ok(schema),
      '/api/conventions/set-rules': (body) => {
        saved.push(body);
        head = repairedAt;
        return ok({ revision: repairedAt, receipt_id: RECEIPT, warnings: [] });
      },
      '/api/conventions/get-rules': (body) => {
        const at = body.at as { revision?: string } | undefined;
        if (at?.revision === repairedAt) {
          return ok({
            revision: repairedAt,
            rules: {
              schema_version: 1,
              rules: [
                {
                  glob: 'notes/**',
                  case: 'kebab',
                  separator: '-',
                  prefix: '',
                  date_order: 'ymd',
                  date_format: '',
                  strip_version_suffix: true,
                  extension_policy: 'preserve',
                  alias_preservation: true,
                  collision: 'reject',
                },
              ],
            },
          });
        }
        return ok({
          revision: HEAD,
          unreadable: {
            text: brokenText,
            problems: [
              {
                code: 'rules_schema_version_unknown',
                message: 'This build reads naming rules of format version 1.',
                location: '/schema_version',
              },
              {
                code: 'rules_glob_invalid',
                message: 'The pattern notes/**x is not a valid glob.',
                location: '/rules/0/glob',
              },
            ],
          },
        });
      },
    });
    await page.goto(`${server.origin()}/w/${WS}`);
    await press(page, 'Naming rules');
    await expect(page.getByRole('heading', { name: 'Naming rules' })).toBeFocused();
    await expect(page.getByText('The saved rules file cannot be read.')).toBeVisible();
    await expect(page.getByRole('tab', { name: 'Form' })).toHaveCount(0);
    const goTo = page.getByRole('button', { name: 'Go to line 3, column 11' });
    await expect(goTo).toBeVisible();
    await expectNoAxeViolations(page, 'a rules file that cannot be read');
    await shot(page, '40-rules-unreadable');
    await goTo.focus();
    await page.keyboard.press('Enter');
    expect(await focused(page)).toMatch(/^textbox:/);
    await expectVisibleFocus(page);

    // The repair, as a paste over the whole text, then saved from the keyboard.
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.insertText(repairedText);
    await expect(page.getByText('Your text differs from the saved file.')).toBeVisible();
    await press(page, 'Save the rules');
    await expect(page.locator('.rules-said')).toContainText('The rules are saved.');
    expect(saved).toMatchObject([
      { base_revision: HEAD, rules: { schema_version: 1, rules: [{ glob: 'notes/**' }] } },
    ]);
    // Read again at the revision the save made: rules, so the form is offered, and the notice that
    // the file cannot be read is gone.
    await expect(page.getByRole('tab', { name: 'Form' })).toBeVisible();
    await expect(page.getByText('The saved rules file cannot be read.')).toHaveCount(0);
    expect(await focused(page)).not.toBe('body');
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'a rules file repaired and saved');
    await shot(page, '40b-rules-repaired');
  });

  test('the uploads the service holds, and the question before one is discarded', async ({
    page,
  }) => {
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      '/api/imports/list-uploads': (body) =>
        ok({
          items:
            body.state === 'waiting'
              ? [upload(1, 'report.pdf'), upload(2, 'minutes.docx')]
              : [upload(3, 'half.csv', { complete: false, received_bytes: '512' })],
          next_cursor: null,
        }),
    });
    await page.goto(`${server.origin()}/w/${WS}/import`);
    const held = page.getByRole('region', { name: 'Uploads the service holds' });
    await expect(
      held.getByRole('checkbox', { name: 'Include “papers/report.pdf” in the import' }),
    ).toBeVisible();
    await expectNoAxeViolations(page, 'the uploads the service holds');
    await shot(page, '41-uploads-held');
    await press(page, 'Discard “papers/half.csv”');
    await expect(page.getByRole('button', { name: 'Keep the upload' })).toBeFocused();
    await expectVisibleFocus(page);
    await expectNoAxeViolations(page, 'the question before an upload is discarded');
    await shot(page, '42-upload-discard-question');
  });

  test('a new note’s warning and receipt', async ({ page }) => {
    const made = 'c'.repeat(40);
    const decisions = {
      ...summary(NEW, 'notes/Decisions.md', 'Decisions', 'note'),
      revision: made,
    };
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      '/api/items/get-item': (body) =>
        ok({
          summary: body.item_id === NEW ? decisions : note,
          body: body.item_id === NEW ? '# Decisions\n' : '# Meeting notes',
          content_digest: 'c'.repeat(64),
          properties: { type: 'note', okf_jawn: { id: String(body.item_id) } },
        }),
      '/api/items/create-item': (body) => {
        const item = {
          ...summary(NEW, String(body.path), String(body.title), 'note'),
          revision: made,
        };
        return ok({
          item: {
            summary: item,
            body: String(body.body),
            content_digest: 'c'.repeat(64),
            properties: { type: 'note', okf_jawn: { id: NEW } },
          },
          receipt_id: RECEIPT,
          warnings: [
            {
              code: 'okf_validator_warning',
              message: 'The note has no description.',
              location: String(body.path),
            },
          ],
        });
      },
      '/api/events/get-receipt': () =>
        ok({
          id: RECEIPT,
          workspace_id: WS,
          operation_id: 'create_item',
          principal_subject: 'ada',
          route: 'browser_session',
          audience: 'human_display',
          returned_at: '2026-01-02T08:15:00.000Z',
          sources: [],
        }),
    });
    await page.goto(`${server.origin()}/w/${WS}/i/${NOTE}`);
    await expect(page.getByText('Revenue grew by 12% in 2025.')).toBeVisible();
    await press(page, 'New note');
    await page.keyboard.type('Decisions');
    await page.keyboard.press('Enter');
    await expect(page.getByText('The note has no description.')).toBeVisible();
    await press(page, 'Show the receipt');
    await expect(page.getByText('create_item', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Hide the receipt' })).toBeFocused();
    await expectNoAxeViolations(page, 'a new note’s warning and receipt');
    await shot(page, '43-new-note-receipt');
  });

  test('jobs with their times, and a failed job that cannot be tried again', async ({ page }) => {
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      '/api/imports/list-jobs': () =>
        ok({
          items: [
            {
              id: JOB,
              kind: 'import',
              state: 'failed',
              attempt: 2,
              progress: 40,
              item_ids: [],
              warnings: [],
              workspace_id: WS,
              retryable: false,
              created_at: '2026-01-02T08:15:00.000Z',
              started_at: '2026-01-02T08:16:00.000Z',
              finished_at: '2026-01-02T08:20:00.000Z',
              error: {
                code: 'invalid_input',
                message: 'a card path was taken since the base',
                field: '/upload_ids/0',
              },
            },
          ],
          next_cursor: null,
        }),
    });
    await page.goto(`${server.origin()}/w/${WS}/jobs`);
    const row = page.locator(`li[data-job="${JOB}"]`);
    await expect(row).toContainText('The service will not take this job again.');
    await expect(row).toContainText(/Accepted .+; Started .+; Ended .+\./);
    await expect(row.getByRole('button', { name: 'Try the job again' })).toHaveCount(0);
    await expectNoAxeViolations(page, 'a job with its times');
    await shot(page, '44-job-times');
  });

  test('Attention rows with their place, and the uploads waiting', async ({ page }) => {
    await routeApi(page, {
      '/api/workspaces/open-workspace': () => ok(writableWorkspace),
      '/api/attention/get-attention': () =>
        ok({
          revision: HEAD,
          next_cursor: null,
          items: [
            {
              kind: 'instruction_like',
              message: 'Line 3 reads “ignore previous instructions”.',
              action: 'read_item',
              item_id: NOTE,
              item: note,
              selection: { kind: 'lines', range: { start: 3, end: 3 } },
            },
            {
              kind: 'uploads_waiting',
              message: '2 uploads are complete and not imported (4 KB).',
              action: 'list_uploads',
            },
          ],
        }),
    });
    await page.goto(`${server.origin()}/w/${WS}/attention`);
    await expect(page.getByText(', at line 3')).toBeVisible();
    const waiting = page.getByRole('link', { name: 'See the uploads waiting to be imported' });
    await expect(waiting).toBeVisible();
    await expectNoAxeViolations(page, 'Attention with a place and the uploads waiting');
    await shot(page, '45-attention-part2');
    await waiting.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: 'Import files' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Uploads the service holds' })).toBeVisible();
  });
});
