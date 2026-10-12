/**
 * Document Markdown as sanitized React elements; no raw HTML ever reaches the DOM here.
 *
 * Two independent layers keep raw HTML out: `skipHtml` drops it, and rehype-sanitize removes
 * anything outside its schema (and prefixes every id, so document content cannot clobber the
 * page's own ids). GFM task boxes are named, disabled checkboxes. Links stay text and images
 * stay their description here: following links
 * and showing retained images are separate, authorized paths. Hostile HTML is viewed only in
 * the sandbox viewer on its separate origin.
 */
import Markdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';

export interface SafeMarkdownProps {
  markdown: string;
}

export function SafeMarkdown({ markdown }: SafeMarkdownProps) {
  return (
    <Markdown
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[rehypeSanitize]}
      skipHtml
      components={{
        a: ({ children }) => <span>{children}</span>,
        img: ({ alt }) => <span>[Image: {alt ?? 'source image'}]</span>,
        // A GFM task box is the only input Markdown makes: shown, never editable, and named.
        input: ({ type, checked }) =>
          type === 'checkbox' ? (
            <input
              type="checkbox"
              checked={checked === true}
              disabled
              readOnly
              aria-label={checked === true ? 'Task done' : 'Task not done'}
            />
          ) : null,
      }}
    >
      {markdown}
    </Markdown>
  );
}
