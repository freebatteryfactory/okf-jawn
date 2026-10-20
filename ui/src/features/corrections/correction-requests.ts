/**
 * What a source's corrections ask of the service, and the words they are said in (UI M4a, ruling
 * A4; SPEC section 5: "Generated extraction and human corrections remain distinguishable").
 *
 * Where each request takes its values from, as the generated client and `crates/core` say:
 * - `correct_digest` is based on the head the workspace shows (`base_revision`; the service
 *   refuses another as `conflict` with `stale_base` on `/base_revision`), and names the item's
 *   digest at that head: `SourceAppearance.extraction.digest` of `get_item` there, the conversion
 *   record its text was read from (`invalid_input` on `/digest` otherwise). A source with no
 *   record (pending, failed, unsupported) has nothing to correct.
 * - Keeping the supplied text is the same request with `adopts`, the accepted proposal the text
 *   came from (`Extraction.supplied.proposal_id`), and `corrected_markdown` exactly that
 *   proposal's text for the item (its `supply_extraction` change, read with `get_proposal`): the
 *   service compares the two by SHA-256 digest. The body the reader shows is not that text (the
 *   store keeps a body without its last line break), so it is never sent for it.
 * - `redigest_item` takes the settings of the source's current conversion, the only settings the
 *   wire reports (`Extraction.converter.settings`, present exactly when a converter ran). A
 *   conversion of only the pages not converted takes those same settings (the service refuses
 *   others on `/settings`), and is offered only when the conversion names such pages
 *   (`ConversionOutcome::Partial.coverage.not_converted`). A source no converter ran on (pending,
 *   unsupported) has no settings on the wire, and nothing reports the deployment's defaults, so it
 *   is not reconverted from here (a contract request: see the M4a report).
 *
 * The revision a correction makes becomes the head the workspace shows when it still shows the
 * base (`adoptHeadFrom`), as for every change of the person's own. Before that, the item is read at
 * the new revision (`readAhead`): the reader then has it when the head moves, and is not drawn
 * again from nothing, so this section, its text box and keyboard focus stay on the page. The same
 * is done before the person is shown the revision a refusal or a reconversion names.
 */
import type { QueryClient } from '@tanstack/react-query';
import { getItemOptions } from '../../api/generated/@tanstack/react-query.gen';
import { correctDigest, redigestItem } from '../../api/generated/sdk.gen';
import type {
  ConversionOutcome,
  ConversionSettings,
  ConverterIdentity,
  Extraction,
  Job,
  OcrPolicy,
  PageRange,
  Proposal,
  ReceiptId,
  Revision,
  Warning,
} from '../../api/generated/types.gen';
import { formatInstant } from '../../lib/instant';
import { wordsFor } from '../../lib/words';
import { describePages } from '../documents/describe';
import { classifyFailure } from '../errors/failure';
import { adoptHeadFrom, checkHeadsAgain, type HeadChange } from '../explorer/head';
import type { ExportAttempt } from '../exports/exports';
import { rememberJob } from '../jobs/jobs';
import type { Attempt } from '../tree/changes';

/** The most a correction may hold, in UTF-8 bytes (`CorrectDigestRequest.corrected_markdown`). */
export const MAX_CORRECTION_BYTES = 8_388_608;

/** A correction as sent: the digest it corrects, its text, and the proposal it keeps, if any. */
export interface Correction {
  digest: string;
  text: string;
  adopts: string | null;
}

/** A correction saved: the head change it made, and what the commit answered beside it. */
export interface Corrected extends HeadChange {
  warnings: Warning[];
  receiptId: ReceiptId;
}

/** A reconversion as asked: on which head, with which settings, of the whole file or not. */
export interface Reconversion {
  base: Revision;
  settings: ConversionSettings;
  unconvertedOnly: boolean;
}

/** The item as `get_item` answers it at `revision`: the reader's own query. */
export function itemAt(workspaceId: string, itemId: string, revision: Revision) {
  return getItemOptions({
    body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
  });
}

/** Read the item at `revision` before the workspace shows it; a failed read is the reader's to say. */
async function readAhead(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  revision: Revision,
) {
  await client.prefetchQuery(itemAt(workspaceId, itemId, revision));
}

/**
 * Save a correction, on the attempt's base, under its key. A refusal that names the head the
 * request was compared with (`stale_base`) has the item read there first, so that showing that
 * version keeps the person's text on the page.
 */
