/**
 * The uploads the service holds for this workspace, on the import page (`held-uploads.ts`):
 * those that are uploaded and not imported, which can be included in the next import, and those
 * that were never finished. Either kind can be discarded. Uploads this page's own list holds are
 * not listed again here.
 *
 * Nothing expires, so the page says what stays: an upload is kept until it is imported or
 * discarded. An upload whose name the service did not keep (one opened before it kept names) is
 * shown by its identity. A list that cannot be read is a failure notice with its retry, never "no
 * uploads"; `not_implemented` reads "not available in this build".
 *
 * Discarding asks first, in place: "Keep the upload" takes keyboard focus, and choosing it gives
 * focus back to "Discard". The request is repeated under its key by the notice's retry. Once the
 * service has answered, the row leaves and a polite status says so; keyboard focus that was in it
 * goes to the row now in its place, else the one before, else the list's heading.
 */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import type { Upload } from '../../api/generated/types.gen';
import { type HeldFocus, takeHeld, useWatchedPart } from '../../lib/focus';
import { Instant } from '../../lib/instant';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { describeSize } from './files';
import { discardHeld, type HeldState, heldPath, heldUploadsOptions } from './held-uploads';
import { shownName } from './paths';

export interface WaitingUploadsProps {
  workspaceId: string;
  /** The uploads this page's own list holds: not listed again here. */
  own: ReadonlySet<string>;
  /** The uploaded ones the person included in the next import. */
  included: ReadonlySet<string>;
  onInclude: (uploadId: string, include: boolean) => void;
}

export function WaitingUploads({ workspaceId, own, included, onInclude }: WaitingUploadsProps) {
  const heading = useId();
  const [said, setSaid] = useState('');
  return (
    <section aria-labelledby={heading} className="held-uploads">
      <h3 id={heading} className="panel-heading">
        Uploads the service holds
      </h3>
      <p className="meta">
        The service keeps an upload until it is imported or discarded: nothing is removed by itself.
      </p>
      <HeldList
        workspaceId={workspaceId}
        state="waiting"
        own={own}
        included={included}
        onInclude={onInclude}
        onDiscarded={setSaid}
      />
      <HeldList
        workspaceId={workspaceId}
        state="incomplete"
        own={own}
        included={included}
        onInclude={onInclude}
        onDiscarded={setSaid}
      />
      {/* Present from the start, so what a discard did is announced. */}
      <p role="status" className="meta">
        {said}
      </p>
    </section>
  );
}

const TITLES: Record<HeldState, string> = {
  waiting: 'Uploaded and not imported',
  incomplete: 'Not finished',
};

const LISTING: Record<HeldState, string> = {
  waiting: 'Listing the uploads that are not imported',
  incomplete: 'Listing the uploads that were not finished',
};

