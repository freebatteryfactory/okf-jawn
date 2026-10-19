/**
 * A comparison at its own address (`/w/<id>/compare?from=…&to=…&item=…`), where the page tool
 * `open_comparison` lands (`webmcp/tools.ts`; SPEC section 9: "open a comparison/source"). The
 * Explorer had no address for two arbitrary versions; this page is a thin, addressable shell over
 * M3's one diff reader (ruling R3) and draws nothing of its own: the comparison is asked through
 * `history.ts` (`diffOptions`, over the generated `diffItemsOptions`), the same query the Timeline
 * asks, and shown by `Changes.tsx`, which draws each file through `changed-files.tsx`. Like every
 * page of the workspace it is shown in place of the documents, and its address can be kept.
 *
 * Both versions are named by the address as exact revisions, never "the latest", and both are
 * shown "as of" their revision. An address that does not name two revisions is refused in the
 * words the workspace route uses for a link that is not valid, and asks nothing. A comparison the
 * service refuses is shown as the Timeline and Rewind show theirs: a failure notice with the
 * failure's own words and its retry, never "nothing changed".
 */
import { useQuery } from '@tanstack/react-query';
import { useId } from 'react';
import type { Revision } from '../../api/generated/types.gen';
import { zItemId, zRevision } from '../../api/generated/zod.gen';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import { Changes } from './Changes';
import { diffOptions } from './history';

/** Two exact versions to compare, of the whole workspace or of one item (`item`). */
export interface Comparison {
  from: Revision;
  to: Revision;
  item: string | null;
}

/** The comparison's search parameters, each kept only when it is valid. */
export interface ComparisonSearch {
  from?: Revision;
  to?: Revision;
  item?: string;
}

export function validateComparisonSearch(raw: Record<string, unknown>): ComparisonSearch {
  const search: ComparisonSearch = {};
  const from = zRevision.safeParse(raw.from);
  if (from.success) search.from = from.data;
  const to = zRevision.safeParse(raw.to);
  if (to.success) search.to = to.data;
  const item = zItemId.safeParse(raw.item);
  if (item.success) search.item = item.data;
  return search;
}

/** The comparison an address names, or null when it does not name two revisions. */
export function comparisonOf(raw: Record<string, unknown>): Comparison | null {
  const { from, to, item } = validateComparisonSearch(raw);
  if (from === undefined || to === undefined) return null;
  return { from, to, item: item ?? null };
}

export interface ComparisonPageProps {
  workspaceId: string;
  comparison: Comparison | null;
}

export function ComparisonPage({ workspaceId, comparison }: ComparisonPageProps) {
  const heading = useId();
  return (
    <section className="workspace-page" aria-labelledby={heading}>
      <h2 id={heading} tabIndex={-1} className="page-heading">
        Comparison
      </h2>
      {comparison === null ? (
        <div role="alert" className="failure-notice">
          <p>
            <strong>This link is not valid.</strong>
          </p>
          <p>The address does not name two versions to compare. Nothing was requested.</p>
        </div>
      ) : (
        <ComparedChanges
          key={`${comparison.from} ${comparison.to} ${comparison.item ?? ''}`}
          workspaceId={workspaceId}
          comparison={comparison}
        />
      )}
    </section>
  );
}

function ComparedChanges({
  workspaceId,
  comparison,
}: {
  workspaceId: string;
  comparison: Comparison;
}) {
  const { from, to, item } = comparison;
  const diff = useQuery(diffOptions(workspaceId, from, to, item));
  const failure = useRetry({
    request: `${workspaceId} ${from} ${to} ${item ?? ''}`,
    failed: diff.isError,
    error: diff.error,
    run: () => diff.refetch(),
    loaded: () => null,
  });
  return (
    <>
      <p className="meta">
        {item === null ? 'The workspace' : 'One item'} as of revision{' '}
        <code title={from}>{short(from)}</code>, compared with{' '}
        {item === null ? 'the workspace' : 'the item'} as of revision{' '}
        <code title={to}>{short(to)}</code>.
      </p>
      {diff.isPending && !failure.retrying && <p role="status">Comparing the two versions…</p>}
      {failure.shown && (
        <FailureNotice
          context="Comparing the two versions"
          error={failure.error}
          retry={failure}
          workspaceId={workspaceId}
        />
      )}
      {diff.data !== undefined && <Changes result={diff.data} title="What changed" level={3} />}
    </>
  );
}
