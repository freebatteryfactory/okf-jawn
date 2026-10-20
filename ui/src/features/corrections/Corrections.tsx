/**
 * A Source's corrections (UI M4a, ruling A4), in the reader after its text and its original: whose
 * text is shown, Correct text, Keep the supplied text and Reconvert.
 *
 * Whose text is shown is said first, as of the version it is (SPEC section 5: generated
 * extraction and human corrections remain distinguishable, and supplied text is labelled as
 * supplied by that agent): the converter's text, a person's correction over it, or text an agent
 * supplied and a person accepted, with who supplied it, through which client, who accepted it and
 * when, the pages the agent says it covers (its claim, never a location), who kept it over the
 * converter's text, and the proposal it came from. An agent's text that a later conversion
 * replaced is said to be kept in the history. The same words describe the correction's text box,
 * so its starting text is never taken for another's.
 *
 * Where the actions are taken from: the item as `get_item` answered it at the revision the reader
 * shows (`document`), its source's extraction (`SourceAppearance.extraction`), and the head the
 * workspace shows, the base of every change. A change is made only on the version the workspace
 * shows: on a tab that reads another revision the actions stay on the page, `aria-disabled`, and
 * say why (the reader offers to open the version the workspace shows).
 *
 * Who may (`api/operations.json`): `correct_digest` and `redigest_item` need `write`, and neither
 * runs in an archived workspace. A reader without `write` is told why each is not offered
 * (`Missing`); in an archived workspace each stays, `aria-disabled`, described by the archive's
 * words, and sends nothing. The service decides either way, and its refusal is said in words.
 */
import { Link } from '@tanstack/react-router';
import { useId } from 'react';
import type { Extraction, ItemDocument, Revision } from '../../api/generated/types.gen';
import { allowed, archivedWords, Missing } from '../explorer/rights';
import { useWorkspaceChanges } from '../tree/context';
import { CorrectText } from './CorrectText';
import { acceptedOn, claimedPages, short, whoseText } from './corrections';
import { KeepSupplied } from './KeepSupplied';
import { Reconvert } from './Reconvert';

export interface CorrectionsProps {
  workspaceId: string;
  itemId: string;
  /** The revision the reader shows. */
  revision: Revision;
  /** The workspace's head: a correction is based on it. */
  head: Revision;
  /** The item as `get_item` answered at `revision`; a Source has `source`. */
  document: ItemDocument;
}

/** Who supplied the text, who accepted it, the pages it claims, and where it came from. */
function SuppliedBy({ workspaceId, extraction }: { workspaceId: string; extraction: Extraction }) {
  const supplied = extraction.supplied ?? null;
  if (supplied === null) return null;
  const shown = extraction.text_origin === 'supplied_by_agent' && !extraction.corrected;
  const through = supplied.client_id ? `, through ${supplied.client_id}` : '';
  const claim = claimedPages(supplied.pages);
  return (
    <p>
      {shown
        ? `${supplied.supplied_by} supplied it${through}; ${supplied.approved_by} accepted it on ${acceptedOn(supplied.approved_at)}.`
        : `Text an agent supplied, which ${supplied.approved_by} accepted on ${acceptedOn(supplied.approved_at)}, is kept in the history: ${extraction.corrected ? 'a person’s correction' : 'this conversion’s text'} is shown in its place.`}
      {claim === null ? null : ` ${claim}`}{' '}
      <Link
        to="/w/$workspaceId/proposals/$proposalId"
        params={{ workspaceId, proposalId: supplied.proposal_id }}
      >
        The proposal it came from
      </Link>
    </p>
  );
}

export function Corrections({ workspaceId, itemId, revision, head, document }: CorrectionsProps) {
  const { rights } = useWorkspaceChanges();
  const headingId = useId();
  const blockedId = useId();
  const source = document.source ?? null;
  if (source === null) return null;
  const { extraction } = source;
  const atHead = revision === head;
  const can = allowed(rights, 'write');
  const blocked = !atHead
    ? `This tab reads revision ${short(revision)}, not the version this workspace shows (revision ${short(head)}). Corrections start from the version the workspace shows.`
    : can === 'archived' && rights.archivedAt !== null
      ? archivedWords(rights.archivedAt)
      : null;
  const ready = atHead && can === 'yes';
  const blockedBy = blocked === null ? null : blockedId;
  const offersKeep = (extraction.supplied ?? null) !== null && (extraction.digest ?? null) !== null;
  return (
    <section aria-labelledby={headingId} className="corrections grid gap-2">
      <h3 id={headingId} className="text-lg font-bold">
        Corrections
      </h3>
      <p>
        As of revision <code title={revision}>{short(revision)}</code>,{' '}
        {atHead ? 'the version this workspace shows' : 'the version this tab reads'}, the text shown
        is {whoseText(extraction)}.
      </p>
      <SuppliedBy workspaceId={workspaceId} extraction={extraction} />
      {can === 'no-permission' ? (
        <>
          <Missing action="Correcting the text" permission="write" />
          {offersKeep && <Missing action="Keeping the supplied text" permission="write" />}
          <Missing action="Reconverting this file" permission="write" />
        </>
      ) : (
        <>
          {blocked !== null && (
            <p id={blockedId} className="meta">
              {blocked}
            </p>
          )}
          <CorrectText
            workspaceId={workspaceId}
            itemId={itemId}
            head={head}
            body={document.body}
            extraction={extraction}
            ready={ready}
            blockedBy={blockedBy}
          />
          <KeepSupplied
            workspaceId={workspaceId}
            itemId={itemId}
            head={head}
            extraction={extraction}
            ready={ready}
            blockedBy={blockedBy}
          />
          <Reconvert
            workspaceId={workspaceId}
            itemId={itemId}
            head={head}
            extraction={extraction}
            ready={ready}
            blockedBy={blockedBy}
          />
        </>
      )}
    </section>
  );
}
