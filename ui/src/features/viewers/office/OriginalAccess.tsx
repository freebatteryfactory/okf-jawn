/**
 * Access to an original's bytes (UI M4a, ruling A2): a plain link to them, named by the file the
 * original was supplied as, with its size and media type beside it. The Office viewer, the
 * original with no viewer and a PDF that PDF.js cannot open all offer it.
 *
 * The link is not a request of the page's: a plain `<a href={download_object path} download>` is
 * the one exception to the transport rule here, as it is for a finished job's artifact
 * (`features/jobs/ArtifactDownload.tsx`). Why a link and not the byte read handed to an object
 * URL:
 * - an original can be larger than memory, and than what the byte read allows: that read bounds
 *   its whole answer by the transport's time limit (`REQUEST_TIMEOUT_MS`), which a large file on a
 *   slow link would not meet, and an object URL needs the whole Blob held in memory first. The
 *   browser follows a link itself and saves the file as it comes.
 * - the link keeps to the same rules: it goes to this origin (`original.ts`); it is a `GET`, which
 *   needs no CSRF token; the browser sends the session cookie itself; `download_object` answers
 *   every download as an attachment, with `Content-Security-Policy: sandbox`, so the file never
 *   becomes a page of this origin (`crates/server/src/transfers.rs`); and the service authorizes
 *   it again, through the exact revision the reader shows.
 * What it gives up: a refusal is the browser's failed download, not words of this page. The
 * `download` attribute gives the file its supplied name; the service's answer names none.
 *
 * Reading an original is a read: it is offered to everyone who can open the item, in an archived
 * workspace too, and the service decides on every download.
 */
import { sizeWords } from '../../jobs/ArtifactDownload';
import { type OriginalFile, originalHref, originalName } from './original';

export function OriginalAccess(file: OriginalFile) {
  const name = originalName(file);
  return (
    <p className="original-access">
      <a className="underline" href={originalHref(file)} download={name}>
        Download the original, {name}
      </a>{' '}
      <span className="meta">
        ({sizeWords(file.appearance.size)}, <code>{file.appearance.media_type}</code>)
      </span>
    </p>
  );
}
