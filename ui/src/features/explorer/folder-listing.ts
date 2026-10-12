/**
 * Folder listings for the tree: the listing operation, one folder at a time, only once a
 * folder is expanded, and page by page through the operation's cursor.
 *
 * Each page is its own generated `list_items` query; a folder's view is the pages it shows,
 * fetched in cursor order. Every page is pinned to one resolved revision, so pages never mix
 * two states of the workspace. A later page that fails keeps the pages already shown.
 */
import { type QueryClient, queryOptions } from '@tanstack/react-query';
import { listItemsOptions } from '../../api/generated/@tanstack/react-query.gen';
import type { ListItemsResponse, Revision } from '../../api/generated/types.gen';

/** Entries requested per listing page. */
export const LISTING_PAGE = 200;

export interface FolderListing {
  pages: ListItemsResponse[];
  /** The failure of the page after the last one shown, if it failed. */
  moreError: unknown;
}

export function listingPageOptions(
  workspaceId: string,
  revision: Revision,
  folder: string,
  cursor: string | null,
) {
  return listItemsOptions({
    body: {
      workspace_id: workspaceId,
      at: { kind: 'revision', revision },
      folder,
      page: { limit: LISTING_PAGE, cursor },
    },
  });
}

const folderListingKey = (workspaceId: string, revision: Revision, folder: string, pages: number) =>
  ['explorer-folder', workspaceId, revision, folder, pages] as const;

export function folderListingOptions(
  client: QueryClient,
  workspaceId: string,
  revision: Revision,
  folder: string,
  pages: number,
) {
  return queryOptions({
    queryKey: folderListingKey(workspaceId, revision, folder, pages),
    queryFn: async (): Promise<FolderListing> => {
      const shown: ListItemsResponse[] = [];
      let cursor: string | null = null;
      for (let index = 0; index < pages; index += 1) {
        try {
          // A revision-pinned page never changes, so a page already fetched is reused.
          const page: ListItemsResponse = await client.fetchQuery({
            ...listingPageOptions(workspaceId, revision, folder, cursor),
            staleTime: Number.POSITIVE_INFINITY,
          });
          shown.push(page);
          if (page.next_cursor === null || page.next_cursor === undefined) break;
          cursor = page.next_cursor;
        } catch (error) {
          if (index === 0) throw error;
          return { pages: shown, moreError: error };
        }
      }
      return { pages: shown, moreError: null };
    },
    // While one more page loads, the pages already shown stay on screen (and keep focus).
    placeholderData: () =>
      client.getQueryData<FolderListing>(
        folderListingKey(workspaceId, revision, folder, pages - 1),
      ),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** The folders whose children are on screen: the root, and expanded folders under expanded parents. */
export function visibleFolders(expanded: Record<string, true>): string[] {
  const shown = (folder: string): boolean => {
    const cut = folder.lastIndexOf('/');
    return cut < 0 || (expanded[folder.slice(0, cut)] === true && shown(folder.slice(0, cut)));
  };
  return ['', ...Object.keys(expanded).filter(shown).sort()];
}

/** Every folder above `path`, outermost first: `a/b/c.md` gives `a` and `a/b`. */
export function ancestorFolders(path: string): string[] {
  const parts = path.split('/');
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join('/'));
}
