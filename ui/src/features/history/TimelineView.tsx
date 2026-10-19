/**
 * A Timeline in the Explorer: the versions of the workspace, or of one item, newest first, from
 * the revision the workspace or the tab shows, through the Timeline shared with MCP Apps.
 *
 * Each version offers its Changes (what it changed against the version before it, through the
 * one diff reader), "Read this version" for an item (its tab cites that revision), and Rewind to
 * it (`Rewind.tsx`). Rewind needs permission to change the workspace: without it, it is not
 * offered and the Timeline says why above its list; in an archived workspace it is offered and
 * says why it cannot run. The version the workspace shows is offered no Rewind of the workspace:
 * nothing would differ.
 *
 * Versions are listed `LOG_PAGE` at a time; "Show older versions" asks for the next page and
 * moves keyboard focus to the first version it brought. A list that fails with nothing listed
 * is a failure notice, never "no versions"; a further page that fails leaves the list as it is,
 * with its notice beside it. While the list is asked again at a new head (after the person's own
 * Rewind), the versions listed stay on screen, so an open panel and keyboard focus stay too.
 *
 * Showing a version's Changes keeps keyboard focus on its button, which says which it does
 * (`aria-expanded`). Rewind's panel takes focus when it opens; when it closes, focus goes back to
 * the button that opened it.
 */
