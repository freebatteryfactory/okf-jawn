/** Source content is supplied by the application's read result, never model-written quotation props. */
import type { z } from 'zod';
import type { zReadItemResponse } from '../../api/generated/zod.gen';
import { SafeMarkdown } from './SafeMarkdown';

export interface SourceExcerptProps {
  result: z.infer<typeof zReadItemResponse>;
}

export function SourceExcerpt({ result }: SourceExcerptProps) {
  return (
    <article className="source-excerpt">
      <header>
        <strong>{result.source.path}</strong>
        <code>{result.source.revision}</code>
      </header>
      <SafeMarkdown markdown={result.markdown} />
      {result.warnings.map((warning) => (
        <p key={`${warning.code}:${warning.message}`} role="note">
          {warning.message}
        </p>
      ))}
      {result.truncated && (
        <p role="status">
          Partial reading. More content is available through the continuation cursor.
        </p>
      )}
    </article>
  );
}
