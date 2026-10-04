/** Source content is supplied by the application's read result, never model-written quotation props. */
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeSanitize from 'rehype-sanitize';
import type { ReadItemResponse } from '../../api/generated/types.gen';

export interface SourceExcerptProps { result: ReadItemResponse }

export function SourceExcerpt({ result }: SourceExcerptProps) {
  return <article className="source-excerpt">
    <header><strong>{result.source.path}</strong><code>{result.source.revision}</code></header>
    <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeSanitize]} skipHtml
      components={{ a: ({ children }) => <span>{children}</span>, img: ({ alt }) => <span>[Image: {alt ?? 'source image'}]</span> }}>
      {result.markdown}
    </Markdown>
    {result.warnings.map(warning => <p key={`${warning.code}:${warning.message}`} role="note">{warning.message}</p>)}
    {result.truncated && <p role="status">Partial reading. More content is available through the continuation cursor.</p>}
  </article>;
}
