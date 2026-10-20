/**
 * A2 / SPEC sections 5 and 7: an Office conversion remains readable and its exact original can
 * be saved; an unsupported original is retained and downloadable without claiming a conversion.
 * The Office original is a valid small RTF document; its converter answer is explicitly mocked,
 * so these journeys prove browser access, not native conversion. The other original is a binary
 * fixture with no viewer. Expected download bytes are those authored below, never viewer output.
 *
 * Both engines run pointer and keyboard journeys, with zero axe violations and visible focus.
 * WebKit download links use the existing E8 direct-focus exception; Enter still follows them.
 * The built UI and downloads carry the actual server policies through useOriginalServer.
 */
import { readFile } from 'node:fs/promises';
import { expect, type Page, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  ok,
  readAnswer,
  shot,
  workspace,
} from './support/explorer';
import {
  OBJECT_PATH,
  routeSource,
  sourceAddress,
  sourceRow,
  useOriginalServer,
  watchPolicy,
} from './support/m4a-pdf';
import { openOriginal } from './support/reader';
import { tabTo } from './support/views';

const RTF = Buffer.from(
  '{\\rtf1\\ansi Annual report 2025\\par Revenue grew by 12 percent.}',
  'ascii',
);
const OTHER = Buffer.from([0x4f, 0x4b, 0x46, 0x00, 0xfe, 0x81, 0x00, 0x01]);

async function savedOriginal(
  page: Page,
  name: string,
  bytes: Buffer,
  activate: () => Promise<void>,
) {
  const downloading = page.waitForEvent('download');
  await activate();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe(name);
  expect(await readFile(await download.path())).toEqual(bytes);
}

for (const original of [
  {
    kind: 'Office conversion',
    filename: 'annual-report.rtf',
    mediaType: 'application/rtf',
    bytes: RTF,
    converted: true,
    words: 'This Rich Text document is shown as its conversion',
    screenshot: 'office',
  },
  {
    kind: 'other original',
    filename: 'retained.bin',
    mediaType: 'application/octet-stream',
    bytes: OTHER,
    converted: false,
    words: 'There is no viewer here for files of this type (application/octet-stream).',
    screenshot: 'other',
  },
]) {
  test.describe(original.kind, () => {
    const server = useOriginalServer({
      [OBJECT_PATH]: { body: original.bytes, contentType: 'application/octet-stream' },
    });

    for (const keyboard of [false, true]) {
      test(`${keyboard ? 'from the keyboard' : 'with the pointer'}: read its state and save its exact bytes`, async ({
        page,
        browserName,
      }) => {
        const policy = await watchPolicy(page);
        const reads = await routeSource(page, {
          mediaType: original.mediaType,
          bytes: original.bytes,
          filename: original.filename,
          ...(original.converted
            ? {}
            : { extraction: { outcome: { status: 'unsupported' }, text_origin: 'none' } }),
          extra: {
            // Reads, including the original, remain available in an archived workspace.
            '/api/workspaces/open-workspace': () =>
              ok({ ...workspace, archived_at: '2026-01-02T08:00:00.000Z' }),
            ...(!original.converted
              ? {
                  '/api/reads/read-item': (body: Record<string, unknown>) =>
                    ok(
                      readAnswer(sourceRow(original.mediaType), body, {
                        view: body.view,
                        markdown: '',
                        outline: [],
                      }),
                    ),
                }
              : {}),
          },
        });
        await page.goto(sourceAddress(server.origin()));
        if (original.converted) {
          await expect(
            page.getByText('Revenue grew by 12 percent.', { exact: true }),
          ).toBeVisible();
          await expectNoAxeViolations(page, 'the Office conversion in the reader');
        }
        const region = await openOriginal(page);
        await expect(region).toContainText(original.words);
        const link = region.getByRole('link', {
          name: `Download the original, ${original.filename}`,
        });
        await expect(link).toHaveAttribute('href', OBJECT_PATH);
        await expect(link).toHaveAttribute('download', original.filename);
        if (keyboard) {
          if (browserName === 'webkit') {
            // E8: Safari's default keyboard setting excludes links from Tab order.
            await link.focus();
          } else {
            await tabTo(page, link);
          }
          await expect(link).toBeFocused();
          await expectVisibleFocus(page);
        }
        await expectNoAxeViolations(page, `${original.kind}, original access`);
        await shot(page, `m4a-${original.screenshot}-${keyboard ? 'keyboard' : 'pointer'}`);
        await savedOriginal(page, original.filename, original.bytes, () =>
          keyboard ? page.keyboard.press('Enter') : link.click(),
        );
        expect(server.count(OBJECT_PATH)).toBe(1);
        // Saving uses the browser's attachment stream, not a whole Blob cached by the page.
        expect(reads).toEqual([]);
        expect(await policy.refusals()).toEqual([]);
      });
    }
  });
}
