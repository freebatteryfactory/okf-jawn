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
 *
 * Markdown shown under a heading of the page starts below it (`headingLevel`): a `#` is drawn at
 * that level and deeper headings keep their steps from it, none deeper than h6, so the page's
 * outline never skips a level. Without one, a `#` is an h1, as a document of its own.
 */
import type { ComponentPropsWithoutRef } from 'react';
import Markdown, { type Components, type ExtraProps } from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';

/** A heading level of HTML. */
export type HeadingLevel = 1 | 2 | 3 | 4 | 5 | 6;

export interface SafeMarkdownProps {
  markdown: string;
  /** The level a `#` heading is drawn at: one below the page heading it is shown under. */
  headingLevel?: HeadingLevel;
}

const HEADINGS = ['h1', 'h2', 'h3', 'h4', 'h5', 'h6'] as const;

/** Each Markdown heading drawn `headingLevel - 1` levels deeper, none deeper than h6. */
function shiftedHeadings(headingLevel: HeadingLevel): Components {
  const shifted: Components = {};
  HEADINGS.forEach((markdownTag, index) => {
    const Tag = HEADINGS[Math.min(HEADINGS.length - 1, index + headingLevel - 1)] ?? 'h6';
    shifted[markdownTag] = ({
      node: _node,
      ...props
    }: ComponentPropsWithoutRef<'h1'> & ExtraProps) => <Tag {...props} />;
  });
  return shifted;
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

export function SafeMarkdown({ markdown, headingLevel = 1 }: SafeMarkdownProps) {
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
        ...(headingLevel === 1 ? {} : shiftedHeadings(headingLevel)),
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
