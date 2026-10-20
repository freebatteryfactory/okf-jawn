/**
 * One image on screen (ruling A2): its bytes read through `download_object` into the browser
 * cache (`bytes.ts`), shown through an object URL that is revoked when the image lets it go, with
 * its alt text, and, when the reader's selection is a region of the image's page, that region
 * outlined over it.
 *
 * While the bytes are read the frame says so; a read that fails is shown in their place, through
 * its retry (`useRetry`), which ends with keyboard focus on what it loaded: the words that the
 * image is being shown, and then the image (the object URL is made once the bytes are there, so
 * the image follows those words by one drawing, and focus follows it). The image keeps
 * its recorded size as its aspect, so the page does not move under the reader as images arrive,
 * and is fitted to the pane unless the caller shows it at its own size.
 */
import { useQuery } from '@tanstack/react-query';
import { useLayoutEffect, useRef } from 'react';
import type { PageRegion } from '../../../api/generated/types.gen';
import { takeHeld, useWatchedPart } from '../../../lib/focus';
import { FailureNotice } from '../../errors/FailureNotice';
import { useRetry } from '../../errors/retry';
import { type ObjectAddress, objectBytesOptions, useObjectUrl } from './bytes';

export interface ImageFrameProps {
  address: ObjectAddress;
  /** The media type the service recorded for the bytes. */
  mediaType: string;
  /** What the image is, for a person who cannot see it. */
  alt: string;
  /** The recorded size in pixels, when it is known before the bytes are. */
  size?: { width: number; height: number };
  /** A region of the image's page to outline, in the page's own extent. */
  region?: PageRegion | null;
  /** Fitted to the pane (the default), or at its own size. */
  fit?: boolean;
  /** What is being done, for a failure's words: "Reading the original image". */
  context: string;
  /** Say the reading of the bytes in a polite status (one image on screen, not a gallery). */
  announce?: boolean;
  /** The image's own size once its bytes are shown. */
  onLoaded?: (natural: { width: number; height: number }) => void;
}

/** The region's box as fractions of its page, measured from the top-left corner. */
export function regionBox(region: PageRegion): {
  left: number;
  top: number;
  width: number;
  height: number;
} | null {
  const { bbox, page_size: page } = region;
  if (!(page.width > 0 && page.height > 0)) return null;
  const [top, bottom] =
    bbox.coord_origin === 'top_left'
      ? [bbox.t, bbox.b]
      : [page.height - bbox.t, page.height - bbox.b];
  const left = Math.min(bbox.l, bbox.r);
  const right = Math.max(bbox.l, bbox.r);
  const upper = Math.min(top, bottom);
  const lower = Math.max(top, bottom);
  const clamp = (value: number) => Math.min(Math.max(value, 0), 1);
  return {
    left: clamp(left / page.width),
    top: clamp(upper / page.height),
    width: clamp((right - left) / page.width),
    height: clamp((lower - upper) / page.height),
  };
}

export function ImageFrame({
  address,
  mediaType,
  alt,
  size,
  region,
  fit = true,
  context,
  announce = false,
  onLoaded,
}: ImageFrameProps) {
  const bytes = useQuery(objectBytesOptions(address));
  const url = useObjectUrl(bytes.data, mediaType);
  const imageRef = useRef<HTMLImageElement | null>(null);
  // The words shown until the image is: a retry's focus waits there, and goes on to the image.
  const heldPending = useRef<HTMLElement | null>(null);
  const [pending, pendingRef] = useWatchedPart<HTMLParagraphElement>(heldPending);
  useLayoutEffect(() => {
    if (takeHeld(heldPending)) imageRef.current?.focus();
  });
  const failure = useRetry({
    request: JSON.stringify(address),
    failed: bytes.isError,
    error: bytes.error,
    run: () => bytes.refetch(),
    loaded: () => imageRef.current ?? pending.current,
  });
  const box = region ? regionBox(region) : null;

  if (failure.shown) {
    return (
      <FailureNotice
        context={context}
        error={failure.error}
        retry={failure}
        workspaceId={address.workspaceId}
      />
    );
  }
  if (url === null) {
    return (
      <p
        ref={pendingRef}
        tabIndex={-1}
        className="image-pending grid place-items-center border border-dashed p-4"
        style={size ? { aspectRatio: `${size.width} / ${size.height}` } : undefined}
        {...(announce ? { role: 'status' } : {})}
      >
        Reading the image…
      </p>
    );
  }
  return (
    <div className={fit ? 'image-frame max-w-full overflow-hidden' : 'image-frame overflow-auto'}>
      <div className="relative inline-block max-w-full align-top">
        <img
          ref={imageRef}
          src={url}
          alt={alt}
          tabIndex={-1}
          {...(size ? { width: size.width, height: size.height } : {})}
          className={fit ? 'block h-auto max-w-full' : 'block max-w-none'}
          onLoad={(event) =>
            onLoaded?.({
              width: event.currentTarget.naturalWidth,
              height: event.currentTarget.naturalHeight,
            })
          }
        />
        {box && (
          <span
            aria-hidden="true"
            className="image-region pointer-events-none absolute border-[3px] border-[Highlight] outline outline-1 outline-[Canvas]"
            style={{
              left: `${box.left * 100}%`,
              top: `${box.top * 100}%`,
              width: `${box.width * 100}%`,
              height: `${box.height * 100}%`,
            }}
          />
        )}
      </div>
    </div>
  );
}
