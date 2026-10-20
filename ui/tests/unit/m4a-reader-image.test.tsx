/** A2 images: retained object URL lifetime and region coordinates from the wire, not renderer output. */
import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useObjectUrl } from '../../src/features/viewers/image/bytes';
import { regionBox } from '../../src/features/viewers/image/ImageFrame';
import { mediaAlt } from '../../src/features/viewers/image/MediaImage';
import { PAGE_ONE, pageImage, picture } from './support/m4a-reader';

afterEach(() => vi.restoreAllMocks());

it('releases every object URL when bytes change and when the image closes', async () => {
  const create = vi
    .spyOn(URL, 'createObjectURL')
    .mockReturnValueOnce('blob:first')
    .mockReturnValueOnce('blob:second');
  const revoke = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
  const first = new Blob(['first'], { type: 'application/octet-stream' });
  const second = new Blob(['second'], { type: 'image/png' });
  const hook = renderHook(({ blob }) => useObjectUrl(blob, 'image/png'), {
    initialProps: { blob: first },
  });
  await waitFor(() => expect(hook.result.current).toBe('blob:first'));
  expect(create.mock.calls[0]?.[0]).toMatchObject({ type: 'image/png' });
  hook.rerender({ blob: second });
  await waitFor(() => expect(hook.result.current).toBe('blob:second'));
  expect(revoke.mock.calls).toEqual([['blob:first']]);
  hook.unmount();
  expect(revoke.mock.calls).toEqual([['blob:first'], ['blob:second']]);
});

it('shows a selected region in either recorded coordinate origin', () => {
  const page = { page_no: 1, page_size: { width: 200, height: 400 } };
  expect(
    regionBox({ ...page, bbox: { l: 20, r: 100, t: 40, b: 120, coord_origin: 'top_left' } }),
  ).toEqual({ left: 0.1, top: 0.1, width: 0.4, height: 0.2 });
  expect(
    regionBox({ ...page, bbox: { l: 20, r: 100, t: 360, b: 280, coord_origin: 'bottom_left' } }),
  ).toEqual({ left: 0.1, top: 0.1, width: 0.4, height: 0.2 });
});

it('uses a retained caption and names an uncaptioned picture/page explicitly', () => {
  expect(mediaAlt(picture)).toBe('Revenue by quarter, 2025');
  expect(mediaAlt({ ...picture, caption: null })).toBe(
    'A picture inferred to be on page 1, not certain; the document gives it no caption.',
  );
  expect(mediaAlt(pageImage(1, PAGE_ONE))).toBe(
    'Page 1 as the converter rendered it; the document gives it no caption.',
  );
});
