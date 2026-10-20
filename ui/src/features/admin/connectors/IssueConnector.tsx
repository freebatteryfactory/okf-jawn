/**
 * Issue a connector for this workspace (ruling C5): a name for it, and whether it may also
 * propose. It may read by default, propose only when allowed, and never verify or approve: the
 * contract grants a connector nothing more (`CreateConnectorRequest.allow_propose`), and the words
 * say so. Proposing is offered only to a person who may propose here, since a connector is given
 * only what its owner may do (`create_connector` checks the owner's grant on each workspace it
 * lists); otherwise the page says why it is not offered.
 *
 * One press is one request, under one idempotency key kept while it asks the same thing
 * (`attemptFor`): a failure's "Try again" repeats it under that key, so a request whose answer was
 * lost issues no second connector. The service then either finishes what it began and answers the
 * secret (a resumed attempt rotates the secret nobody received), or, when the first attempt had
 * completed, answers `already_issued` without the secret: that is said in words, with the
 * connector it names, and the next press is a new request under a new key.
 *
 * The secret the service answers goes into this component's state, and nowhere else
 * (`issueConnector`); the list of connectors is asked again. It is shown until the person hides
 * it (`ConnectorSecret`), and goes with the page.
 *
 * A refusal because the person is not the local owner reads the hosted words, never as an error to
 * retry; a refusal of the owner's own grant on this workspace is said in this place's words.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import type { AccessRoute, IssuedConnector } from '../../../api/generated/types.gen';
import { FailureNotice } from '../../errors/FailureNotice';
import type { FailureMessage } from '../../errors/failure';
import { useRetry } from '../../errors/retry';
import {
  allowedOperation,
  archivedWords,
  LOCAL_OWNER_WORDS,
  Missing,
  type Rights,
} from '../../explorer/rights';
import { type Attempt, attemptFor } from '../destructive';
import { ConnectorSecret } from './ConnectorSecret';
import {
  alreadyIssued,
  type IssueValues,
  isGrantRefusal,
  isHostedRefusal,
  issueConnector,
  listedAgain,
} from './connectors';

export interface IssueConnectorProps {
  workspaceId: string;
  rights: Rights;
  /** The route of the person's session, once known. */
  route: AccessRoute | undefined;
}

const GRANT_REFUSED: FailureMessage = {
  title: 'The service refused to give a connector this workspace.',
  advice:
    'A connector may be given only what you may do here yourself: reading, and proposing when you may propose.',
  action: { kind: 'none' },
};

/** The words of a replay the service answered `already_issued`, naming the connector it issued. */
function issuedBefore(connectorId: string): FailureMessage {
  const which = connectorId === '' ? 'a connector' : `connector ${connectorId.slice(0, 8)}`;
  return {
    title: 'This connector was already issued, and its secret is not shown again.',
    advice: `The service issued ${which} under this request before, and shows a secret only once. If it was lost, revoke that connector and issue a new one.`,
    action: { kind: 'none' },
  };
}

