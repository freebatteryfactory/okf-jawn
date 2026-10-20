/**
 * Propose a change to an item, from its page: a new text for a Note, with a title and the reason,
 * proposed on the head the workspace shows (`open_proposal` with one `edit` change). The item's
 * properties go with it as `get_item` answered them, its header included (the service takes an
 * echoed header, and refuses a changed one). A proposal changes nothing until someone who may
 * approve it accepts it, and the form says so.
 *
 * It is offered to a person who may propose (SPEC section 8: "An explicitly allowed drafter may
 * create proposals"); in an archived workspace the button to send stays, says why it cannot run,
 * and sends nothing. A proposal is repeated under its key while it says the same thing. The form
 * opens in place with keyboard focus in its first field; Cancel and Escape give focus back to the
 * button that opened it; when the proposal is made, focus goes to the words that say so, with a
 * link to the proposal.
 *
 * An assistant in this browser may fill the form for the person through the page tool
 * `prepare_proposal` (`webmcp/proposal-form.ts`), while WebMCP is on and the person may propose.
 * The form opens with the assistant's title, reason and text and says who filled it; nothing is
 * sent until the person presses Propose. It never writes over words the person is writing. Being
 * filled does not move keyboard focus, which stays where the person has it, unless it was on the
 * words of a proposal already made, which the form replaces: then it goes to the form's title. A
 * polite status says that the form was filled.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { type KeyboardEvent, useId, useLayoutEffect, useRef, useState } from 'react';
import type { ItemDocument, Proposal } from '../../api/generated/types.gen';
import { pageHeading, takeHeld, useWatchedPart } from '../../lib/focus';
import { type PreparedProposal, useFillableProposalForm } from '../../webmcp/proposal-form';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowed, archivedWords } from '../explorer/rights';
import { type Attempt, attemptFor } from '../tree/changes';
import { useWorkspaceChanges } from '../tree/context';
import { type ProposedEdit, proposeEdit } from './proposals';

export interface ProposeChangeProps {
  workspaceId: string;
  document: ItemDocument;
}

/** What an assistant filled the form with. */
interface Filled {
  title: string;
  why: string;
  text: string;
}

