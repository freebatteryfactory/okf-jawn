/**
 * A picture or a page render a read returned (`MediaReference`, ruling A2), as the multimodal and
 * pages views show it: the image, its alt text, and a caption that says what it is.
 *
 * - The alt text is the document's own caption; where the document gives none, the words say
 *   so, and what the image is (a picture, or the render of a page).
 * - The caption repeats the document's caption (or says there is none), says where the image
 *   lies in the original with how that was found (an inferred place is never shown as direct:
 *   SPEC section 5), and gives its recorded size and type.
 * - When the reader's selection is a region of the image's page, the region is outlined.
 *
 * The bytes are the media object's, read at the revision the read resolved (its citation's).
 */
import type { MediaReference, PageRegion } from '../../../api/generated/types.gen';
import { Locations, pageOf } from '../../reader/Locations';
import { ImageFrame } from './ImageFrame';

/** The page an image is of, when its citation or its place names one. */
export function mediaPage(media: MediaReference): number | null {
  const { selection, locations } = media.source;
  if (selection.kind === 'pages' && selection.range.start === selection.range.end) {
    return selection.range.start;
  }
  for (const location of locations ?? []) {
    const page = pageOf(location);
    if (page !== null) return page;
  }
  return null;
}

/** The alt text: the document's caption, or words that say there is none and what it is. */
export function mediaAlt(media: MediaReference): string {
  const caption = media.caption?.trim();
  if (caption) return caption;
  const page = mediaPage(media);
  const places = (media.source.locations ?? []).filter((place) => pageOf(place) === page);
  const inferred =
    page !== null &&
    places.some((place) => place.provenance === 'inferred') &&
    !places.some((place) => place.provenance === 'direct');
  if (inferred) {
    return media.role === 'page_image'
      ? `A page render inferred to be of page ${page}, not certain; the document gives it no caption.`
      : `A picture inferred to be on page ${page}, not certain; the document gives it no caption.`;
  }
  if (media.role === 'page_image') {
    return page === null
      ? 'A page as the converter rendered it; the document gives it no caption.'
      : `Page ${page} as the converter rendered it; the document gives it no caption.`;
  }
  return page === null
    ? 'A picture the document gives no caption.'
    : `A picture on page ${page} that the document gives no caption.`;
}

export function MediaImage({
  media,
  region,
}: {
  media: MediaReference;
  /** The reader's selection, when it is a region: outlined on the image of its page. */
  region: PageRegion | null;
}) {
  const page = mediaPage(media);
  const caption = media.caption?.trim() || undefined;
  const outlined = region !== null && page !== null && region.page_no === page ? region : null;
  const title =
    media.role === 'page_image' ? (page === null ? 'A page' : `Page ${page}`) : (caption ?? null);
  return (
    <figure tabIndex={-1} className="media-image m-0 grid gap-2">
      <ImageFrame
        address={{
          workspaceId: media.source.workspace_id,
          itemId: media.source.item_id,
          revision: media.source.revision,
          digest: media.object,
        }}
        mediaType={media.media_type}
        alt={mediaAlt(media)}
        size={{ width: media.width, height: media.height }}
        region={outlined}
        context={
          media.role === 'page_image' ? 'Reading this page’s render' : 'Reading this picture'
        }
      />
      <figcaption className="grid gap-1">
        {title !== null ? <strong>{title}</strong> : null}
        {media.role === 'picture' && caption === undefined ? (
          <span>No caption: the document gives this picture none.</span>
        ) : null}
        <Locations
          locations={media.source.locations ?? []}
          label={media.role === 'page_image' ? 'Where this page is' : 'Where this picture is'}
        />
        <span className="meta">
          {media.width} × {media.height} pixels, {media.media_type}
          {outlined ? '. The selected region is outlined.' : ''}
        </span>
      </figcaption>
    </figure>
  );
}
