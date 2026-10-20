/** A2: the original and digest are actual MergeView panes with keyboard difference navigation. */
import { expect, test } from '@playwright/test';
import {
  expectNoAxeViolations,
  expectVisibleFocus,
  shot,
  useExplorerServer,
} from './support/explorer';
import { activate, sourceFixture, sourceUrl } from './support/m4a-sheets';
import { openOriginal } from './support/reader';

test.describe('A2 original beside digest', () => {
  const server = useExplorerServer();
  test('names read-only panes, counts differences and navigates each from the keyboard', async ({
    page,
  }) => {
    const fixture = await sourceFixture(page, {
      path: 'agenda.txt',
      mediaType: 'text/plain',
      original: 'Agenda\nBudget: 1.2 M\nDone.\n',
      markdown: '# Agenda\n\nBudget: 1.2 M\n\nDone.\n',
    });
    await page.goto(sourceUrl(server.origin()));
    const original = await openOriginal(page, { keyboard: true });
    await activate(
      page,
      original.getByRole('button', { name: 'Compare the original with its converted text' }),
    );
    const comparison = original.getByRole('region', {
      name: 'The original beside its converted text',
    });
    const left = comparison.getByRole('textbox', { name: 'Original: agenda.txt' });
    const right = comparison.getByRole('textbox', {
      name: 'Converted text: what the reader shows',
    });
    await expect(left).toHaveAttribute('aria-readonly', 'true');
    await expect(right).toHaveAttribute('aria-readonly', 'true');
    await expect(comparison).toContainText('2 places differ.');
    await activate(page, comparison.getByRole('button', { name: 'Next difference' }));
    await expect(comparison).toContainText('Difference 1 of 2:');
    await expect(left).toBeFocused();
    await expectVisibleFocus(page);
    await activate(page, comparison.getByRole('button', { name: 'Next difference' }));
    await expect(comparison).toContainText('Difference 2 of 2:');
    await expect(left).toBeFocused();
    await activate(page, comparison.getByRole('button', { name: 'Previous difference' }));
    await expect(comparison).toContainText('Difference 1 of 2:');
    await expect(left).toBeFocused();
    await expectVisibleFocus(page);
    expect(fixture.ranges).toEqual(['bytes=0-26']);
    await expectNoAxeViolations(page, 'original beside digest');
    await shot(page, 'm4a-original-beside-digest');
  });
});
