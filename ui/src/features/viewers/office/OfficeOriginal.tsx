/**
 * An Office file's original (UI M4a, ruling A2; SPEC section 7: Office extraction with original
 * access): a word-processing document or a presentation is shown as its conversion, the text the
 * reader's text view holds, and its own bytes are offered as a download (`OriginalAccess.tsx`).
 * No Office file is drawn in the page.
 *
 * The words say whose text the reader shows, as the service recorded it (`Extraction`): the
 * converter's (all of it, or only part, and with a person's corrections over it), an agent's that
 * a person accepted, or none. With none, they say why: not converted yet, the converter could not
 * read it, or its format is not converted. A conversion the service did not record is never
 * claimed. How the conversion stands in detail is the reader's (`ConversionState`).
 */
import type { Extraction } from '../../../api/generated/types.gen';
import { wordsFor } from '../../../lib/words';
import type { OriginalProps } from '../types';
import { OriginalAccess } from './OriginalAccess';
import { essence } from './original';

/** What a file of each Office media type is, inside a sentence. */
const KINDS: Readonly<Record<string, string>> = {
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document': 'Word document',
  'application/msword': 'Word document',
  'application/vnd.openxmlformats-officedocument.presentationml.presentation':
    'PowerPoint presentation',
  'application/vnd.ms-powerpoint': 'PowerPoint presentation',
  'application/vnd.oasis.opendocument.text': 'OpenDocument text',
  'application/vnd.oasis.opendocument.presentation': 'OpenDocument presentation',
  'application/rtf': 'Rich Text document',
};

const NOT_DRAWN = 'The file itself is not drawn here.';

/** How the file is shown, as the service recorded its text. */
export function shownAs(kind: string, { outcome, text_origin, corrected }: Extraction): string {
  // As text, for a value a newer service may send that this build does not name.
  const origin: string = text_origin;
  const status: string = outcome.status;
  switch (text_origin) {
    case 'converter': {
      const read =
        outcome.status === 'partial'
          ? 'the text the converter read from it, which is not all of it'
          : 'the text the converter read from it';
      const over = corrected ? ', with a person’s corrections over it' : '';
      return `This ${kind} is shown as its conversion: the reader’s text view has ${read}${over}. ${NOT_DRAWN}`;
    }
    case 'supplied_by_agent':
      return `This ${kind} is shown as text an agent supplied for it, which a person accepted: the reader’s text view has it. It is not the converter’s reading of the file, and the file itself is not drawn here.`;
    case 'none':
      break;
    default:
      return `This ${kind} is shown as text of an origin this page does not know (“${origin}”). ${NOT_DRAWN}`;
  }
  switch (outcome.status) {
    case 'pending':
      return `This ${kind} has no conversion yet: the reader’s text view will show it once the converter has read the file. ${NOT_DRAWN}`;
    case 'failed':
      return `This ${kind} has no conversion to show: the converter could not read it. ${NOT_DRAWN}`;
    case 'unsupported':
      return `This ${kind} has no conversion to show: its format is not converted by this service. ${NOT_DRAWN}`;
    case 'completed':
    case 'partial':
      return `This ${kind} has no text to show. ${NOT_DRAWN}`;
    default:
      return `This ${kind} has no text to show: its conversion is in a state this page does not know (“${status}”). ${NOT_DRAWN}`;
  }
}

export function OfficeOriginal({ workspaceId, itemId, revision, path, appearance }: OriginalProps) {
  const kind = wordsFor(KINDS, essence(appearance.media_type)) ?? 'Office file';
  return (
    <div className="office-original grid gap-2">
      <p>{shownAs(kind, appearance.extraction)}</p>
      <p>Its original is kept, and can be downloaded:</p>
      <OriginalAccess
        workspaceId={workspaceId}
        itemId={itemId}
        revision={revision}
        path={path}
        appearance={appearance}
      />
    </div>
  );
}
