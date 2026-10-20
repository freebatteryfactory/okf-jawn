/**
 * A Source's corrections (UI M4a, ruling A4): correct its text, keep supplied text, reconvert. A
 * placeholder of the M4a skeleton: the builder of ruling A4 replaces it.
 */
import type { ItemDocument, Revision } from '../../api/generated/types.gen';

export interface CorrectionsProps {
  workspaceId: string;
  itemId: string;
  /** The revision the reader shows. */
  revision: Revision;
  /** The workspace's head: a correction is based on it. */
  head: Revision;
  /** The item as `get_item` answered at `revision`; a Source has `source`. */
  document: ItemDocument;
}

export function Corrections(_props: CorrectionsProps) {
  return <p className="meta">Corrections are not built yet.</p>;
}
