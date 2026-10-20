/** A2: CodeMirror original bytes remain bounded and read-only through keyboard continuation. */
import { expect, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  shot,
  useExplorerServer,
} from './support/explorer';
import { activate, sourceFixture, sourceUrl } from './support/m4a-sheets';
import { openOriginal } from './support/reader';
import { tabTo } from './support/views';

test.describe('A2 text and code', () => {
  const server = useExplorerServer();
  test('reads JSON code one byte part at a time without making it editable', async ({ page }) => {
    // The literal fixture is 70,013 UTF-8 bytes; continuation starts inside its string value.
    const originalText = `{"value":"${'a'.repeat(70_000)}"}\n`;
    const fixture = await sourceFixture(page, {
      path: 'data.json',
      mediaType: 'application/json',
      original: originalText,
      markdown: 'Converted JSON data.\n',
    });
    await page.goto(sourceUrl(server.origin()));
    const original = await openOriginal(page, { keyboard: true });
    const box = original.getByRole('textbox', { name: 'Original text of data.json' });
    await expect(box).toHaveAttribute('aria-readonly', 'true');
    await expect(box).toHaveAttribute('data-language', 'json');
    await expect(original).toContainText('Bytes 1–65,536 of 70,013 of the original are shown');
    expect(fixture.ranges).toEqual(['bytes=0-65535']);
    await tabTo(page, box);
    await page.keyboard.press('ArrowRight');
    await expectVisibleFocus(page);
    // The real Tab target must also scroll the long code with its caret, not only receive focus.
    await page.keyboard.press('Control+End');
    await expect
      .poll(() => original.locator('.cm-scroller').evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
    await page.keyboard.type('X');
    await expect(box).not.toContainText('X');
    await expectNoAxeViolations(page, 'JSON first bounded byte part');
    await activate(
      page,
      original.getByRole('button', { name: 'Show more of the original', exact: true }),
    );
    await expect(original).toContainText('All 70,013 bytes of the original are shown.');
    await expect(box).toBeFocused();
    await expectVisibleFocus(page);
    expect(fixture.ranges).toEqual(['bytes=0-65535', 'bytes=65536-70012']);
    await expectNoAxeViolations(page, 'JSON after keyboard continuation');
    await shot(page, 'm4a-code-original');
  });
});
