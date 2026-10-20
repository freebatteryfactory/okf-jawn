/** A2 fixtures: real viewer routes, bounded original bytes, and keyboard activation. */
import { expect, type Locator, type Page } from '@playwright/test';
import type { Selection } from '../../../src/api/generated/types.gen';
import { HEAD, ok, readAnswer, routeApi, SOURCE, summary, WS } from './explorer';
import { tabTo } from './views';

export const OBJECT = 'b'.repeat(64);
export const DIGEST = 'c'.repeat(64);

/** The fixture's expected content is authored by each test, never by the viewer. */
export async function sourceFixture(
  page: Page,
  options: {
    path: string;
    mediaType: string;
    original: string;
    markdown: string;
    secondPart?: string;
    sheets?: string[];
  },
) {
  const bytes = new TextEncoder().encode(options.original);
  const item = {
    ...summary(SOURCE, options.path, options.path, 'document', 'source'),
    media_type: options.mediaType,
    extraction: { status: 'completed', text_origin: 'converter', corrected: false },
  };
  const reads: Record<string, unknown>[] = [];
  const ranges: Array<string | undefined> = [];
  await routeApi(page, {
    '/api/items/get-item': () =>
      ok({
        summary: item,
        properties: { type: 'document', okf_jawn: { id: SOURCE } },
        source: {
          object: OBJECT,
          size: String(bytes.length),
          media_type: options.mediaType,
          names: [],
          metadata: {},
          extraction: {
            corrected: false,
            digest: DIGEST,
            outcome: { status: 'completed' },
            text_origin: 'converter',
            warnings: options.sheets ? [{ kind: 'cell_values_only', sheets: options.sheets }] : [],
          },
        },
      }),
    '/api/reads/read-item': (body) => {
      reads.push(body);
      const more = body.cursor === 'second-part';
      const textView = body.view === 'text' || body.view === 'multimodal';
      return ok(
        readAnswer(item, body, {
          view: body.view,
          markdown: textView ? (more ? options.secondPart : options.markdown) : '',
          extraction: item.extraction,
          truncated: textView && !more && options.secondPart !== undefined,
          next_cursor: textView && !more && options.secondPart !== undefined ? 'second-part' : null,
          source: {
            workspace_id: WS,
            item_id: SOURCE,
            path: options.path,
            revision: HEAD,
            digest: body.view === 'original' ? OBJECT : DIGEST,
            selection: body.selection,
            locations: [],
          },
        }),
      );
    },
    '/api/reads/get-sources': () => ok({ revision: HEAD, sources: [], declared: [] }),
  });
  await page.route(
    `**/api/workspaces/${WS}/items/${SOURCE}/revisions/${HEAD}/objects/${OBJECT}`,
    async (route) => {
      const range = route.request().headers().range;
      ranges.push(range);
      const match = /^bytes=(\d+)-(\d+)$/.exec(range ?? '');
      if (match === null) throw new Error('The original must be read by byte range');
      const first = Number(match[1]);
      const last = Math.min(Number(match[2]), bytes.length - 1);
      await route.fulfill({
        status: 206,
        contentType: options.mediaType,
        headers: { 'Content-Range': `bytes ${first}-${last}/${bytes.length}` },
        body: Buffer.from(bytes.slice(first, last + 1)),
      });
    },
  );
  return { reads, ranges };
}

/** Reach controls through the real Tab order, then activate with Enter. */
export async function activate(page: Page, control: Locator) {
  await tabTo(page, control);
  await expect(control).toBeFocused();
  await page.keyboard.press('Enter');
}

export function sourceUrl(origin: string, selection: Selection = { kind: 'all' }) {
  const search = new URLSearchParams({ sel: JSON.stringify(selection) });
  return `${origin}/w/${WS}/i/${SOURCE}?${search}`;
}
