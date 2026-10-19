/**
 * Source content is supplied by the application's read result, never model-written quotation props.
 * Its Markdown starts at `headingLevel` (`SafeMarkdown`): one below the page heading it is under.
 */
import type { z } from 'zod';
import type { zReadItemResponse } from '../../api/generated/zod.gen';
import { type HeadingLevel, SafeMarkdown } from './SafeMarkdown';

export interface SourceExcerptProps {
  result: z.infer<typeof zReadItemResponse>;
  /** The level the source's `#` headings are drawn at; h1 when none is given. */
  headingLevel?: HeadingLevel;
}

export function SourceExcerpt({ result, headingLevel }: SourceExcerptProps) {
  return (
    <article className="source-excerpt">
      <header>
        <strong>{result.source.path}</strong>
        <code>{result.source.revision}</code>
      </header>
      <SafeMarkdown
        markdown={result.markdown}
        {...(headingLevel === undefined ? {} : { headingLevel })}
      />
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