export function IssueConnector({ workspaceId, rights, route }: IssueConnectorProps) {
  const client = useQueryClient();
  const nameId = useId();
  const nameErrorId = useId();
  const aboutId = useId();
  const archivedId = useId();
  const nameRef = useRef<HTMLInputElement | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const secretRef = useRef<HTMLFieldSetElement | null>(null);
  const [label, setLabel] = useState('');
  const [allowPropose, setAllowPropose] = useState(false);
  const [nameMissing, setNameMissing] = useState(false);
  // The secret, while it is shown: this component's state, and nowhere else.
  const [issued, setIssued] = useState<IssuedConnector | null>(null);
  const last = useRef<Attempt<IssueValues> | null>(null);
  const mayPropose = rights.permissions.includes('propose');
  const values: IssueValues = {
    workspaceId,
    label: label.trim(),
    allowPropose: mayPropose && allowPropose,
  };
  const request = JSON.stringify(values);

  const issue = useMutation({
    mutationFn: (attempt: Attempt<IssueValues>) => issueConnector(attempt, setIssued),
    onSuccess: () => {
      // The service answered: the next press is another connector.
      last.current = null;
      setLabel('');
      setAllowPropose(false);
      void listedAgain(client);
    },
    onError: (error) => {
      // A replay was answered: that request is over, and the connector it names is listed.
      if (alreadyIssued(error) !== null) {
        last.current = null;
        void listedAgain(client);
      }
    },
  });
  // What was last sent is said only while it is what the form asks.
  const sent = issue.variables;
  const mine = sent !== undefined && JSON.stringify(sent.values) === request;
  const failure = useRetry({
    request,
    failed: mine && issue.isError,
    error: mine ? issue.error : null,
    run: () => (mine ? issue.mutateAsync(sent) : Promise.resolve()),
    loaded: () => buttonRef.current,
  });

  // The secret takes keyboard focus when it appears, so its words are read first.
  useEffect(() => {
    if (issued !== null) secretRef.current?.focus();
  }, [issued]);

  const can = allowedOperation(rights, 'create_connector', 'admin');
  if (can === 'no-permission') {
    return <Missing action="Issuing a connector" permission="admin" />;
  }
  const stopped = can === 'archived';
  const pending = mine && issue.isPending;
  const hosted = failure.shown && isHostedRefusal(failure.error, route);
  const replayed = failure.shown ? alreadyIssued(failure.error) : null;
  const words =
    replayed !== null
      ? issuedBefore(replayed)
      : failure.shown && isGrantRefusal(failure.error)
        ? GRANT_REFUSED
        : undefined;

  return (
    <>
      {issued !== null && (
        <ConnectorSecret
          issued={issued}
          boxRef={secretRef}
          onHide={() => {
            setIssued(null);
            buttonRef.current?.focus();
          }}
        />
      )}
      <form
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          if (stopped || pending) return;
          if (values.label === '') {
            setNameMissing(true);
            nameRef.current?.focus();
            return;
          }
          setNameMissing(false);
          const attempt = attemptFor(last.current, values);
          last.current = attempt;
          issue.mutate(attempt);
        }}
      >
        <fieldset className="grid justify-items-start gap-1 rounded border border-[color:GrayText] px-3 py-2">
          <legend className="px-1 font-bold">Issue a connector for this workspace</legend>
          <p id={aboutId} className="m-0">
            A connector may read this workspace. With proposing allowed it may also propose changes,
            for a person to review; it can never verify or approve anything.
          </p>
          <label htmlFor={nameId}>Name of the connector</label>
          <input
            id={nameId}
            ref={nameRef}
            type="text"
            value={label}
            autoComplete="off"
            aria-invalid={nameMissing}
            aria-describedby={nameMissing ? nameErrorId : undefined}
            className="w-[min(100%,30rem)]"
            onChange={(event) => {
              setLabel(event.target.value);
              if (event.target.value.trim() !== '') setNameMissing(false);
            }}
          />
          {nameMissing && (
            <p id={nameErrorId} className="field-error m-0">
              Give the connector a name, such as the app it is for.
            </p>
          )}
          {mayPropose ? (
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={allowPropose}
                aria-describedby={aboutId}
                onChange={(event) => setAllowPropose(event.target.checked)}
              />
              Also allow proposing
            </label>
          ) : (
            <Missing action="Letting a connector propose changes" permission="propose" />
          )}
          {stopped && rights.archivedAt !== null && (
            <p id={archivedId} className="meta m-0">
              {archivedWords(rights.archivedAt)}
            </p>
          )}
          {/* aria-disabled rather than disabled: the button keeps focus while the request is out. */}
          <button
            type="submit"
            ref={buttonRef}
            aria-disabled={stopped || pending}
            aria-describedby={stopped ? archivedId : undefined}
          >
            {pending && !failure.retrying ? 'Issuing the connector…' : 'Issue a connector'}
          </button>
          {hosted ? (
            <p className="m-0">{LOCAL_OWNER_WORDS}</p>
          ) : (
            failure.shown && (
              <FailureNotice
                context="Issuing a connector"
                error={failure.error}
                retry={failure}
                workspaceId={workspaceId}
                words={words}
              />
            )
          )}
        </fieldset>
      </form>
    </>
  );
}
