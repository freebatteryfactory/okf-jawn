/** Render version comparison without substituting current documents for historical inputs. */
import type { z } from 'zod';
import type { zDiffResponseOutput } from '../../api/generated/zod.gen';

export interface ChangesProps {
  result: z.infer<typeof zDiffResponseOutput>;
}

export function Changes({ result }: ChangesProps) {
  return (
    <section aria-label="Changes">
      <h2>Changes</h2>
      <p>
        <code>{result.from}</code> → <code>{result.to}</code>
      </p>
      {result.changes.map((change, index) => (
        <article key={JSON.stringify([change.old_path, change.new_path, index])}>
          <h3>{change.new_path ?? change.old_path}</h3>
          {change.binary ? (
            <p>Binary content changed; inspect the retained originals.</p>
          ) : (
            <pre>{change.patch}</pre>
          )}
        </article>
      ))}
    </section>
  );
}
