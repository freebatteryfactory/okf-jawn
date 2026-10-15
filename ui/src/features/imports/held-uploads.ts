/**
 * The uploads the service holds for a workspace (`list_uploads`), and dropping one
 * (`discard_upload`): ruling 3 of contract batch 2.
 *
 * Nothing expires: a slot stays until it is discarded, imported, or its workspace is purged (owner
 * decision of 2026-10-14). So the import page shows what is waiting, in two lists asked for apart:
 * the uploads that are complete and taken by no job (`waiting`: ready to import), and those that
 * were opened and never finished (`incomplete`). Each is paged through its cursor, newest first as
 * the service orders them, `UPLOADS_PAGE` at a time; a further page is asked for only when the
 * person asks.
 *
 * Discarding removes the slot and its bytes. It is one request under one idempotency key, repeated
 * under that key while it is the same upload, so a discard whose answer was lost is not a second.
 * Once it is answered, and once an import has started (which consumes the uploads it names), both
 * lists are asked again.
 */
import { infiniteQueryOptions, type QueryClient } from '@tanstack/react-query';
import { discardUpload, listUploads } from '../../api/generated/sdk.gen';
import type { Upload } from '../../api/generated/types.gen';

/** Uploads requested per page. */
export const UPLOADS_PAGE = 50;

/** The two lists the page shows. */
export type HeldState = 'waiting' | 'incomplete';

/**
 * The cache key of one list. The generated client offers `list_uploads` only as a mutation (it
 * needs `write`), so the key is this page's own.
 */
export function heldUploadsKey(workspaceId: string, state: HeldState) {
  return ['held-uploads', workspaceId, state] as const;
}

/** The uploads the service holds in `state`, newest first. */
export function heldUploadsOptions(workspaceId: string, state: HeldState) {
  return infiniteQueryOptions({
    queryKey: heldUploadsKey(workspaceId, state),
    queryFn: async ({ pageParam, signal }) => {
      const { data } = await listUploads({
        body: {
          workspace_id: workspaceId,
          state,
          page: { limit: UPLOADS_PAGE, cursor: pageParam },
        },
        signal,
        throwOnError: true,
      });
      return data;
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** Ask both lists again: an upload was discarded, or an import took some. */
export function heldUploadsChanged(client: QueryClient, workspaceId: string) {
  for (const state of ['waiting', 'incomplete'] as const) {
    void client.invalidateQueries({ queryKey: heldUploadsKey(workspaceId, state) });
  }
}

/** Discard an upload the service holds: its slot and its bytes. Resolves with its last state. */
export async function discardHeld(
  client: QueryClient,
  workspaceId: string,
  uploadId: string,
  key: string,
): Promise<Upload> {
  const { data } = await discardUpload({
    body: { workspace_id: workspaceId, upload_id: uploadId, idempotency_key: key },
    throwOnError: true,
  });
  heldUploadsChanged(client, workspaceId);
  return data;
}

/** An upload's name as the person supplied it, with its folder; null when the service kept none. */
export function heldPath(upload: Upload): string | null {
  const name = upload.filename ?? null;
  if (name === null || name === '') return null;
  const folder = upload.relative_path ?? '';
  return folder === '' ? name : `${folder}/${name}`;
}
