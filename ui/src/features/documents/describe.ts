/**
 * Plain words for selections, locators and location provenance.
 *
 * An inferred location is always worded as inferred and never with the certainty of a direct
 * one (SPEC section 5); an unresolved one says why it is unknown.
 */
import type { z } from 'zod';
import type {
  zCellRange,
  zConversionOutcome,
  zExtractionStatus,
  zExtractionSummary,
  zExtractionWarning,
  zFailureReason,
  zPageRange,
  zSelection,
  zSourceLocation,
  zSourceLocator,
  zTextOrigin,
  zUncitedReason,
  zUnresolvedReason,
} from '../../api/generated/zod.gen';
import { wordsFor } from '../../lib/words';

type CellRange = z.infer<typeof zCellRange>;
type ConversionOutcome = z.infer<typeof zConversionOutcome>;
type ExtractionStatus = z.infer<typeof zExtractionStatus>;
type ExtractionSummary = z.infer<typeof zExtractionSummary>;
type ExtractionWarning = z.infer<typeof zExtractionWarning>;
type FailureReason = z.infer<typeof zFailureReason>;
type PageRange = z.infer<typeof zPageRange>;
type Selection = z.infer<typeof zSelection>;
type SourceLocation = z.infer<typeof zSourceLocation>;
type SourceLocator = z.infer<typeof zSourceLocator>;
type TextOrigin = z.infer<typeof zTextOrigin>;
type UncitedReason = z.infer<typeof zUncitedReason>;
type UnresolvedReason = z.infer<typeof zUnresolvedReason>;

const span = (noun: string, plural: string, start: number, end: number) =>
  start === end ? `${noun} ${start}` : `${plural} ${start}–${end}`;

export function describeCells(range: CellRange): string {
  return `sheet “${range.sheet}”, ${span('row', 'rows', range.row_start, range.row_end)}, ${span('column', 'columns', range.column_start, range.column_end)}`;
}

export function describePages(ranges: readonly PageRange[]): string {
  return ranges.map((range) => span('page', 'pages', range.start, range.end)).join(', ');
}

export function describeSelection(selection: Selection): string {
  switch (selection.kind) {
    case 'all':
      return 'the whole item';
    case 'lines':
      return span('line', 'lines', selection.range.start, selection.range.end);
    case 'pages':
      return span('page', 'pages', selection.range.start, selection.range.end);
    case 'cells':
      return describeCells(selection.range);
    case 'section':
      return `section “${selection.heading}”`;
    case 'region':
      return `a region on page ${selection.region.page_no}`;
  }
}

export function describeLocator(locator: SourceLocator): string {
  switch (locator.kind) {
    case 'page':
      return `page ${locator.page_no}`;
    case 'region': {
      const { bbox, page_no } = locator.region;
      return `a region on page ${page_no} (left ${bbox.l}, top ${bbox.t}, right ${bbox.r}, bottom ${bbox.b})`;
    }
    case 'cells':
      return describeCells(locator.range);
  }
}

const UNRESOLVED: Record<Exclude<UnresolvedReason['kind'], 'ambiguous_match'>, string> = {
  format_has_no_locator: 'this format has no page or cell locations',
  not_located_by_converter: 'the converter did not record where this is',
  no_text_layer: 'the original has no text layer to search',
  no_text_to_match: 'there is no text to match',
  no_page_to_search: 'there is no page to search',
  no_match: 'the text was not found in the original',
  match_not_a_location: 'the match in the original does not give a location',
  corrected_text: 'a person corrected the text, so it no longer matches the original',
  supplied_text: 'an agent supplied the text; it was not read from the original',
};

export function describeUnresolved(reason: UnresolvedReason): string {
  return reason.kind === 'ambiguous_match'
    ? `the text appears ${reason.occurrences} times in the original`
    : UNRESOLVED[reason.kind];
}

/** The provenance label and the words for one location. */
export function describeLocation(location: SourceLocation): { label: string; text: string } {
  switch (location.provenance) {
    case 'direct':
      return {
        label: 'Direct location',
        text: `${describeLocator(location.locator)}, recorded from the document itself`,
      };
    case 'inferred':
      return {
        label: 'Inferred location, not certain',
        text: `${describeLocator(location.locator)}, found by matching text or layout`,
      };
    case 'unresolved':
      return { label: 'Location unknown', text: describeUnresolved(location.reason) };
  }
}

export const UNCITED: Record<UncitedReason, string> = {
  external: 'a reference outside this workspace',
  scope: 'outside the workspace you may read',
  not_found: 'nothing at that path at this revision',
  malformed: 'not a source entry this application can read',
};

export const EXTRACTION_STATUS: Record<ExtractionStatus, string> = {
  pending: 'Conversion is pending; the original bytes are kept.',
  completed: 'Converted.',
  partial: 'Partly converted.',
  failed: 'Conversion failed; the original bytes are kept.',
  unsupported: 'Bytes kept; text extraction is not supported for this format.',
};

