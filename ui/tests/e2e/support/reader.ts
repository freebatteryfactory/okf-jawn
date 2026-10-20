/**
 * Where a journey finds a Source's original and its corrections in the reader (UI M4a). The
 * reader's builder (ruling A1) owns this file: when the reader's views change how the original is
 * reached, these helpers change with it, and every viewer's journey keeps calling them.
 */
import type { Locator, Page } from '@playwright/test';

/** The original of the item the reader shows, in its viewer. */
export async function openOriginal(page: Page): Promise<Locator> {
  return page.getByRole('region', { name: 'Original' });
}
