/**
 * Which read views an item has (ruling A1; SPEC section 7: "The read operation offers outline,
 * text, multimodal, pages and original"), found from the item's own fields as `get_item` answers
 * them, by the rules `read_item` serves them with (`crates/core/src/application/reads.rs`):
 *
 * - **Text** and **Outline**: every item. A Note's text is its Markdown, a View's its prose and
 *   block, a Source's the text shown for it (the correction, the supplied text or the
 *   converter's); a Source with no text yet is answered as having none, with the service's
 *   warning, and the reader says so. The outline is the text's structure.
 * - **Text and pictures** (`multimodal`): the text, with the pictures the conversion record keeps.
 *   Only a Source has one, and only once a document was produced (`extraction.digest`, present
 *   for a completed or partial conversion): a pending, failed or unsupported conversion keeps none.
 * - **Pages**: the converter's page renders. A Source only (`read_item` refuses `pages` of an item
 *   with no original), with a conversion record, of a paginated format (`extraction.page_count`),
 *   converted with page renders kept (`converter.settings.page_images`).
 * - **Original**: the source's own bytes, for a Source the service describes
 *   (`ItemDocument.source`); a Note and a View have none.
 *
 * A view the item cannot answer is never offered, and the reader says in words why it is not.
 * Until `get_item` has answered, the item's kind is not known: Text and Outline are offered, and
 * nothing is said refused.
 */
import type {
  ItemDocument,
  ItemKind,
  ReadView,
  SourceAppearance,
} from '../../api/generated/types.gen';

/** The views in the order the reader offers them. */
export const VIEW_ORDER: readonly ReadView[] = [
  'text',
  'outline',
  'multimodal',
  'pages',
  'original',
];

/** Each view's name, as the reader's switch and its words say it. */
export const VIEW_NAMES: Record<ReadView, string> = {
  text: 'Text',
  outline: 'Outline',
  multimodal: 'Text and pictures',
  pages: 'Pages',
  original: 'Original',
};

export interface RefusedView {
  view: ReadView;
  /** Why the item cannot answer it, as a clause: "a Note has no pages". */
  reason: string;
}

export interface ItemViews {
  /** The views offered, in `VIEW_ORDER`. */
  offered: ReadView[];
  /** The views not offered, each with its reason. */
  refused: RefusedView[];
}

/** What of an item decides its views: its kind, and for a Source what the service says of it. */
export interface ItemShape {
  kind: ItemKind;
  source: SourceAppearance | null;
}

export function shapeOf(document: ItemDocument): ItemShape {
  return { kind: document.summary.kind, source: document.source ?? null };
}

const NOUN: Record<ItemKind, string> = { note: 'a Note', source: 'a Source', view: 'a View' };

/** Why a source keeps nothing converted yet, or null when a document was produced. */
function notConverted(source: SourceAppearance, what: string): string | null {
  const { extraction } = source;
  if (extraction.digest !== null && extraction.digest !== undefined) return null;
  switch (extraction.outcome.status) {
    case 'pending':
      return `its conversion is pending, so it keeps no ${what} yet`;
    case 'failed':
      return `its conversion failed, so it keeps no ${what}`;
    case 'unsupported':
      return `its format is not converted, so it has no ${what}`;
    default:
      return `no conversion record of it is kept, so it has no ${what}`;
  }
}

function refusal(shape: ItemShape, view: ReadView): string | null {
  const { kind, source } = shape;
  if (view === 'text' || view === 'outline') return null;
  if (kind !== 'source') {
    switch (view) {
      case 'multimodal':
        return `${NOUN[kind]} keeps no pictures of its own; pictures are kept from a converted Source`;
      case 'pages':
        return `${NOUN[kind]} has no pages; page renders are kept from a converted Source`;
      case 'original':
        return `${NOUN[kind]} has no original file: its text is the item`;
    }
  }
  if (source === null) return 'the service did not describe this source’s original';
  switch (view) {
    case 'multimodal':
      return notConverted(source, 'pictures');
    case 'pages': {
      const unconverted = notConverted(source, 'page renders');
      if (unconverted !== null) return unconverted;
      const { converter, page_count } = source.extraction;
      if (page_count === null || page_count === undefined || page_count < 1) {
        return 'its format has no pages to render';
      }
      if (converter?.settings.page_images !== true) {
        return 'its conversion did not keep page renders';
      }
      return null;
    }
    case 'original':
      return null;
  }
}

/** The views an item has; `null` while what it is, is not known. */
export function viewsOf(shape: ItemShape | null): ItemViews {
  if (shape === null) return { offered: ['text', 'outline'], refused: [] };
  const offered: ReadView[] = [];
  const refused: RefusedView[] = [];
  for (const view of VIEW_ORDER) {
    const reason = refusal(shape, view);
    if (reason === null) offered.push(view);
    else refused.push({ view, reason });
  }
  return { offered, refused };
}

/** The words for the views refused: "Not offered here: Pages, because …; Original, because …." */
export function refusedWords(refused: readonly RefusedView[]): string | null {
  if (refused.length === 0) return null;
  const each = refused.map(({ view, reason }) => `${VIEW_NAMES[view]}, because ${reason}`);
  return `Not offered here: ${each.join('; ')}.`;
}
