/**
 * Where a journey finds a Source's original and its corrections in the reader (UI M4a). The
 * reader's builder (ruling A1) owns this file: when the reader's views change how the original is
 * reached, these helpers change with it, and every viewer's journey keeps calling them.
 *
 * The reader shows one view of an item at a time, chosen in its "Read as" radio group (Text,
 * Outline, Text and pictures, Pages, Original: only the views the item has). The original is the
 * Original view: a region named "Original" holding the viewer its media type goes to. A Source's
 * corrections stay beside whatever view is shown.
 */
import { expect, type Locator, type Page } from '@playwright/test';
import { tabTo } from './views';

export type ReaderViewName = 'Text' | 'Outline' | 'Text and pictures' | 'Pages' | 'Original';

export interface ChooseView {
  /** Choose with the keyboard alone: focus moves into the group, and the arrow keys choose. */
  keyboard?: boolean;
  /** The reader to choose in, when the page shows more than one (two panes). */
  within?: Locator;
}

/** The "Read as" choice of the reader. */
export function readAs(page: Page, options: ChooseView = {}): Locator {
  return (options.within ?? page).getByRole('radiogroup', { name: 'Read as' });
}

/**
 * Show the reader's view `name`. With the keyboard, focus goes to the chosen radio (where Tab
 * puts it: a radio group is one Tab stop) and the arrow keys move the choice. Native WebKit stops
 * at the group's ends, so travel toward the target in DOM order rather than assuming wrapping.
 * Each intermediate choice must be checked and focused before another key is sent.
 */
export async function chooseView(page: Page, name: ReaderViewName, options: ChooseView = {}) {
  const group = readAs(page, options);
  const choice = group.getByRole('radio', { name, exact: true });
  await expect(choice).toBeVisible();
  if (!options.keyboard) {
    await choice.check();
    return;
  }
  const count = await group.getByRole('radio').count();
  await tabTo(page, group.locator('input:checked'));
  for (let presses = 0; presses < count; presses += 1) {
    const positions = await group.getByRole('radio').evaluateAll(
      (elements, wanted) => ({
        current: elements.findIndex((element) => (element as HTMLInputElement).checked),
        target: elements.findIndex((element) => (element as HTMLInputElement).value === wanted),
      }),
      await choice.inputValue(),
    );
    expect(positions.current).toBeGreaterThanOrEqual(0);
    expect(positions.target).toBeGreaterThanOrEqual(0);
    if (positions.current === positions.target) break;
    const step = positions.target < positions.current ? -1 : 1;
    const next = group.getByRole('radio').nth(positions.current + step);
    const key = step < 0 ? 'ArrowLeft' : 'ArrowRight';
    await page.keyboard.press(key);
    await expect(next).toBeChecked();
    await expect(next).toBeFocused();
  }
  await expect(choice).toBeChecked();
  await expect(choice).toBeFocused();
}

/** The original of the item the reader shows, in its viewer: its Original view, chosen. */
export async function openOriginal(page: Page, options: ChooseView = {}): Promise<Locator> {
  await chooseView(page, 'Original', options);
  return (options.within ?? page).getByRole('region', { name: 'Original' });
}
