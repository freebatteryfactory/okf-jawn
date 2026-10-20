/**
 * Which page of a PDF the viewer opens at for the reader's selection, and the sentence that says
 * so (UI M4a, ruling A2). A selection of pages opens at its first page and a region at its page;
 * a page the PDF does not have, or a selection a PDF's pages do not mark (lines of the text,
 * cells, a section), opens at page 1, and the sentence says why. The whole item says nothing.
 */
import type { Selection } from '../../../api/generated/types.gen';
import { describeSelection } from '../../documents/describe';

export interface Opening {
  /** The page to open at. */
  page: number;
  /** What the viewer says of the selection, or null when there is nothing to say. */
  said: string | null;
}

/** "1 page", "3 pages". */
export function pagesWords(count: number): string {
  return `${count} ${count === 1 ? 'page' : 'pages'}`;
}

/** The first page `selection` names, or null when it names none. */
function namedPage(selection: Selection): number | null {
  switch (selection.kind) {
    case 'pages':
      return selection.range.start;
    case 'region':
      return selection.region.page_no;
    default:
      return null;
  }
}

export function openingOf(selection: Selection, pageCount: number): Opening {
  if (selection.kind === 'all') return { page: 1, said: null };
  const shown = describeSelection(selection);
  const page = namedPage(selection);
  if (page === null) {
    const what = selection.kind === 'lines' ? `${shown} of the text` : shown;
    return {
      page: 1,
      said: `The reader shows ${what}, which a PDF’s pages do not mark: it opened at page 1.`,
    };
  }
  if (page < 1 || page > pageCount) {
    return {
      page: 1,
      said: `The reader shows ${shown}, but this PDF has ${pagesWords(pageCount)}: it opened at page 1.`,
    };
  }
  return { page, said: `The reader shows ${shown}: the PDF opened at page ${page}.` };
}
