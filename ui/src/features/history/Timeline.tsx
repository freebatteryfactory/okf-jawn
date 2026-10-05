/** Show content versions, not an invented complete application audit trail. */
import type { z } from 'zod';
import type { zLogResponse } from '../../api/generated/zod.gen';

export interface TimelineProps {
  result: z.infer<typeof zLogResponse>;
}

export function Timeline({ result }: TimelineProps) {
  return (
    <section aria-label="Timeline">
      <h2>Timeline</h2>
      <ol>
        {result.commits.map((commit) => (
          <li key={commit.revision}>
            <strong>{commit.message}</strong>
            <p>
              {commit.author} · <time>{commit.committed_at}</time>
            </p>
            <code>{commit.revision}</code>
          </li>
        ))}
      </ol>
    </section>
  );
}
