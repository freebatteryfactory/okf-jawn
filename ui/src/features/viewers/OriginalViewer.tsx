/**
 * An item's original, in the viewer its media type goes to (`original-kind.ts`; rulings A2, A3).
 * Each viewer is its own module and reads the bytes it needs itself: through `download_object`
 * (`app/byte-read.ts`), or, for HTML, through a sandbox capability.
 */
import { HtmlSandbox } from './html/HtmlSandbox';
import { ImageOriginal } from './image/ImageOriginal';
import { OfficeOriginal } from './office/OfficeOriginal';
import { OtherOriginal } from './office/OtherOriginal';
import { originalKind } from './original-kind';
import { PdfViewer } from './pdf/PdfViewer';
import { SpreadsheetView } from './sheet/SpreadsheetView';
import { TextOriginal } from './text/TextOriginal';
import type { OriginalProps } from './types';

export function OriginalViewer(props: OriginalProps) {
  switch (originalKind(props.appearance.media_type)) {
    case 'pdf':
      return <PdfViewer {...props} />;
    case 'image':
      return <ImageOriginal {...props} />;
    case 'html':
      return <HtmlSandbox {...props} />;
    case 'spreadsheet':
      return <SpreadsheetView {...props} />;
    case 'office':
      return <OfficeOriginal {...props} />;
    case 'text':
      return <TextOriginal {...props} />;
    case 'other':
      return <OtherOriginal {...props} />;
  }
}