function HeldList({
  workspaceId,
  state,
  own,
  included,
  onInclude,
  onDiscarded,
}: {
  workspaceId: string;
  state: HeldState;
  own: ReadonlySet<string>;
  included: ReadonlySet<string>;
  onInclude: (uploadId: string, include: boolean) => void;
  onDiscarded: (said: string) => void;
}) {
  const heading = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const listed = useInfiniteQuery(heldUploadsOptions(workspaceId, state));
  const uploads = (listed.data?.pages ?? [])
    .flatMap((page) => page.items)
    .filter((upload) => !own.has(upload.id));
  const failure = useRetry({
    request: `${workspaceId}/${state}`,
    failed: listed.isError,
    error: listed.error,
    run: () => (listed.isFetchNextPageError ? listed.fetchNextPage() : listed.refetch()),
    loaded: () => listRef.current?.querySelector<HTMLElement>('button') ?? headingRef.current,
  });

  // A row that left with keyboard focus in it: focus goes to the row now in its place.
  const held: HeldFocus = useRef<HTMLElement | null>(null);
  const leftAt = useRef(0);
  useLayoutEffect(() => {
    if (!takeHeld(held)) return;
    const rows = listRef.current?.querySelectorAll<HTMLElement>(':scope > li');
    const next = rows?.item(Math.min(leftAt.current, (rows?.length ?? 0) - 1));
    (next?.querySelector<HTMLElement>('input, button') ?? headingRef.current)?.focus();
  });

  return (
    <div className="held-list">
      <h4 id={heading} ref={headingRef} tabIndex={-1}>
        {TITLES[state]}
        {listed.data === undefined
          ? ''
          : ` (${uploads.length}${listed.hasNextPage ? ', and more' : ''})`}
      </h4>
      {listed.isPending && !failure.retrying && <p role="status">Listing them…</p>}
      {listed.isSuccess && uploads.length === 0 && <p className="meta">None.</p>}
      {uploads.length > 0 && (
        <ul ref={listRef} className="upload-list" aria-labelledby={heading}>
          {uploads.map((upload, index) => (
            <HeldRow
              key={upload.id}
              workspaceId={workspaceId}
              upload={upload}
              state={state}
              included={included.has(upload.id)}
              onInclude={(include) => onInclude(upload.id, include)}
              held={held}
              onDiscarded={(name) => {
                leftAt.current = index;
                onInclude(upload.id, false);
                onDiscarded(`The upload of ${name} was discarded: the service no longer holds it.`);
              }}
            />
          ))}
        </ul>
      )}
      {failure.shown && (
        <FailureNotice
          context={LISTING[state]}
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {listed.hasNextPage && !listed.isError && (
        // aria-disabled rather than disabled: the button keeps focus while the page is asked.
        <button
          type="button"
          ref={moreRef}
          aria-disabled={listed.isFetchingNextPage}
          onClick={() => {
            if (!listed.isFetchingNextPage) void listed.fetchNextPage();
          }}
        >
          {listed.isFetchingNextPage ? 'Listing more…' : 'Show more uploads'}
        </button>
      )}
    </div>
  );
}

function HeldRow({
  workspaceId,
  upload,
  state,
  included,
  onInclude,
  held,
  onDiscarded,
}: {
  workspaceId: string;
  upload: Upload;
  state: HeldState;
  included: boolean;
  onInclude: (include: boolean) => void;
  held: HeldFocus;
  onDiscarded: (name: string) => void;
}) {
  const client = useQueryClient();
  const [, rowRef] = useWatchedPart<HTMLLIElement>(held);
  const [asking, setAsking] = useState(false);
  const [gone, setGone] = useState(false);
  const discardRef = useRef<HTMLButtonElement | null>(null);
  const keepRef = useRef<HTMLButtonElement | null>(null);
  const focusNext = useRef<'keep' | 'discard' | null>(null);
  useLayoutEffect(() => {
    const next = focusNext.current;
    focusNext.current = null;
    if (next === 'keep') keepRef.current?.focus();
    else if (next === 'discard') discardRef.current?.focus();
  });
  const path = heldPath(upload);
  const name = path === null ? `the upload ${upload.id.slice(0, 8)}` : `“${shownName(path)}”`;
  // One discard of this upload: repeated under its key.
  const key = useRef(crypto.randomUUID());
  const discard = useMutation({
    mutationFn: () => discardHeld(client, workspaceId, upload.id, key.current),
    onSuccess: () => {
      setGone(true);
      onDiscarded(name);
    },
  });
  const failure = useRetry({
    request: upload.id,
    failed: discard.isError,
    error: discard.error,
    run: () => discard.mutateAsync(),
    loaded: () => discardRef.current,
  });
  if (gone) return null;
  const size = upload.size === undefined || upload.size === null ? null : Number(upload.size);
  const received = Number(upload.received_bytes);
  return (
    <li ref={rowRef} className="upload-row">
      {state === 'waiting' && (
        <>
          <input
            type="checkbox"
            checked={included}
            aria-label={`Include ${name} in the import`}
            onChange={(event) => onInclude(event.target.checked)}
          />{' '}
        </>
      )}
      {/* Isolated, so a name written right to left does not reorder the words beside it. */}
      <bdi className="upload-path">
        {path === null
          ? `Upload ${upload.id.slice(0, 8)} (its name was not kept)`
          : shownName(path)}
      </bdi>{' '}
      <span className="meta">
        {state === 'waiting'
          ? size === null
            ? describeSize(received)
            : describeSize(size)
          : size === null
            ? `${describeSize(received)} sent`
            : `${describeSize(received)} of ${describeSize(size)} sent`}
        {upload.created_at ? (
          <>
            , opened <Instant value={upload.created_at} />
          </>
        ) : null}
      </span>{' '}
      {asking ? (
        <fieldset className="confirm">
          <legend>Discard {name}?</legend>
          <p>Its bytes are removed from the service. The file on your computer stays as it is.</p>
          <button
            type="button"
            ref={keepRef}
            onClick={() => {
              focusNext.current = 'discard';
              setAsking(false);
            }}
          >
            Keep the upload
          </button>{' '}
          {/* aria-disabled rather than disabled: the button keeps focus while it works. */}
          <button
            type="button"
            aria-disabled={discard.isPending}
            onClick={() => {
              if (!discard.isPending) discard.mutate();
            }}
          >
            {discard.isPending ? 'Discarding…' : 'Discard it'}
          </button>
        </fieldset>
      ) : (
        <button
          type="button"
          ref={discardRef}
          aria-label={`Discard ${name}`}
          onClick={() => {
            focusNext.current = 'keep';
            setAsking(true);
          }}
        >
          Discard
        </button>
      )}
      {failure.shown && (
        <FailureNotice
          context={`Discarding ${name}`}
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
    </li>
  );
}
