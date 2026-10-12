/**
 * The URL's search parameters, validated with the generated schemas.
 *
 * `q` is the workspace search; `sel` is the selection inside the open item and `rev` the exact
 * revision it was cited at. A deep link with these reopens the same item and selection. A value
 * that does not validate is dropped rather than sent to the server as a guess.
 */
import type { z } from 'zod';
import { zRevision, zSelection } from '../../api/generated/zod.gen';

export type ItemSelection = z.infer<typeof zSelection>;

export interface WorkspaceSearch {
  q?: string;
}

export interface ItemSearch {
  sel?: ItemSelection;
  rev?: string;
}

export function validateWorkspaceSearch(raw: Record<string, unknown>): WorkspaceSearch {
  const q = raw.q;
  return typeof q === 'string' && q.trim().length > 0 ? { q } : {};
}

export function validateItemSearch(raw: Record<string, unknown>): ItemSearch {
  const search: ItemSearch = {};
  const sel = zSelection.safeParse(raw.sel);
  if (sel.success && sel.data.kind !== 'all') search.sel = sel.data;
  const rev = zRevision.safeParse(raw.rev);
  if (rev.success) search.rev = rev.data;
  return search;
}
