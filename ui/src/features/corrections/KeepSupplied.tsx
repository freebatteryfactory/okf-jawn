/**
 * Keep the supplied text (`correct_digest` with `adopts`; SPEC section 5: "A later complete
 * conversion is shown instead unless a person keeps the supplied text, which stays in history").
 *
 * Offered where an agent's text was accepted for this source (`Extraction.supplied`) and a
 * conversion has a digest to keep it over, unless a person has kept it already
 * (`supplied.kept_by` while it is the text shown). What is kept is the proposal's own text for
 * this item (`get_proposal`, its `supply_extraction` change), sent as `corrected_markdown` with
 * the proposal as `adopts`: the service holds the two to the same SHA-256 digest. The button waits
 * for that text (`aria-disabled` until it is read); a proposal that holds no text for this item
 * is said, and nothing is offered. Where the supplied text is not the one shown, it can be read
 * here before it is kept.
 *
 * The request is a correction's (`corrections.ts` `correct`): on the head shown, repeated under
 * its key while it is tried again, a new key after an answer. Once kept, the workspace shows the
 * revision it made, and keyboard focus goes to the words that say so. A refusal is said in words
 * with the service's own: a `conflict` on `/adopts` (the proposal is not accepted, supplies no
 * text for this item, or holds another text) as the text that could not be kept, with no retry,
 * since the same request would be refused again.
 *
 * Who may, as for Correct text: `write`, at the version the workspace shows; otherwise the button
 * stays, `aria-disabled`, described by why (`blockedBy`).
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import type { Extraction, Revision } from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { SafeMarkdown } from '../documents/SafeMarkdown';
import { FailureNotice } from '../errors/FailureNotice';
import { classifyFailure, type FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import { proposalOptions } from '../proposals/proposals';
import { Committed } from '../tree/Committed';
import { type Attempt, attemptFor } from '../tree/changes';
import { useWorkspaceChanges } from '../tree/context';
import { type Corrected, type Correction, correct, short, suppliedTextOf } from './corrections';

export interface KeepSuppliedProps {
  workspaceId: string;
  itemId: string;
  head: Revision;
  extraction: Extraction;
  ready: boolean;
  blockedBy: string | null;
}

const NOT_KEPT: FailureMessage = {
  title: 'The supplied text could not be kept.',
  advice:
    'The service refused it: the proposal it came from is not accepted, or holds no text or other text for this file. Nothing was changed.',
  action: { kind: 'none' },
};

/** A `conflict` that is not a moved base, said as the text that could not be kept. */
function keepWords(error: unknown): FailureMessage | undefined {
  const failure = classifyFailure(error);
  return failure.kind === 'api' &&
    failure.error.code === 'conflict' &&
    (failure.error.detail ?? null) === null
    ? NOT_KEPT
    : undefined;
}