export function ProposeChange({ workspaceId, document }: ProposeChangeProps) {
  const client = useQueryClient();
  const { head, rights } = useWorkspaceChanges();
  const archivedId = useId();
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState('');
  const [why, setWhy] = useState('');
  const [text, setText] = useState(document.body);
  const [untitled, setUntitled] = useState(false);
  const [made, setMade] = useState<Proposal | null>(null);
  // What an assistant filled the form with (null when the person opened it), and the words that
  // say it was filled.
  const [filledWith, setFilledWith] = useState<Filled | null>(null);
  const [filledSaid, setFilledSaid] = useState('');
  const openerRef = useRef<HTMLButtonElement | null>(null);
  const titleRef = useRef<HTMLInputElement | null>(null);
  const madeRef = useRef<HTMLParagraphElement | null>(null);
  const last = useRef<Attempt<ProposedEdit> | null>(null);
  const send = useMutation({
    mutationFn: (attempt: Attempt<ProposedEdit>) => proposeEdit(client, workspaceId, attempt),
    onSuccess: (proposal) => {
      setMade(proposal);
      setFilledWith(null);
      setOpen(false);
    },
  });
  const run = () => {
    const attempt = attemptFor(last.current, head, {
      itemId: document.summary.id,
      title: title.trim(),
      description: why.trim(),
      body: text,
      properties: document.properties,
    });
    last.current = attempt;
    return send.mutateAsync(attempt);
  };
  const failure = useRetry({
    request: document.summary.id,
    failed: send.isError,
    error: send.error,
    run,
    loaded: () => madeRef.current ?? titleRef.current,
  });

  // The person's opening of the form puts focus in its first field; an assistant's filling of it
  // does not, unless focus was on what the form replaced.
  const focusOnOpen = useRef(false);
  useLayoutEffect(() => {
    if (!open || !focusOnOpen.current) return;
    focusOnOpen.current = false;
    titleRef.current?.focus();
  }, [open]);
  const can = allowed(rights, 'propose');
  const fill = (prepared: PreparedProposal) => {
    const values = { title: prepared.title, why: prepared.description, text: prepared.body };
    // The words of a proposal already made leave in place of the form.
    if (made !== null && madeRef.current?.contains(globalThis.document.activeElement)) {
      focusOnOpen.current = true;
    }
    setTitle(values.title);
    setWhy(values.why);
    setText(values.text);
    setUntitled(false);
    setMade(null);
    setFilledWith(values);
    setOpen(true);
    setFilledSaid(
      `An assistant in this browser filled in the form to propose a change to “${document.summary.title}”. Nothing has been sent.`,
    );
  };
  // The form an assistant may fill: only on the head shown, never over the person's own words.
  useFillableProposalForm(workspaceId, document.summary.id, can === 'yes', (prepared) => {
    if (prepared.base !== head) return 'stale';
    const untouched = filledWith ?? { title: '', why: '', text: document.body };
    if (open && (title !== untouched.title || why !== untouched.why || text !== untouched.text)) {
      return 'busy';
    }
    fill(prepared);
    return 'filled';
  });
  // The form leaves when it is left or done: focus that was in it goes to the words that say a
  // proposal was made, else back to the button that opened it.
  const heldForm = useRef<HTMLElement | null>(null);
  const [, formRef] = useWatchedPart<HTMLDivElement>(heldForm);
  useLayoutEffect(() => {
    if (takeHeld(heldForm)) (madeRef.current ?? openerRef.current ?? pageHeading())?.focus();
  });
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    setOpen(false);
  };
  if (can === 'no-permission') return null;
  const ready = can === 'yes' && !send.isPending;

  return (
    <div className="propose-change">
      <p className="document-actions">
        <button
          type="button"
          ref={openerRef}
          aria-expanded={open}
          onClick={() => {
            if (!open) {
              setMade(null);
              setFilledSaid('');
              focusOnOpen.current = true;
            }
            setOpen((now) => !now);
          }}
        >
          Propose a change…
        </button>
      </p>
      <p role="status" className="sr-only">
        {filledSaid}
      </p>
      {made !== null && !open && (
        <p ref={madeRef} tabIndex={-1} role="status">
          Proposed: “{made.title}”. It changes nothing until someone who may approve it accepts it.{' '}
          <Link
            to="/w/$workspaceId/proposals/$proposalId"
            params={{ workspaceId, proposalId: made.id }}
          >
            Open the proposal
          </Link>
        </p>
      )}
      {open && (
        <div ref={formRef} className="change-form">
          <p className="meta">
            A proposal changes nothing until someone who may approve it accepts it. It is made on
            the version this workspace shows.
          </p>
          {filledWith !== null && (
            <p className="notice">
              An assistant in this browser filled in this form. Nothing has been sent: read it,
              change what you like, then press Propose to send it, or Cancel.
            </p>
          )}
          <p className="change-field">
            <label>
              Title{' '}
              <input
                ref={titleRef}
                type="text"
                value={title}
                aria-invalid={untitled}
                onKeyDown={onKeyDown}
                onChange={(event) => {
                  setTitle(event.target.value);
                  setUntitled(false);
                }}
              />
            </label>
          </p>
          {untitled && <p className="field-error">Give the proposal a title first.</p>}
          <p className="change-field">
            <label>
              Why{' '}
              <textarea
                rows={2}
                value={why}
                onKeyDown={onKeyDown}
                onChange={(event) => setWhy(event.target.value)}
              />
            </label>
          </p>
          <p className="change-field">
            <label>
              The text you propose{' '}
              <textarea
                rows={10}
                value={text}
                onKeyDown={onKeyDown}
                onChange={(event) => setText(event.target.value)}
              />
            </label>
          </p>
          <p className="change-buttons">
            <button
              type="button"
              aria-disabled={!ready}
              {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
              onClick={() => {
                if (!ready) return;
                if (title.trim().length === 0) {
                  setUntitled(true);
                  titleRef.current?.focus();
                  return;
                }
                void run().catch(() => undefined);
              }}
            >
              {send.isPending ? 'Proposing…' : 'Propose'}
            </button>{' '}
            <button type="button" onClick={() => setOpen(false)}>
              Cancel
            </button>
          </p>
          {can === 'archived' && rights.archivedAt !== null && (
            <p id={archivedId} className="meta">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          {failure.shown && (
            <FailureNotice
              context="Proposing this change"
              error={failure.error}
              retry={failure}
              workspaceId={workspaceId}
            />
          )}
        </div>
      )}
    </div>
  );
}
