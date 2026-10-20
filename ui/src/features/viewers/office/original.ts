/**
 * The original of a source as a file (UI M4a, ruling A2): the name it was supplied under, and the
 * address of its bytes for a plain link.
 *
 * The address is `download_object`'s route (`api/transports.json`) for the item at the exact
 * revision the reader shows, built by the generated client from its own route template under the
 * base it reaches the API at (`client.buildUrl`, as every request's address is built), so the
 * link goes to the same origin as every request. The template is typed by the generated
 * `DownloadObjectData`: a contract that moves the route fails type checking here.
 */
import { client } from '../../../api/generated/client.gen';
import type { DownloadObjectData, SourceAppearance } from '../../../api/generated/types.gen';

const OBJECT_ROUTE: DownloadObjectData['url'] =
  '/api/workspaces/{workspace_id}/items/{item_id}/revisions/{revision}/objects/{digest}';

export interface OriginalFile {
  workspaceId: string;
  itemId: string;
  /** The exact revision the bytes are authorized through. */
  revision: string;
  /** The item's path at that revision. */
  path: string;
  appearance: SourceAppearance;
}

/**
 * The name the original was supplied under: the filename of its latest observation, or, where
 * none was recorded, the last part of the item's path.
 */
export function originalName({ appearance, path }: Pick<OriginalFile, 'appearance' | 'path'>) {
  const latest = appearance.names.reduce<SourceAppearance['names'][number] | null>(
    (found, name) =>
      found === null || Date.parse(name.observed_at) > Date.parse(found.observed_at) ? name : found,
    null,
  );
  if (latest !== null && latest.filename.length > 0) return latest.filename;
  return path.split('/').at(-1) ?? path;
}

/** Where the original's bytes are read: `download_object` for that revision, on the API's base. */
export function originalHref({
  workspaceId,
  itemId,
  revision,
  appearance,
}: Omit<OriginalFile, 'path'>): string {
  return client.buildUrl<DownloadObjectData>({
    url: OBJECT_ROUTE,
    path: {
      workspace_id: workspaceId,
      item_id: itemId,
      revision,
      digest: appearance.object,
    },
  });
}

/** A media type without its parameters, in lower case. */
export function essence(mediaType: string): string {
  return (mediaType.split(';', 1)[0] ?? '').trim().toLowerCase();
}
