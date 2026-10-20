/**
 * Reconvert (`redigest_item`): convert the original again with the settings of its current
 * conversion, the only settings the wire reports (`corrections.ts`), said in words before the
 * button. Where the conversion names pages it did not convert, "Convert only the pages not
 * converted" asks for those alone (`unconverted_only`), with the same settings; it is offered
 * nowhere else. A source no converter ran on has no settings to reconvert with, and the group
 * says so instead of offering anything.
 *
 * A reconversion is durable work: the service answers 202 with the job it queued, which is said
 * as one (`acceptedWords`), told to the cache (`rememberJob`: the Jobs page lists it as every
 * job), and followed here, its row as the Jobs page shows it (`JobRow`), asked again on the jobs'
 * clock until it ends (`jobOptions`). Once it reports the revision it made, "Show the new
 * conversion" shows that revision as the workspace's head, when the workspace still shows the one
 * the reconversion was started on (the item is read there first, so the section stays); the
 * reader then reads the new conversion. Otherwise the page says to refresh.
 *
 * Idempotency as for an export (`useExportStart`): a press is repeated under its key until a job
 * answers it ("Try again" after a failure that may have been carried out), and the next press after
 * an answer is a new reconversion. A refusal is said in the words of its code with the service's
 * own; a job that fails says why in its row (`not_implemented`: "Not available in this build.").
 *
 * Who may: `write`, at the version the workspace shows; in an archived workspace, or on a tab that
 * reads another revision, the buttons stay, `aria-disabled`, described by why (`blockedBy`), and
 * send nothing.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useId, useRef, useState } from 'react';
import type { Extraction, Job, Revision } from '../../api/generated/types.gen';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { acceptedWords, useExportStart } from '../exports/ExportStart';
import { JobRow } from '../jobs/JobRow';
import { jobOptions } from '../jobs/jobs';
import { useWorkspaceChanges } from '../tree/context';
import {
  notConverted,
  notConvertedWords,
  type Reconversion,
  reconvert,
  settingsWords,
  short,
  showMade,
} from './corrections';

export interface ReconvertProps {
  workspaceId: string;
  itemId: string;
  head: Revision;
  extraction: Extraction;
  ready: boolean;
  blockedBy: string | null;
}

export function Reconvert({
  workspaceId,
  itemId,
  head,
  extraction,
  ready,
  blockedBy,
}: ReconvertProps) {
  const client = useQueryClient();
  const headingId = useId();
  const settingsId = useId();
  const keptId = useId();
  const pagesId = useId();
  const wholeRef = useRef<HTMLButtonElement | null>(null);
  // The head the last press was made on: the version the job it started was asked on.
  const [startedOn, setStartedOn] = useState<Revision>(head);
  const start = useExportStart<Reconversion>({
    // Each press is followed whatever the head becomes; its values say what it asked.
    request: `${workspaceId}/${itemId}`,
    start: (attempt) => reconvert(client, workspaceId, itemId, attempt),
    loaded: () => wholeRef.current,
  });
  const converter = extraction.converter ?? null;
  const pages = notConverted(extraction.outcome);
  const canStart = ready && converter !== null && !start.pending;
  const begin = (unconvertedOnly: boolean) => {
    if (!canStart || converter === null) return;
    setStartedOn(head);
    start.begin({ base: head, settings: converter.settings, unconvertedOnly });
  };
  const describe = (...ids: (string | null)[]) => ids.filter(Boolean).join(' ');
  const said =
    start.pending && !start.failure.retrying
      ? 'Starting the reconversion…'
      : start.started === null
        ? ''
        : `${acceptedWords('The reconversion', start.started)} It is followed below, and listed on the Jobs page.`;
  return (
    <fieldset
      aria-labelledby={headingId}
      className="grid gap-2 rounded border border-[color:GrayText] px-3 py-2"
    >
      <h4 id={headingId} className="font-bold">
        Reconvert
      </h4>
      {converter === null ? (
        <p>
          Reconvert is not offered: no conversion of this file recorded the settings it was
          converted with, and the service does not say which settings a new conversion would take.
        </p>
      ) : (
        <>
          <p id={settingsId}>{settingsWords(converter)}</p>
          <p id={keptId} className="meta">
            A correction is never erased: it stays with the conversion it corrected, and is shown
            whenever that conversion is the current one. A new conversion that differs shows the
            converter’s new text.
          </p>
          {pages.length > 0 && (
            <p id={pagesId}>
              {notConvertedWords(pages)} Converting only them adds them to the pages already
              converted, in a new conversion.
            </p>
          )}
          <p className="flex flex-wrap gap-2">
            {/* aria-disabled, never disabled: the buttons keep focus while the request is out. */}
            {pages.length > 0 && (
              <button
                type="button"
                aria-disabled={!canStart}
                aria-describedby={describe(pagesId, settingsId, blockedBy)}
                onClick={() => begin(true)}
              >
                Convert only the pages not converted
              </button>
            )}
            <button
              type="button"
              ref={wholeRef}
              aria-disabled={!canStart}
              aria-describedby={describe(settingsId, keptId, blockedBy)}
              onClick={() => begin(false)}
            >
              Reconvert the whole file
            </button>
          </p>
        </>
      )}
      {/* Present from the start, so what pressing did is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
      {start.failure.shown && (
        <FailureNotice
          context="Reconverting this file"
          error={start.failure.error}
          retry={start.failure}
          workspaceId={workspaceId}
        />
      )}
      {start.started !== null && (
        <StartedReconversion
          key={start.started.id}
          workspaceId={workspaceId}
          itemId={itemId}
          accepted={start.started}
          startedOn={startedOn}
        />
      )}
    </fieldset>
  );
}

interface StartedReconversionProps {
  workspaceId: string;
  itemId: string;
  /** The job as the service accepted it. */
  accepted: Job;
  /** The head the workspace showed when it was started. */
  startedOn: Revision;
}

/** The reconversion started here, as the service now reports it, and the version it made. */
function StartedReconversion({
  workspaceId,
  itemId,
  accepted,
  startedOn,
}: StartedReconversionProps) {
  const client = useQueryClient();
  const { head, onChanged } = useWorkspaceChanges();
  const followed = useQuery(jobOptions(workspaceId, accepted.id, false));
  const listRef = useRef<HTMLUListElement | null>(null);
  const [said, setSaid] = useState('');
  const failure = useRetry({
    request: accepted.id,
    failed: followed.isError,
    error: followed.error,
    run: () => followed.refetch(),
    loaded: () => listRef.current?.querySelector<HTMLElement>('li') ?? null,
  });
  const job = followed.data ?? accepted;
  const made = job.state === 'succeeded' ? (job.revision ?? null) : null;
  const shown = made !== null && head === made;
  const show = async () => {
    if (made === null || shown) return;
    const change = await showMade(client, workspaceId, itemId, made, startedOn);
    if (change.adopted) {
      onChanged(change);
      setSaid(`The workspace now shows revision ${short(made)}, which the conversion made.`);
    } else {
      setSaid(
        `This page shows another version than the one the reconversion was started on. Refresh the workspace to see revision ${short(made)}, which it made.`,
      );
    }
  };
  return (
    <>
      <ul ref={listRef} className="job-list" aria-label="The reconversion started here">
        <JobRow workspaceId={workspaceId} job={job} />
      </ul>
      {failure.shown && (
        <FailureNotice
          context="Asking how the reconversion stands"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {made !== null && (
        <p>
          <button type="button" aria-disabled={shown} onClick={() => void show()}>
            Show the new conversion
          </button>
        </p>
      )}
      <p role="status" className="meta">
        {said}
      </p>
    </>
  );
}
