/**
 * What a viewer of an item's original is given (ruling A2): the item at an exact revision, its
 * source's bytes as the service describes them (`SourceAppearance`: digest, media type, size,
 * extraction), and the selection the reader shows, which a viewer honours where it can (pages for
 * a PDF, cells for a spreadsheet, lines for text).
 */
import type { Revision, Selection, SourceAppearance } from '../../api/generated/types.gen';

export interface OriginalProps {
  workspaceId: string;
  itemId: string;
  /** The exact revision read; the bytes are authorized through it. */
  revision: Revision;
  /** The item's path at that revision. */
  path: string;
  /** The source's bytes and how they were turned into text. */
  appearance: SourceAppearance;
  /** What the reader shows of the item. */
  selection: Selection;
}
