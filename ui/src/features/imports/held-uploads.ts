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
 *
 * Both lists are asked again, too, whenever this page's own list lets go of a slot the service
 * still holds (`useHeldUploadsFollow`): a file removed from the list after bytes were sent, or a
 * slot left for a new one. The import page leaves out of these lists the slots its own list holds,
 * so their pages, read before the slot was opened, lack it without showing that; once the page
 * lets it go, the slot is listed, as the page then says, and can be discarded, with no reload.
 * Opening a slot needs no asking: while the page's list holds it, these lists leave it out anyway.
 * Limit: removing a file whose `create_upload` is in flight aborts it, and the page never learns
 * whether the service opened the slot (`uploading.ts`); such a slot is listed at the next listing.
 */
import { infiniteQueryOptions, type QueryClient, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { discardUpload, listUploads } from '../../api/generated/sdk.gen';
import type { Upload } from '../../api/generated/types.gen';
import { type UploadEntry, useUploads } from './upload-list';

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

/**
 * The workspaces where a slot the page's list held in `before` (an entry's slot or its verified
 * upload) is held by no entry in `now`: removed, or left for a new slot.
 */
export function slotsLetGo(
  before: readonly UploadEntry[],
  now: readonly UploadEntry[],
): ReadonlySet<string> {
  const workspaces = new Set<string>();
  if (before === now) return workspaces;
  const kept = new Set<string>();
  for (const entry of now) {
    if (entry.slotId !== null) kept.add(entry.slotId);
    if (entry.uploadId !== null) kept.add(entry.uploadId);
  }
  for (const entry of before) {
    for (const id of [entry.slotId, entry.uploadId]) {
      if (id !== null && !kept.has(id)) workspaces.add(entry.workspaceId);
    }
  }
  return workspaces;
}

/**
 * Ask the uploads the service holds again whenever the page's list lets a slot go. Mounted with the
 * application's root (`routes/__root.tsx`), as the uploads run outside the import page: a slot left
 * while the person looks at something else is listed when the import page is shown again, not
 * only once its lists are older than the cache keeps them. An import also lets slots go (they are
 * consumed); `beginImport` asks both lists again itself, for the held uploads it took as well.
 */
export function useHeldUploadsFollow() {
  const client = useQueryClient();
  useEffect(
    () =>
      useUploads.subscribe((now, before) => {
        for (const workspaceId of slotsLetGo(before.entries, now.entries)) {
          heldUploadsChanged(client, workspaceId);
        }
      }),
    [client],
  );
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
