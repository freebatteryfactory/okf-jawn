/**
 * The Timeline: content versions, newest first, shared by MCP Apps and the Explorer (SPEC section
 * 9). It shows content snapshots, not an application audit trail: Git history is not the complete
 * event log.
 *
 * Each version is said by what it says (its message) and when it was made, in the reader's own
 * locale and time zone (SPEC section 3), then by the author label it carries, which is a label
 * and not a verified person, and its revision, shortened, with the whole revision in a title. A
 * revision is never a person's only handle on a version.
 *
 * It takes no data of its own: the caller passes the answer, the name and heading level of the
 * place it is shown in, and what each version offers there (`actions`: the Explorer's Changes,
 * Read and Rewind; MCP Apps passes none).
 */
import type { ReactNode } from 'react';
import type { z } from 'zod';
import type { Commit } from '../../api/generated/types.gen';
import type { zLogResponse } from '../../api/generated/zod.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import { short } from '../documents/describe';

export interface TimelineListProps {
  commits: readonly Commit[];
  /** What each version offers besides its words. */
  actions?: ((commit: Commit) => ReactNode) | undefined;
  /** Each version can take keyboard focus from script (a further page moves focus to it). */
  entryFocusable?: boolean;
}

/** The versions themselves, for a caller that gives them its own heading. */
export function TimelineList({ commits, actions, entryFocusable = false }: TimelineListProps) {
  if (commits.length === 0) return <p>No versions to show.</p>;
  return (
    <ol className="timeline-entries">
      {keyed(commits, (commit) => commit.revision).map(({ key, value: commit }) => (
        <li key={key} className="version" {...(entryFocusable ? { tabIndex: -1 } : {})}>
          <strong className="version-message">
            {commit.message.length > 0 ? commit.message : '(no description)'}
          </strong>
          <p className="meta">
            <Instant value={commit.committed_at} />, author recorded as {commit.author}, revision{' '}
            <code title={commit.revision}>{short(commit.revision)}</code>
          </p>
          {actions?.(commit)}
        </li>
      ))}
    </ol>
  );
}

export interface TimelineProps {
  result: z.infer<typeof zLogResponse>;
  /** The Timeline's name where it is shown; "Timeline" by default. */
  title?: string;
  /** Its heading's level; two by default. */
  level?: 2 | 3;
  /** What each version offers besides its words. */
  actions?: (commit: Commit) => ReactNode;
}

export function Timeline({ result, title = 'Timeline', level = 2, actions }: TimelineProps) {
  const Heading = level === 2 ? 'h2' : 'h3';
  return (
    <section aria-label={title} className="timeline">
      <Heading>{title}</Heading>
      <TimelineList commits={result.commits} actions={actions} />
    </section>
  );
}
