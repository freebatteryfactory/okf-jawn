/**
 * How the conversion of a source stands, in the reader: always in words, for every state the
 * generated types name (SPEC section 5: the item shows that state accurately), and for a state
 * they do not name (a newer service): that one is said to be unknown, with the service's word,
 * and is not called converted.
 *
 * - pending: the original is kept and its text is not ready; it is not an empty document;
 * - failed: with the reason the wire gives (`FailureReason`) and what the converter recorded;
 * - unsupported: the bytes are kept and nothing was extracted. It is said to be neither an empty
 *   document nor a converted one;
 * - partial: with the pages that were not converted, when the wire names them;
 * - completed: "Converted."
 *
 * The full outcome comes from `get_item` (`source.extraction.outcome`). Where only the listing's
 * verdict is known (`ExtractionSummary.status`), its plain sentence is shown without the detail.
 *
 * Trying a failed conversion again: the wire's operation for one file is `redigest_item`, which
 * belongs with correcting a conversion and is not offered here. A failed import job has its retry
 * on the Jobs page, and the notice says so.
 */
import { Link } from '@tanstack/react-router';
import type { z } from 'zod';
import type { zConversionOutcome, zExtractionStatus } from '../../api/generated/zod.gen';
import { keyed } from '../../lib/keys';
import { describeOutcome, extractionSentence } from './describe';

type ConversionOutcome = z.infer<typeof zConversionOutcome>;
type ExtractionStatus = z.infer<typeof zExtractionStatus>;

export interface ConversionStateProps {
  workspaceId: string;
  /** Everything the converter recorded, when `get_item` has answered with it. */
  outcome: ConversionOutcome | null;
  /** The verdict alone, from a listing or a reading. */
  status: ExtractionStatus | null;
}

/** What a state means for the person, beyond its name. */
const MEANS: Partial<Record<ExtractionStatus, string>> = {
  pending: 'Its text is not ready yet. This is not an empty document.',
  unsupported:
    'There is no extracted text. This is not an empty document, and it has not been converted.',
};

export function ConversionState({ workspaceId, outcome, status }: ConversionStateProps) {
  const state = outcome?.status ?? status;
  if (state === null) return null;
  const sentence = outcome === null ? extractionSentence(state) : describeOutcome(outcome);
  const issues =
    outcome?.status === 'failed' || outcome?.status === 'partial' ? outcome.issues : [];
  return (
    <div role="note" className="notice conversion-state">
      <p>
        <strong>Conversion:</strong> {sentence}
        {MEANS[state] === undefined ? null : ` ${MEANS[state]}`}
      </p>
      {issues.length > 0 && (
        <ul>
          {keyed(issues, (issue) => JSON.stringify(issue)).map(({ key, value: issue }) => (
            <li key={key}>
              The converter recorded: <q>{issue.error_message}</q> ({issue.module_name})
            </li>
          ))}
        </ul>
      )}
      {state === 'failed' && (
        <p>
          If the import that brought this file in failed, it can be tried again under{' '}
          <Link to="/w/$workspaceId/jobs" params={{ workspaceId }}>
            Jobs
          </Link>
          .
        </p>
      )}
    </div>
  );
}
