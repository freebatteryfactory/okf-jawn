/**
 * Reopen a saved View. The server resolves it (`resolve_view`), and what it reports is the source
 * of truth: its `charts` statuses say which chart failed and why, its `resolved_bindings` say what
 * the View cites. Nothing is rebuilt from the saved document in the browser, and a pinned View is
 * never refreshed here (`refresh_live` is false).
 */
import { useEffect, useState } from 'react';
import type { z } from 'zod';
import { resolveView } from '../../api/generated';
import type { Client } from '../../api/generated/client';
import { zResolveViewResponse } from '../../api/generated/zod.gen';
import { PresentView, type PresentViewProps } from './PresentView';

export interface SavedViewProps {
  workspaceId: string;
  itemId: string;
  /** Host tool call for the sources and datasets the View cites. */
  callTool: PresentViewProps['callTool'];
  /** The generated client; the application's configured client when omitted. */
  client?: Client;
}

/** What a resolve answered, for the item it was asked about: an answer for other ids is never shown. */
type Answer =
  | { state: 'refused'; message: string }
  | { state: 'opened'; response: z.infer<typeof zResolveViewResponse> };
type Reopened = Answer & { key: string };
const noMessage = 'The server refused to open the View.';

export function SavedView({ workspaceId, itemId, callTool, client }: SavedViewProps) {
  const [reopened, setReopened] = useState<Reopened | null>(null);
  const key = JSON.stringify([workspaceId, itemId]);
  useEffect(() => {
    let active = true;
    const settle = (next: Answer) => {
      if (active) setReopened({ ...next, key });
    };
    resolveView({
      ...(client ? { client } : {}),
      body: {
        workspace_id: workspaceId,
        item_id: itemId,
        at: { kind: 'latest' },
        refresh_live: false,
      },
    })
      .then((result) => {
        if (result.error !== undefined) {
          const said: unknown = result.error.message;
          settle({
            state: 'refused',
            message: typeof said === 'string' && said.trim() !== '' ? said : noMessage,
          });
          return;
        }
        const parsed = zResolveViewResponse.safeParse(result.data);
        settle(
          parsed.success
            ? { state: 'opened', response: parsed.data }
            : { state: 'refused', message: 'The server returned a View this client cannot read.' },
        );
      })
      .catch((cause) =>
        settle({
          state: 'refused',
          message: cause instanceof Error ? cause.message : 'The View could not be reopened.',
        }),
      );
    return () => {
      active = false;
    };
  }, [workspaceId, itemId, client, key]);
  if (reopened === null || reopened.key !== key)
    return <p role="status">Reopening the saved View…</p>;
  if (reopened.state === 'refused') return <p role="alert">{reopened.message}</p>;
  return <PresentView response={reopened.response} callTool={callTool} />;
}