/**
 * The mark a source carries wherever it is named (a row of the tree, a tab, a job's list of
 * items): how its conversion stands, when it is anything but converted. A converted source, and
 * an item that is not a source, carries none. "format not supported" is its own word: such a
 * source is never marked, or left unmarked, as converted.
 */
export const CONVERSION_MARK: Record<Exclude<ExtractionStatus, 'completed'>, string> = {
  pending: 'conversion pending',
  partial: 'partly converted',
  failed: 'conversion failed',
  unsupported: 'format not supported',
};

/**
 * A state this build does not know (a newer service) is marked as unknown, with the service's own
 * word. It is never left unmarked: no mark is how a converted source looks.
 */
export function conversionWord(
  extraction: { status: ExtractionStatus } | null | undefined,
): string | null {
  if (extraction === null || extraction === undefined) return null;
  if (extraction.status === 'completed') return null;
  return (
    wordsFor(CONVERSION_MARK, extraction.status) ??
    `conversion state not known: “${extraction.status}”`
  );
}

/** How a conversion stands, as a sentence; a state this build does not know is said to be unknown. */
export function extractionSentence(status: string): string {
  return (
    wordsFor(EXTRACTION_STATUS, status) ??
    `The conversion is in a state this page does not know (“${status}”). It is not known to be converted.`
  );
}

export const TEXT_ORIGIN: Record<TextOrigin, string> = {
  converter: 'The text shown is the converter’s.',
  supplied_by_agent:
    'The text shown was supplied by an agent and accepted by a person; it is not the converter’s.',
  none: 'There is no text to show.',
};

export const CORRECTED = 'A person’s correction is shown over the converted text.';

/**
 * The labels a source's text carries wherever it is shown (SPEC section 5: agent-supplied text
 * is labelled as such, and generated extraction and human corrections stay distinguishable):
 * partial or failed conversion, text not from the converter, and a person's correction.
 */
export function extractionLabels(extraction: ExtractionSummary | null | undefined): string[] {
  if (extraction === null || extraction === undefined) return [];
  const labels: string[] = [];
  if (extraction.status !== 'completed') labels.push(EXTRACTION_STATUS[extraction.status]);
  if (extraction.text_origin !== 'converter') labels.push(TEXT_ORIGIN[extraction.text_origin]);
  if (extraction.corrected) labels.push(CORRECTED);
  return labels;
}

/**
 * The labels of a source's text that say whose text it is: not the converter's, or corrected by a
 * person. How the conversion itself stands is said apart (`ConversionState`).
 */
export function textLabels(extraction: ExtractionSummary | null | undefined): string[] {
  if (extraction === null || extraction === undefined) return [];
  const labels: string[] = [];
  if (extraction.text_origin !== 'converter') labels.push(TEXT_ORIGIN[extraction.text_origin]);
  if (extraction.corrected) labels.push(CORRECTED);
  return labels;
}

export function describeFailure(reason: FailureReason): string {
  switch (reason.kind) {
    case 'damaged':
      return 'the file is damaged';
    case 'memory_limit':
      return `it needed more than the memory limit of ${reason.limit_bytes} bytes`;
    case 'time_limit':
      return `it took longer than ${reason.limit_seconds} seconds`;
    case 'converter_crashed':
      return 'the converter stopped unexpectedly';
    case 'converter_error':
      return 'the converter reported an error';
  }
}

export function describeOutcome(outcome: ConversionOutcome): string {
  switch (outcome.status) {
    case 'partial':
      return outcome.coverage?.not_converted.length
        ? `${EXTRACTION_STATUS.partial} Not converted: ${describePages(outcome.coverage.not_converted)}.`
        : EXTRACTION_STATUS.partial;
    case 'failed':
      return `Conversion failed because ${describeFailure(outcome.reason)}; the original bytes are kept.`;
    default:
      return extractionSentence(outcome.status);
  }
}

export function describeWarning(warning: ExtractionWarning): string {
  switch (warning.kind) {
    case 'undecodable_glyphs':
      return warning.pages.length > 0
        ? `Some characters could not be decoded, on ${describePages(warning.pages.map((page) => ({ start: page.page_no, end: page.page_no })))}.`
        : 'Some characters could not be decoded.';
    case 'inferred_locations':
      return `${warning.items} locations were inferred by matching text or layout, not recorded directly.`;
    case 'unlocated_items':
      return `${warning.items} parts have no known location in the original.`;
    case 'cell_values_only':
      return `Only cell values were read, not formulas or formats, in: ${warning.sheets.join(', ')}.`;
  }
}

/** A revision or digest shortened for display; the full value goes in a title. */
export function short(value: string): string {
  return value.slice(0, 10);
}
