/**
 * Read one item: its identity and properties from `get_item`, its text from `read_item` as
 * sanitized Markdown, at one resolved revision and the selection the URL names.
 *
 * A partial read says so and offers its continuation, which resumes the same resolved read.
 * Warnings, partial extraction and agent-supplied text are shown, never smoothed over. Each
 * operation's failure is shown where its content would have been.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useId } from 'react';
import {
  getItemOptions,
  readItemInfiniteOptions,
} from '../../api/generated/@tanstack/react-query.gen';
import type { ReadItemRequest, Revision } from '../../api/generated/types.gen';
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
  /** The workspace head. */
  head: Revision;
  /** The revision to read at: the head, or the exact revision a citation named. */
  revision: Revision;
  selection: ItemSelection | undefined;
  /** Show another selection of this item (or the whole item). */
  onSelect: (selection: ItemSelection | undefined) => void;
  /** Leave a cited revision for the head. */
  onReadLatest: () => void;
}

const ROLE = { note: 'Note', source: 'Source', view: 'View' } as const;

export function itemOptions(workspaceId: string, itemId: string, revision: Revision) {
  return getItemOptions({
    body: { workspace_id: workspaceId, item_id: itemId, at: { kind: 'revision', revision } },
  });
}

export function DocumentView({
  workspaceId,
  itemId,
  head,
  revision,
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

      {revision !== head && (
        <p role="note" className="notice">
          You are reading revision <code title={revision}>{short(revision)}</code>, which a citation
          named; it is not the latest version.{' '}
          <button type="button" onClick={onReadLatest}>
            Read the latest version
          </button>
        </p>
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
        />
      )}
      {pages.length > 0 && (
        <div className="document-body">
          {pages.map((page) => (
            <SafeMarkdown key={page.receipt_id} markdown={page.markdown} />
          ))}
        </div>
      )}
      {keyed(warnings, (warning) => JSON.stringify(warning)).map(({ key, value: warning }) => (
        <p key={key} role="note" className="notice">
          {warning.message}
          {warning.location ? ` (${warning.location})` : ''}
        </p>
      ))}
      {last?.truncated && (
        <div role="status" className="notice">
          <p>
            Partial reading: this is not all of {describeSelection(request.selection)}.
            {last.next_cursor ? '' : ' The rest cannot be continued from here.'}
          </p>
          {read.hasNextPage && (
            <button
              type="button"
              disabled={read.isFetchingNextPage}
              onClick={() => void read.fetchNextPage()}
            >
              {read.isFetchingNextPage ? 'Reading more…' : 'Continue reading'}
            </button>
          )}
        </div>
      )}
      {read.isFetchNextPageError && (
        <FailureNotice
          context="Continuing this reading"
          error={read.error}
          onRetry={() => void read.fetchNextPage()}
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
