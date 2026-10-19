/**
 * Reopen a saved View. The server resolves it (`resolve_view`), and what it reports is the source
 * of truth: its `charts` statuses say which chart failed and why, its `resolved_bindings` say what
 * the View cites. Nothing is rebuilt from the saved document in the browser, and a pinned View is
 * never refreshed here (`refresh_live` is false).
 *
 * Which version of the View is opened is the caller's choice (`at`): nothing here picks the
 * latest. The selector is compared by its value, so a caller may give a new object of the same
 * selector on each render without asking again. An answer is shown only for the workspace, item
 * and selector it was asked for, and only the latest ask's answer is kept.
 *
 * A refusal is the workspace's standing `FailureNotice`, kept on screen through its retry by
 * `useRetry`. The service's own error is handed to the notice as it came, except that a blank
 * message is not quoted; a fault this page found by itself (a body no `ApiError`, a View the
 * generated schema does not read) has words of its own and is never called the service being
 * unreachable. When a retry opens the View, keyboard focus goes to the View in the notice's place.
 *
 * The host's link to a cited source (`sourceLink`) is handed to the View as it is.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import type { z } from 'zod';
import { resolveView } from '../../api/generated';
import type { Client } from '../../api/generated/client';
import type { At } from '../../api/generated/types.gen';
import { zApiError, zAt, zResolveViewResponse } from '../../api/generated/zod.gen';
import { FailureNotice } from '../errors/FailureNotice';
import type { FailureMessage } from '../errors/failure';
import { useRetry } from '../errors/retry';
import type { SourceLink } from './Bindings';
import type { ViewData } from './data';
import { PresentView } from './PresentView';

export interface SavedViewProps {
  workspaceId: string;
  itemId: string;
  /** Which version of the View to open: the revision the caller pins, or `latest` if it asks for it. */
  at: At;
  /** The host's reads of the sources and datasets the View cites. */
  data: ViewData;
  /** The generated client; the application's configured client when omitted. */
  client?: Client;
  /** The host's link to a cited source at the revision read; none, and sources are named as text. */
  sourceLink?: SourceLink | undefined;
}

/** What a resolve answered, for the item it was asked about: an answer for other ids is never shown. */
type Answer =
  | { state: 'refused'; error: unknown; words: FailureMessage | undefined }
  | { state: 'opened'; response: z.infer<typeof zResolveViewResponse> };
type Reopened = Answer & { key: string };

const retry = { kind: 'retry', label: 'Try again' } as const;
/** A refusal that is not an error the service describes. */
const refusedWords: FailureMessage = {
  title: 'The server refused to open the View.',
  advice: 'Try again. If it refuses again, this View cannot be opened here.',
  action: retry,
};
/** An answer the generated schema does not read. */
const unreadableWords: FailureMessage = {
  title: 'The server returned a View this client cannot read.',
  advice: 'Try again. If it happens again, this page and the server may be of different versions.',
  action: retry,
};

/** The refusal a resolve answered: the service's error as it came, or this page's words for it. */
function refusal(error: unknown): Answer {
  const described = zApiError.safeParse(error);
  if (!described.success) return { state: 'refused', error, words: refusedWords };
  const blank = described.data.message.trim() === '';
  return {
    state: 'refused',
    error: blank ? { ...described.data, message: '' } : error,
    words: undefined,
  };
}

export function SavedView({ workspaceId, itemId, at, data, client, sourceLink }: SavedViewProps) {
  const [reopened, setReopened] = useState<Reopened | null>(null);
  const selector = JSON.stringify(at);
  const key = JSON.stringify([workspaceId, itemId, selector]);
  // The latest ask: an answer to an earlier one, for these ids or others, is dropped.
  const latest = useRef(0);
  const place = useRef<HTMLDivElement | null>(null);
  const ask = useCallback(async () => {
    latest.current += 1;
    const asked = latest.current;
    let answer: Answer;
    try {
      const result = await resolveView({
        ...(client ? { client } : {}),
        body: {
          workspace_id: workspaceId,
          item_id: itemId,
          // The caller's selector, by value: `selector` is its own JSON.
          at: zAt.parse(JSON.parse(selector)),
          refresh_live: false,
        },
      });
      if (result.error !== undefined) answer = refusal(result.error);
      else {
        const parsed = zResolveViewResponse.safeParse(result.data);
        answer = parsed.success
          ? { state: 'opened', response: parsed.data }
          : { state: 'refused', error: parsed.error, words: unreadableWords };
      }
    } catch (cause) {
      answer = { state: 'refused', error: cause, words: undefined };
    }
    if (latest.current === asked) setReopened({ ...answer, key });
  }, [workspaceId, itemId, selector, client, key]);
  useEffect(() => {
    void ask();
  }, [ask]);
  const shown = reopened !== null && reopened.key === key ? reopened : null;
  const failure = useRetry({
    request: key,
    failed: shown?.state === 'refused',
    error: shown?.state === 'refused' ? shown.error : undefined,
    run: ask,
    loaded: () => place.current,
  });
  if (shown === null) return <p role="status">Reopening the saved View…</p>;
  if (shown.state === 'refused' || failure.shown)
    return (
      <FailureNotice
        context="Reopening this View"
        error={failure.error}
        retry={failure}
        workspaceId={workspaceId}
        words={shown.state === 'refused' ? shown.words : undefined}
      />
    );
  return (
    // Focusable from script only: where keyboard focus goes when a retry opened the View.
    <div ref={place} tabIndex={-1}>
      <PresentView response={shown.response} data={data} sourceLink={sourceLink} />
    </div>
  );
}