export async function correct(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  attempt: Attempt<Correction>,
): Promise<Corrected> {
  const { digest, text, adopts } = attempt.values;
  const { data } = await correctDigest({
    body: {
      workspace_id: workspaceId,
      item_id: itemId,
      base_revision: attempt.base,
      digest,
      corrected_markdown: text,
      ...(adopts === null ? {} : { adopts }),
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  }).catch(async (thrown: unknown) => {
    const failure = classifyFailure(thrown);
    if (failure.kind === 'api' && failure.error.detail?.kind === 'stale_base') {
      await readAhead(client, workspaceId, itemId, failure.error.detail.head);
    }
    throw thrown;
  });
  await readAhead(client, workspaceId, itemId, data.revision);
  const { previousHead, adopted } = adoptHeadFrom(client, workspaceId, data.revision, attempt.base);
  checkHeadsAgain(client, workspaceId);
  return {
    revision: data.revision,
    previousHead,
    itemIds: [itemId],
    adopted,
    warnings: data.warnings,
    receiptId: data.receipt_id,
  };
}

/** Ask for a reconversion under the attempt's key; the job answered goes into the cache. */
export async function reconvert(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  attempt: ExportAttempt<Reconversion>,
): Promise<Job> {
  const { base, settings, unconvertedOnly } = attempt.values;
  const { data: job } = await redigestItem({
    body: {
      workspace_id: workspaceId,
      item_id: itemId,
      base_revision: base,
      settings,
      unconverted_only: unconvertedOnly,
      idempotency_key: attempt.key,
    },
    throwOnError: true,
  });
  rememberJob(client, workspaceId, job);
  return job;
}

/**
 * Show `revision`, which a reconversion made, as the workspace's head, when the workspace still
 * shows `from`, the head it was started on; the item is read there first.
 */
export async function showMade(
  client: QueryClient,
  workspaceId: string,
  itemId: string,
  revision: Revision,
  from: Revision,
): Promise<HeadChange> {
  await readAhead(client, workspaceId, itemId, revision);
  const { previousHead, adopted } = adoptHeadFrom(client, workspaceId, revision, from);
  checkHeadsAgain(client, workspaceId);
  return { revision, previousHead, itemIds: [itemId], adopted };
}

/** The text an accepted proposal supplied for this item, as it holds it; null when it holds none. */
export function suppliedTextOf(proposal: Proposal, itemId: string): string | null {
  if (proposal.kind !== 'supply_extraction') return null;
  for (const change of proposal.changes) {
    if (change.kind === 'supply_extraction' && change.item_id === itemId) return change.markdown;
  }
  return null;
}

/** The pages a conversion names as not converted; none for any other outcome. */
export function notConverted(outcome: ConversionOutcome): PageRange[] {
  return outcome.status === 'partial' ? (outcome.coverage?.not_converted ?? []) : [];
}

/** A revision shortened for a sentence; the full value goes in a title where there is one. */
export const short = (revision: Revision) => revision.slice(0, 10);

/**
 * Whose text the source shows, as the end of "the text shown is …" (SPEC section 5): the
 * converter's, a person's correction over it, or text an agent supplied (accepted by a person,
 * and kept over the converter's when a person chose it). An origin this build does not know is
 * said to be unknown, with the service's word, and never called the converter's.
 */
export function whoseText(extraction: Extraction): string {
  if (extraction.corrected) return 'a person’s correction, shown over the converter’s text';
  const origin: string = extraction.text_origin;
  if (origin === 'converter') return 'the converter’s text';
  if (origin === 'none') return 'nothing: there is no text to show';
  if (origin === 'supplied_by_agent') {
    const kept = extraction.supplied?.kept_by ?? null;
    return kept === null
      ? 'text an agent supplied and a person accepted, not the converter’s'
      : `text an agent supplied, which ${kept} kept over the converter’s text`;
  }
  return `text of an origin this page does not know (“${origin}”), not known to be the converter’s`;
}

/** When an accepted supply was accepted, in the reader's time zone. */
export function acceptedOn(approvedAt: string): string {
  return formatInstant(approvedAt) ?? approvedAt;
}

/** The pages an agent says its text covers, said as its claim. */
export function claimedPages(pages: readonly PageRange[]): string | null {
  return pages.length === 0
    ? null
    : `The agent says it covers ${describePages(pages)}: its claim, not a location.`;
}

/** Pages not converted, as a sentence. */
export function notConvertedWords(pages: readonly PageRange[]): string {
  const one = pages.length === 1 && pages[0]?.start === pages[0]?.end;
  const named = describePages(pages);
  return `${named.charAt(0).toUpperCase()}${named.slice(1)} ${one ? 'was' : 'were'} not converted.`;
}

const OCR: Record<OcrPolicy, string> = {
  auto: 'text recognition only where a page has no text layer',
  skip: 'no text recognition',
  force_full_page: 'text recognition of every whole page',
};

/** The settings a reconversion takes: those of the source's current conversion. */
export function settingsWords(converter: ConverterIdentity): string {
  const { settings } = converter;
  const ocr = wordsFor(OCR, settings.ocr) ?? `text recognition “${settings.ocr}”`;
  const language =
    settings.ocr === 'skip'
      ? ''
      : `, in ${settings.ocr_language ?? 'the converter’s default language (en)'}`;
  const pages = settings.page_images
    ? `page images kept, at ${settings.page_image_dpi} dpi`
    : `no page images kept; pages rendered at ${settings.page_image_dpi} dpi`;
  const tables = settings.table_structure
    ? 'table structure reconstructed'
    : 'table structure not reconstructed';
  return `With the settings of its current conversion (${converter.name} ${converter.version}): ${ocr}${language}; ${pages}; ${tables}.`;
}

/** Lines of the person's text, one-based and inclusive, where it differs from the text shown. */
export interface LineSpan {
  start: number;
  end: number;
}

/** How many places are named before the rest are counted. */
const PLACES_NAMED = 10;

/** Where the person's text differs from the text shown, in words. */
export function changeWords(places: readonly LineSpan[]): string {
  if (places.length === 0) return 'Your text is the text shown: nothing is changed yet.';
  const named = places
    .slice(0, PLACES_NAMED)
    .map(({ start, end }) => (start === end ? `line ${start}` : `lines ${start}–${end}`));
  const more = places.length - named.length;
  const list = more > 0 ? `${named.join(', ')} and ${more} more` : named.join(', ');
  const count = places.length === 1 ? '1 place' : `${places.length} places`;
  return `Your text differs from the text shown in ${count}: ${list}.`;
}

/** Whether a text is longer than a correction may be. */
export function tooLong(text: string): boolean {
  return new TextEncoder().encode(text).length > MAX_CORRECTION_BYTES;
}
