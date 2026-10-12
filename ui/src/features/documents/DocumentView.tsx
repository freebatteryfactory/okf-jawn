/**
 * Read one item: its identity and properties from `get_item`, its text from `read_item` as
 * sanitized Markdown, at one resolved revision and the selection the URL names. A revision is
 * called "the latest" only when a request made for that statement resolved it.
 *
 * A partial read says so and offers its continuation, which resumes the same resolved read;
 * continuing moves keyboard focus to the part it loaded, and a live region says what happened.
 * Warnings, partial extraction and agent-supplied text are shown, never smoothed over. Each
 * operation's failure is shown where its content would have been.
 */
import {
  type UseMutationResult,
  type UseQueryResult,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query';
import { useEffect, useId, useRef } from 'react';
import {
  getItemOptions,
  openWorkspaceQueryKey,
  readItemInfiniteOptions,
} from '../../api/generated/@tanstack/react-query.gen';
import { openWorkspace, readItem } from '../../api/generated/sdk.gen';
import type {
  ReadItemRequest,
  ReadItemResponse,
  Revision,
  Workspace,
} from '../../api/generated/types.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { FailureNotice } from '../errors/FailureNotice';
import type { ItemSelection } from '../explorer/search-params';
import { describeSelection, extractionLabels, short } from './describe';
import { Properties } from './Properties';
import { SafeMarkdown } from './SafeMarkdown';

/** Text budget per read; a longer item is read in parts through the continuation. */
export const READ_BUDGET_BYTES = 65_536;

export interface DocumentViewProps {
  workspaceId: string;
  itemId: string;
  /** The revision to read at: the shell's resolved head, or the exact revision a link names. */
  revision: Revision;
  /** A link names this revision, so the reader says whether it is still the latest. */
  pinned: boolean;
  selection: ItemSelection | undefined;
  /** Show another selection of this item (or the whole item). */
  onSelect: (selection: ItemSelection | undefined) => void;
  /** The latest version was read; `resolved` is the revision the server resolved for it. */
  onReadLatest: (resolved: Revision) => void;
}

const ROLE = { note: 'Note', source: 'Source', view: 'View' } as const;

/** The words of the reading's live region: what the last continuation did. */
function readingStatus(
  read: { isFetchingNextPage: boolean; isFetchNextPageError: boolean; hasNextPage: boolean },
  parts: number,
): string {
  if (read.isFetchingNextPage) return 'Reading more…';
  if (read.isFetchNextPageError) return 'The rest of this reading could not be read.';
  if (parts > 1) return `Part ${parts} read${read.hasNextPage ? '; more can be read' : ''}.`;
  return '';
}

export function itemOptions(workspaceId: string, itemId: string, revision: Revision) {
  return getItemOptions({
    body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
  });
}

/**
 * A pinned revision, compared with a head fetched for this comparison (never a cached one), and
 * the way to the latest version: a read with `at: latest`, which the server resolves once. Its
 * answer is kept as the first part of a read pinned to the revision it resolved, so the pane,
 * the URL and the continuation all follow that one read.
 */
function PinnedRevision({
  workspaceId,
  revision,
  request,
  onReadLatest,
}: {
  workspaceId: string;
  revision: Revision;
  request: ReadItemRequest;
  onReadLatest: (resolved: Revision) => void;
}) {
  const client = useQueryClient();
  const noticeRef = useRef<HTMLDivElement | null>(null);
  const body = { workspace_id: workspaceId };
  const latestHead = useQuery({
    queryKey: [{ ...openWorkspaceQueryKey({ body })[0], _comparedWith: revision }],
    queryFn: async ({ signal }) => {
      const { data } = await openWorkspace({ body, signal, throwOnError: true });
      return data;
    },
    staleTime: 0,
    refetchOnWindowFocus: true,
  });
  const readLatest = useMutation({
    mutationFn: async () => {
      const { data } = await readItem({
        body: { ...request, at: { kind: 'latest' }, cursor: null },
        throwOnError: true,
      });
      return data;
    },
    onSuccess: (response) => {
      const resolved = response.source.revision;
      const pinned: ReadItemRequest = { ...request, at: { kind: 'revision', revision: resolved } };
      client.setQueryData(readItemInfiniteOptions({ body: pinned }).queryKey, {
        pages: [response],
        pageParams: [{ body: pinned }],
      });
      // The button goes once the pane reads that revision; keyboard focus stays at this notice,
      // which then says which revision is read, instead of dropping to the page.
      const notice = noticeRef.current;
      if (notice?.contains(document.activeElement)) notice.focus();
      onReadLatest(resolved);
    },
  });

  return (
    <div ref={noticeRef} tabIndex={-1} className="pinned-revision">
      <PinnedRevisionState
        workspaceId={workspaceId}
        revision={revision}
        latestHead={latestHead}
        readLatest={readLatest}
      />
    </div>
  );
}

function PinnedRevisionState({
  workspaceId,
  revision,
  latestHead,
  readLatest,
}: {
  workspaceId: string;
  revision: Revision;
  latestHead: UseQueryResult<Workspace>;
  readLatest: UseMutationResult<ReadItemResponse, Error, void>;
}) {
  const reading = <code title={revision}>{short(revision)}</code>;
  if (latestHead.isPending) {
    return (
      <p role="note" className="notice">
        You are reading revision {reading}. Checking whether it is the latest version…
      </p>
    );
  }
  if (latestHead.isError) {
    return (
      <FailureNotice
        context="Checking for the latest version"
        error={latestHead.error}
        onRetry={() => void latestHead.refetch()}
        workspaceId={workspaceId}
      />
    );
  }
  const checked = <Instant value={new Date(latestHead.dataUpdatedAt).toISOString()} />;
  if (latestHead.data.head === revision) {
    return (
      <p role="note" className="notice">
        You are reading revision {reading}, the latest version when checked at {checked}.
      </p>
    );
  }
  return (
    <div role="note" className="notice">
      <p>
        You are reading revision {reading}; it is not the latest version (checked at {checked}).{' '}
        <button
          type="button"
          aria-disabled={readLatest.isPending}
          onClick={() => {
            if (!readLatest.isPending) readLatest.mutate();
          }}
        >
          {readLatest.isPending ? 'Reading the latest version…' : 'Read the latest version'}
        </button>
      </p>
      {readLatest.isError && (
        <FailureNotice
          context="Reading the latest version"
          error={readLatest.error}
          onRetry={() => readLatest.mutate()}
          workspaceId={workspaceId}
        />
      )}
    </div>
  );
}

export function DocumentView({
  workspaceId,
  itemId,
  revision,
  pinned,
  selection,
  onSelect,
  onReadLatest,
}: DocumentViewProps) {
  const titleId = useId();
  const item = useQuery(itemOptions(workspaceId, itemId, revision));
  const request: ReadItemRequest = {
    workspace_id: workspaceId,
    item_id: itemId,
    at: { kind: 'revision', revision },
    view: 'text',
    selection: selection ?? { kind: 'all' },
    cursor: null,
    max_bytes: READ_BUDGET_BYTES,
    max_images: 0,
  };
  const read = useInfiniteQuery({
    ...readItemInfiniteOptions({ body: request }),
    initialPageParam: { body: request },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });

  const pages = read.data?.pages ?? [];
  const first = pages[0];
  const last = pages.at(-1);
  const summary = item.data?.summary;
  const path = summary?.path ?? first?.source.path;
  const title = summary?.title || path || 'Item';
  const warnings = pages.flatMap((page) => page.warnings);
  const extraction = first?.extraction;

  // "Continue reading" moves keyboard focus to the part it loaded. If none arrived, focus stays
  // on the button, which is still there, and the status says what happened.
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const continueRef = useRef<HTMLButtonElement | null>(null);
  const focusPart = useRef<number | null>(null);
  const continuing = read.isFetchingNextPage;
  const continuationFailed = read.isFetchNextPageError;
  const partCount = pages.length;
  useEffect(() => {
    const part = focusPart.current;
    if (part === null) return;
    // Settled once the asked-for part is here, or the continuation failed.
    if (partCount > part) {
      focusPart.current = null;
      const loaded = bodyRef.current?.children.item(part);
      if (loaded instanceof HTMLElement) loaded.focus();
    } else if (continuationFailed && !continuing) {
      focusPart.current = null;
      continueRef.current?.focus();
    }
  }, [partCount, continuing, continuationFailed]);
  const continueReading = () => {
    if (continuing) return;
    focusPart.current = pages.length;
    void read.fetchNextPage();
  };

  return (
    <article className="document" aria-labelledby={titleId}>
      <header className="document-header">
        <h2 id={titleId}>{title}</h2>
        <p className="meta">
          {path ? <span>{path}</span> : null}
          {summary ? (
            <>
              {' · '}
              {ROLE[summary.kind]}
              {' · type '}
              <span className="type-name">{summary.type_name}</span>
              {' · status '}
              {summary.status}
              {summary.archived ? ' · archived' : ''}
            </>
          ) : null}
          {' · revision '}
          <code title={revision}>{short(revision)}</code>
        </p>
        {summary?.description ? <p>{summary.description}</p> : null}
      </header>

      {pinned && (
        <PinnedRevision
          workspaceId={workspaceId}
          revision={revision}
          request={request}
          onReadLatest={onReadLatest}
        />
      )}
      {selection && (
        <p role="note" className="notice">
          Showing {describeSelection(selection)}.{' '}
          <button type="button" onClick={() => onSelect(undefined)}>
            Show the whole item
          </button>
        </p>
      )}

      {item.isError ? (
        <FailureNotice
          context="Reading this item’s properties"
          error={item.error}
          onRetry={() => void item.refetch()}
          workspaceId={workspaceId}
        />
      ) : item.data ? (
        <Properties properties={item.data.properties} />
      ) : (
        <p role="status">Loading properties…</p>
      )}

      {first && first.outline.length > 0 && (
        <details className="outline">
          <summary>Outline ({first.outline.length})</summary>
          <nav aria-label="Outline">
            <ol>
              {keyed(first.outline, (entry) => JSON.stringify(entry)).map(
                ({ key, value: entry }) => (
                  <li key={key} style={{ marginInlineStart: `${entry.level}rem` }}>
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => onSelect(entry.selection)}
                    >
                      {entry.label}
                    </button>
                  </li>
                ),
              )}
            </ol>
          </nav>
        </details>
      )}

      {extractionLabels(extraction).map((label) => (
        <p key={label} role="note" className="notice">
          {label}
        </p>
      ))}

      {read.isPending && <p role="status">Reading…</p>}
      {read.isError && !read.isFetchNextPageError && (
        <FailureNotice
          context="Reading this item"
          error={read.error}
          onRetry={() => void read.refetch()}
          workspaceId={workspaceId}
        />
      )}
      {pages.length > 0 && (
        <div className="document-body" ref={bodyRef}>
          {keyed(pages, (page) => page.receipt_id).map(({ key, value: page }, index) => (
            <div
              key={key}
              data-part
              tabIndex={-1}
              {...(pages.length > 1
                ? { role: 'group', 'aria-label': `Part ${index + 1} of this reading` }
                : {})}
            >
              <SafeMarkdown markdown={page.markdown} />
            </div>
          ))}
        </div>
      )}
      <p role="status" className="sr-only">
        {readingStatus(read, pages.length)}
      </p>
      {keyed(warnings, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}
      {last?.truncated && (
        <div role="note" className="notice">
          <p>
            Partial reading: this is not all of {describeSelection(request.selection)}.
            {last.next_cursor ? '' : ' The rest cannot be continued from here.'}
          </p>
          {read.hasNextPage && (
            // aria-disabled rather than disabled while reading: the button stays focusable and
            // announced (HTML's focus fixup lets an engine blur a focused button that is disabled).
            <button
              type="button"
              ref={continueRef}
              aria-disabled={continuing}
              onClick={continueReading}
            >
              {continuing ? 'Reading more…' : 'Continue reading'}
            </button>
          )}
        </div>
      )}
      {read.isFetchNextPageError && (
        <FailureNotice
          context="Continuing this reading"
          error={read.error}
          onRetry={() => void read.fetchNextPage()}
          workspaceId={workspaceId}
        />
      )}
      {first && (
        <p className="citation">
          Read from {first.source.path} at{' '}
          <code title={first.source.revision}>{short(first.source.revision)}</code>,{' '}
          {describeSelection(first.source.selection)}.
        </p>
      )}
    </article>
  );
}
