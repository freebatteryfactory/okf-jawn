/** Present occurrence metadata and exact citations without inventing document contents. */
import type { z } from 'zod';
import type { zGetSourcesResponse } from '../../api/generated/zod.gen';

export interface SourcesProps {
  result: z.infer<typeof zGetSourcesResponse>;
}

export function Sources({ result }: SourcesProps) {
  return (
    <section aria-label="Sources">
      <h2>Sources</h2>
      <p>
        Resolved revision: <code>{result.revision}</code>
      </p>
      {result.appearance && (
        <dl>
          <dt>Original object</dt>
          <dd>
            <code>{result.appearance.object}</code>
          </dd>
          <dt>Observed names</dt>
          <dd>
            {result.appearance.names.map((name) => (
              <p key={`${name.folder}/${name.filename}:${name.observed_at}`}>
                {name.folder}/{name.filename} · {name.observed_at}
              </p>
            ))}
          </dd>
        </dl>
      )}
      <ul>
        {result.sources.map((source) => (
          <li key={`${source.item_id}:${source.revision}:${JSON.stringify(source.selection)}`}>
            <strong>{source.path}</strong> <code>{source.revision}</code>
            <pre>{JSON.stringify(source.selection, null, 2)}</pre>
          </li>
        ))}
      </ul>
    </section>
  );
}