export function KeepSupplied({
  workspaceId,
  itemId,
  head,
  extraction,
  ready,
  blockedBy,
}: KeepSuppliedProps) {
  const client = useQueryClient();
  const { onChanged } = useWorkspaceChanges();
  const headingId = useId();
  const aboutId = useId();
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const doneRef = useRef<HTMLDivElement | null>(null);
  const [done, setDone] = useState<Corrected | null>(null);
  const last = useRef<Attempt<Correction> | null>(null);
  const supplied = extraction.supplied ?? null;
  const digest = extraction.digest ?? null;
  const shownIsSupplied = extraction.text_origin === 'supplied_by_agent' && !extraction.corrected;
  const kept = shownIsSupplied && (supplied?.kept_by ?? null) !== null;
  const offered = supplied !== null && digest !== null && !kept;

  const proposal = useQuery({
    ...proposalOptions(workspaceId, supplied?.proposal_id ?? ''),
    enabled: offered,
  });
  const markdown =
    proposal.data === undefined || supplied === null ? null : suppliedTextOf(proposal.data, itemId);
  const reading = useRetry({
    request: `${workspaceId}/${supplied?.proposal_id ?? ''}`,
    failed: offered && proposal.isError,
    error: proposal.error,
    run: () => proposal.refetch(),
    loaded: () => keepRef.current,
  });

  const send = useMutation({
    mutationFn: (attempt: Attempt<Correction>) => correct(client, workspaceId, itemId, attempt),
    onSuccess: (corrected) => {
      last.current = null;
      setDone(corrected);
      if (corrected.adopted) onChanged(corrected);
    },
  });
  const keep = () => {
    if (digest === null || markdown === null || supplied === null) return Promise.resolve();
    const attempt = attemptFor(last.current, head, {
      digest,
      text: markdown,
      adopts: supplied.proposal_id,
    });
    last.current = attempt;
    return send.mutateAsync(attempt);
  };
  const failure = useRetry({
    request: head,
    failed: send.isError && send.variables?.base === head,
    error: send.error,
    run: keep,
    loaded: () => keepRef.current ?? doneRef.current,
  });

  // The offer leaves once the text is kept: focus that was in it goes to the words saying so.
  const heldOffer = useRef<HTMLElement | null>(null);
  const [, offerRef] = useWatchedPart<HTMLFieldSetElement>(heldOffer);
  useLayoutEffect(() => {
    if (takeHeld(heldOffer)) (doneRef.current ?? pageHeading())?.focus();
  });

  const canKeep = ready && markdown !== null && !send.isPending;
  return (
    <>
      {offered && (
        <fieldset
          key="offer"
          ref={offerRef}
          aria-labelledby={headingId}
          className="grid gap-2 rounded border border-[color:GrayText] px-3 py-2"
        >
          <h4 id={headingId} className="font-bold">
            The supplied text
          </h4>
          <p id={aboutId}>
            {shownIsSupplied
              ? 'It is shown because an agent supplied it and a person accepted it; no person has kept it over this conversion yet. Keeping it records your choice of it over this conversion’s text: a conversion that comes out the same keeps showing it, and one that differs is shown in its place until it is kept again.'
              : 'It stays in the history, and this conversion’s text is shown in its place. Keeping it shows it again, as your choice, over this conversion’s text; the conversion stays as the converter made it.'}
          </p>
          {!shownIsSupplied && markdown !== null && (
            <details>
              <summary>Read the supplied text</summary>
              <SafeMarkdown markdown={markdown} headingLevel={5} />
            </details>
          )}
          {proposal.isPending && !reading.retrying && (
            <p role="status">Reading the supplied text…</p>
          )}
          {reading.shown && (
            <FailureNotice
              context="Reading the supplied text"
              error={reading.error}
              retry={reading}
              workspaceId={workspaceId}
            />
          )}
          {proposal.data !== undefined && markdown === null ? (
            <p>
              The proposal this text came from holds no text for this file: it cannot be kept from
              here.
            </p>
          ) : (
            <p>
              {/* aria-disabled, never disabled: the button keeps focus while it works. */}
              <button
                type="button"
                ref={keepRef}
                aria-disabled={!canKeep}
                aria-describedby={[aboutId, blockedBy].filter(Boolean).join(' ')}
                onClick={() => {
                  if (canKeep) void keep().catch(() => undefined);
                }}
              >
                Keep the supplied text
              </button>
            </p>
          )}
          <p role="status" className="sr-only">
            {send.isPending && !failure.retrying ? 'Keeping the supplied text…' : ''}
          </p>
          {failure.shown && (
            <FailureNotice
              context="Keeping the supplied text"
              error={failure.error}
              retry={failure}
              workspaceId={workspaceId}
              words={keepWords(failure.error)}
            />
          )}
        </fieldset>
      )}
      {done !== null && (
        <div key="done" ref={doneRef} tabIndex={-1}>
          <p role="status">
            {done.adopted
              ? `Kept: the supplied text is shown in place of this conversion’s text, in revision ${short(done.revision)}.`
              : `Kept, in revision ${short(done.revision)}. This page still shows another version: refresh the workspace to see it.`}
          </p>
          <Committed
            workspaceId={workspaceId}
            warnings={done.warnings}
            receiptId={done.receiptId}
          />
        </div>
      )}
    </>
  );
}
