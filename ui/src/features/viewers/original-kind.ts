/**
 * Which viewer an item's original goes to (rulings A2, A3), by the media type the service sniffed
 * for its bytes (`SourceAppearance.media_type`). Parameters (`; charset=…`) and case are ignored.
 *
 * - `pdf`: PDF.js (`viewers/pdf`).
 * - `image`: the image viewer (`viewers/image`); an `<img>` runs no script, SVG included.
 * - `html`: the sandbox viewer (`viewers/html`), for `text/html` exactly. Nothing else is framed:
 *   not XHTML, not SVG, and a derived HTML object only where a conversion records one.
 * - `spreadsheet`: tables with ranges (`viewers/sheet`).
 * - `office`: a word-processing or presentation file, read as its conversion, with access to the
 *   original (`viewers/office`).
 * - `text`: plain text and code (`viewers/text`).
 * - `other`: no viewer; the page says so and gives access to the original.
 */

export type OriginalKind = 'pdf' | 'image' | 'html' | 'spreadsheet' | 'office' | 'text' | 'other';

const SPREADSHEETS: ReadonlySet<string> = new Set([
  'text/csv',
  'text/tab-separated-values',
  'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  'application/vnd.ms-excel',
  'application/vnd.oasis.opendocument.spreadsheet',
]);

const OFFICE: ReadonlySet<string> = new Set([
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  'application/msword',
  'application/vnd.ms-powerpoint',
  'application/vnd.oasis.opendocument.text',
  'application/vnd.oasis.opendocument.presentation',
  'application/rtf',
]);

/** Text that is not `text/*` but reads as text. */
const TEXT_APPLICATIONS: ReadonlySet<string> = new Set([
  'application/json',
  'application/xml',
  'application/yaml',
  'application/x-yaml',
  'application/javascript',
  'application/toml',
]);

/** The media type without its parameters, in lower case. */
function essence(mediaType: string): string {
  return (mediaType.split(';', 1)[0] ?? '').trim().toLowerCase();
}

export function originalKind(mediaType: string): OriginalKind {
  const type = essence(mediaType);
  if (type === 'application/pdf') return 'pdf';
  if (type === 'text/html') return 'html';
  if (SPREADSHEETS.has(type)) return 'spreadsheet';
  if (OFFICE.has(type)) return 'office';
  if (type.startsWith('image/')) return 'image';
  if (type.startsWith('text/') || TEXT_APPLICATIONS.has(type)) return 'text';
  return 'other';
}
