/**
 * An image's bytes in the browser cache, and the object URL an `<img>` shows them through
 * (ruling A2).
 *
 * The bytes are read once per object of an item revision through `download_object`
 * (`app/byte-read.ts`), and kept by TanStack Query under a key holding the workspace, the item,
 * the revision it is authorized through and the object's digest (SPEC section 7: browser cache
 * keys hold workspace, source, revision and representation). An object's bytes never change
 * (the digest is their SHA-256), so a cached answer is never stale.
 *
 * The object URL is made for the image on screen and revoked when the image lets it go: when it
 * leaves the page, or shows other bytes. The service sends an image's own type only for the
 * types it holds inert (`crates/server/src/transfers.rs` `INERT_TYPES`) and `application/octet-stream`
 * otherwise, so the bytes are given the media type the service recorded for them before an
 * `<img>` shows them (an SVG is shown only as `image/svg+xml`). An `<img>` runs no script, SVG
 * included; the page's policy still applies to the URL.
 */
import { queryOptions } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { readObjectBytes } from '../../../app/byte-read';

/** One object of an item, at the exact revision it is authorized through. */
export interface ObjectAddress {
  workspaceId: string;
  itemId: string;
  revision: string;
  digest: string;
}

/** The browser cache key of an object's bytes. */
export function objectBytesKey({ workspaceId, itemId, revision, digest }: ObjectAddress) {
  return ['objectBytes', { workspace_id: workspaceId, item_id: itemId, revision, digest }] as const;
}

export function objectBytesOptions(address: ObjectAddress) {
  return queryOptions({
    queryKey: objectBytesKey(address),
    queryFn: ({ signal }) => readObjectBytes({ ...address, signal }),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** Whether a media type names an image an `<img>` may show. */
export function isImageType(mediaType: string): boolean {
  return (mediaType.split(';', 1)[0] ?? '').trim().toLowerCase().startsWith('image/');
}

/**
 * An object URL for `blob` as `mediaType`, for as long as the caller shows it: made when the bytes
 * are there, revoked when they change or the caller leaves the page.
 */
export function useObjectUrl(blob: Blob | undefined, mediaType: string): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (blob === undefined) {
      setUrl(null);
      return;
    }
    const type = (mediaType.split(';', 1)[0] ?? '').trim().toLowerCase();
    const typed = blob.type === type ? blob : new Blob([blob], { type });
    const made = URL.createObjectURL(typed);
    setUrl(made);
    return () => {
      URL.revokeObjectURL(made);
    };
  }, [blob, mediaType]);
  return url;
}
