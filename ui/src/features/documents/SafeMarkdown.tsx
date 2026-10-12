/**
 * Document Markdown as sanitized React elements; no raw HTML ever reaches the DOM here.
 *
 * Two independent layers keep raw HTML out: `skipHtml` drops it, and rehype-sanitize removes
 * anything outside its schema (and prefixes every id, so document content cannot clobber the
 * page's own ids). Dropping raw HTML is never silent: where any was removed, one notice says
 * "Embedded HTML is not shown here." (unsupported content is visible, not discarded unseen).
 * GFM task boxes are named, disabled checkboxes. Links stay text and images stay their
 * description here: following links and showing retained images are separate, authorized
 * paths. Hostile HTML is viewed only in the sandbox viewer on its separate origin.
 */
import Markdown from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';

export interface SafeMarkdownProps {
  markdown: string;
}

/** The part of a hast node these plugins read: raw HTML arrives as `raw` nodes. */
interface HastNode {
  type: string;
  children?: HastNode[];
}

export const EMBEDDED_HTML_NOTICE = 'Embedded HTML is not shown here.';

function containsRawHtml(node: HastNode): boolean {
  return node.type === 'raw' || (node.children ?? []).some(containsRawHtml);
}

/** The notice, added after sanitizing: it is the application's own element, not document content. */
function noticeElement() {
  return {
    type: 'element',
    tagName: 'p',
    properties: { role: 'note', className: ['notice'] },
    children: [{ type: 'text', value: EMBEDDED_HTML_NOTICE }],
  };
}

export function SafeMarkdown({ markdown }: SafeMarkdownProps) {
  // Set by the first plugin (before rehype-sanitize removes raw HTML), read by the last.
  const found = { rawHtml: false };
  const detectRawHtml = () => (tree: HastNode) => {
    found.rawHtml = containsRawHtml(tree);
  };
  const noticeRawHtml = () => (tree: HastNode) => {
    if (found.rawHtml) tree.children = [noticeElement(), ...(tree.children ?? [])];
  };
  return (
    <Markdown
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[detectRawHtml, rehypeSanitize, noticeRawHtml]}
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