import { keepPreviousData, useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { Commit, Revision } from '../../api/generated/types.gen';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { allowed, archivedWords, Missing } from '../explorer/rights';
import { useWorkspaceChanges } from '../tree/context';
import { Changes } from './Changes';
import { diffOptions, logOptions } from './history';
import { RewindPanel } from './Rewind';
import { TimelineList } from './Timeline';

export interface TimelineViewProps {
  workspaceId: string;
  /** The revision the Timeline is asked at: the head shown, or the revision a tab cites. */
  revision: Revision;
  /** One item's Timeline, or the workspace's when null. */
  item: { id: string; title: string } | null;
  title: string;
  level: 2 | 3;
}

export function TimelineView({ workspaceId, revision, item, title, level }: TimelineViewProps) {
  const { head, rights } = useWorkspaceChanges();
  const archivedId = useId();
  const itemId = item?.id ?? null;
  const log = useInfiniteQuery({
    ...logOptions(workspaceId, revision, itemId),
    placeholderData: keepPreviousData,
  });
  const commits = (log.data?.pages ?? []).flatMap((page) => page.commits);
  const listRef = useRef<HTMLElement | null>(null);
  const headingId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const Heading = level === 2 ? 'h2' : 'h3';
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const entryAt = (index: number) =>
    listRef.current?.querySelectorAll<HTMLElement>('.timeline-entries > li').item(index) ?? null;

  const nothingListed = log.data === undefined;
  const listFailure = useRetry({
    request: `${workspaceId} ${revision} ${itemId ?? ''}`,
    failed: log.isError && nothingListed,
    error: log.error,
    run: () => log.refetch(),
    loaded: () => entryAt(0) ?? headingRef.current,
  });
  const laterFailure = useRetry({
    request: `${workspaceId} ${revision} ${itemId ?? ''}`,
    failed: log.isError && !nothingListed,
    error: log.error,
    run: () => (log.isFetchNextPageError ? log.fetchNextPage() : log.refetch()),
    loaded: () => entryAt(commits.length - 1),
  });

  // "Show older versions": focus to the first version it brought, else to the button or the last.
  const count = commits.length;
  const focusFrom = useRef<number | null>(null);
  useEffect(() => {
    const from = focusFrom.current;
    if (from === null || log.isFetchingNextPage) return;
    if (count <= from && !log.isFetchNextPageError && log.hasNextPage) return;
    focusFrom.current = null;
    (entryAt(from) ?? moreRef.current ?? entryAt(count - 1))?.focus();
  });

  // Which versions show their Changes, and which one has Rewind's panel open.
  const [shown, setShown] = useState<ReadonlySet<Revision>>(new Set());
  const [rewindAt, setRewindAt] = useState<Revision | null>(null);
  const openers = useRef(new Map<Revision, HTMLButtonElement>());
  const giveBack = useRef<Revision | null>(null);
  useLayoutEffect(() => {
    const back = giveBack.current;
    if (back === null || rewindAt !== null) return;
    giveBack.current = null;
    openers.current.get(back)?.focus();
  });
  const toggle = (revision: Revision) =>
    setShown((now) => {
      const next = new Set(now);
      if (next.has(revision)) next.delete(revision);
      else next.add(revision);
      return next;
    });

  const can = allowed(rights, 'write');
  const actions = (commit: Commit) => {
    const changesId = `${archivedId}-changes-${commit.revision}`;
    const open = shown.has(commit.revision);
    // Rewinding the workspace to the version it shows would change nothing.
    const rewindable = can !== 'no-permission' && (itemId !== null || commit.revision !== head);
    return (
      <>
        <p className="version-actions">
          <button
            type="button"
            aria-expanded={open}
            aria-controls={changesId}
            onClick={() => toggle(commit.revision)}
          >
            {open ? 'Hide its changes' : 'Show its changes'}
          </button>
          {item !== null && (
            <>
              {' '}
              <Link
                to="/w/$workspaceId/i/$itemId"
                params={{ workspaceId, itemId: item.id }}
                search={{ rev: commit.revision }}
              >
                Read this version
              </Link>
            </>
          )}
          {rewindable && (
            <>
              {' '}
              <button
                type="button"
                ref={(button) => {
                  if (button === null) openers.current.delete(commit.revision);
                  else openers.current.set(commit.revision, button);
                }}
                aria-expanded={rewindAt === commit.revision}
                {...(can === 'archived' ? { 'aria-describedby': archivedId } : {})}
                onClick={() =>
                  setRewindAt((now) => (now === commit.revision ? null : commit.revision))
                }
              >
                Rewind to this version…
              </button>
            </>
          )}
        </p>
        <div id={changesId}>
          {open && (
            <VersionChanges
              workspaceId={workspaceId}
              commit={commit}
              itemId={itemId}
              level={level === 2 ? 3 : 4}
            />
          )}
        </div>
        {rewindAt === commit.revision && (
          <RewindPanel
            workspaceId={workspaceId}
            target={commit.revision}
            item={item}
            level={level === 2 ? 3 : 4}
            onClose={() => {
              giveBack.current = commit.revision;
              setRewindAt(null);
            }}
          />
        )}
      </>
    );
  };

  return (
    <section
      className={level === 2 ? 'workspace-page timeline-view' : 'timeline-view'}
      ref={listRef}
      aria-labelledby={headingId}
    >
      <Heading
        id={headingId}
        ref={headingRef}
        tabIndex={-1}
        className={level === 2 ? 'page-heading' : undefined}
      >
        {title}
      </Heading>
      <p className="meta">
        Newest first, from revision <code title={revision}>{short(revision)}</code>
        {revision === head ? ', the one this workspace shows' : ''}. Each version is a saved state
        of {item === null ? 'the workspace' : 'this item'}, not a record of everything done in it.
      </p>
      {can === 'no-permission' && (
        <Missing action="Rewind, which brings back an earlier version," permission="write" />
      )}
      {can === 'archived' && rights.archivedAt !== null && (
        <p id={archivedId} className="meta">
          {archivedWords(rights.archivedAt)}
        </p>
      )}
      {log.isPending && !listFailure.retrying && <p role="status">Listing the versions…</p>}
      {listFailure.shown && (
        <FailureNotice
          context="Listing the versions"
          error={listFailure.error}
          retry={listFailure}
          workspaceId={workspaceId}
        />
      )}
      {log.data !== undefined && (
        <TimelineList commits={commits} actions={actions} entryFocusable />
      )}
      {laterFailure.shown && (
        <FailureNotice
          context="Listing more versions"
          error={laterFailure.error}
          retry={laterFailure}
          workspaceId={workspaceId}
        />
      )}
      {log.hasNextPage && (
        <button
          type="button"
          ref={moreRef}
          aria-disabled={log.isFetchingNextPage}
          onClick={() => {
            if (log.isFetchingNextPage) return;
            focusFrom.current = count;
            void log.fetchNextPage();
          }}
        >
          {log.isFetchingNextPage ? 'Listing older versions…' : 'Show older versions'}
        </button>
      )}
    </section>
  );
}

/** What one version changed against the version before it. */
function VersionChanges({
  workspaceId,
  commit,
  itemId,
  level,
}: {
  workspaceId: string;
  commit: Commit;
  itemId: string | null;
  level: 3 | 4;
}) {
  const before = commit.parents.at(0) ?? null;
  const diff = useQuery({
    ...diffOptions(workspaceId, before ?? '', commit.revision, itemId),
    enabled: before !== null,
  });
  const failure = useRetry({
    request: `${workspaceId} ${commit.revision} ${itemId ?? ''}`,
    failed: diff.isError,
    error: diff.error,
    run: () => diff.refetch(),
    loaded: () => null,
  });
  if (before === null) {
    return <p>This is the first version: there is nothing earlier to compare it with.</p>;
  }
  return (
    <>
      {commit.parents.length > 1 && (
        <p className="meta">
          This version follows {commit.parents.length} earlier versions; it is compared with the
          first of them.
        </p>
      )}
      {diff.isPending && !failure.retrying && (
        <p role="status">Comparing with the version before…</p>
      )}
      {failure.shown && (
        <FailureNotice
          context="Comparing this version with the one before"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {diff.data !== undefined && <Changes result={diff.data} title="Its changes" level={level} />}
    </>
  );
}
