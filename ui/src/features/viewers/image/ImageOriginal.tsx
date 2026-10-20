/**
 * The image viewer of an item's original (UI M4a, ruling A2): the source's own bytes, read with
 * `download_object` (`app/byte-read.ts`) into the browser cache, shown through an object URL that
 * is revoked when the viewer lets it go (`bytes.ts`).
 *
 * The image is fitted to the pane, and "Show at actual size" shows it at its own size, scrolled
 * within the viewer; the button says which is shown (`aria-pressed`). The words under it name the
 * file, its type, its size in bytes and, once the bytes are shown, in pixels. An image is one
 * page: a selection of a region on it is outlined over it, and said to be. The original carries
 * no caption of its own, and its alt text says so.
 */
import { useState } from 'react';
import type { OriginalProps } from '../types';
import { ImageFrame } from './ImageFrame';

/** A decimal byte count as the service gives it, grouped for reading. */
function bytesWords(size: string): string {
  return /^\d+$/.test(size) ? BigInt(size).toLocaleString() : size;
}

export function ImageOriginal({
  workspaceId,
  itemId,
  revision,
  path,
  appearance,
  selection,
}: OriginalProps) {
  const [actual, setActual] = useState(false);
  const [natural, setNatural] = useState<{ width: number; height: number } | null>(null);
  const name = appearance.names[0]?.filename ?? path;
  const region =
    selection.kind === 'region' && selection.region.page_no === 1 ? selection.region : null;
  return (
    <figure className="image-original m-0 grid gap-2">
      <p className="m-0">
        <button type="button" aria-pressed={actual} onClick={() => setActual(!actual)}>
          Show at actual size
        </button>
      </p>
      <ImageFrame
        address={{ workspaceId, itemId, revision, digest: appearance.object }}
        mediaType={appearance.media_type}
        alt={`${name}, the original image. It carries no caption of its own.`}
        region={region}
        fit={!actual}
        context="Reading the original image"
        announce
        onLoaded={setNatural}
      />
      <figcaption className="meta">
        The original file {name}: {appearance.media_type}, {bytesWords(appearance.size)} bytes
        {natural === null ? '' : `, ${natural.width} × ${natural.height} pixels`}.
        {actual ? ' Shown at its actual size.' : ' Fitted to the pane.'}
        {region === null ? '' : ' The selected region is outlined.'}
      </figcaption>
    </figure>
  );
}
