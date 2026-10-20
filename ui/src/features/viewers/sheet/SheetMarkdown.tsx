/**
 * A spreadsheet's text with each of its tables drawn as a `SheetTable` (ruling A2).
 *
 * The Markdown is parsed by the reader's own parser and sanitized the reader's own way
 * (`documents/SafeMarkdown.tsx`): `react-markdown` with GFM, `skipHtml`, and rehype-sanitize, with
 * the one notice where embedded HTML was dropped. A table reaches `SheetTable` as the element the
 * parser built (`gridOf`); everything around the tables (a heading, a note, a picture's
 * description) is shown as the reader shows it, its headings drawn from `h4`, below the
 * "Original" heading the viewer sits under. The tables are numbered after sanitizing, by a step
 * of this application's own (`data-sheet-table`, `data-sheet-tables` on each table element).
 */
import { type ComponentPropsWithoutRef, createContext, useContext } from 'react';
import Markdown, { type Components, type ExtraProps } from 'react-markdown';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { EMBEDDED_HTML_NOTICE } from '../../documents/SafeMarkdown';
import { SheetTable } from './SheetTable';
import { gridOf, type HastNode } from './sheet-read';

/** Whether the whole selection has been read (no table is still to grow), and what says how the
 * keyboard moves in a table. */
const Reading = createContext<{ complete: boolean; describedBy: string | undefined }>({
  complete: false,
  describedBy: undefined,
});

/** The heading a `#` is drawn at, and the deepest. */
const FIRST_HEADING = 4;
const HEADINGS = ['h1', 'h2', 'h3', 'h4', 'h5', 'h6'] as const;

function shifted(): Components {
  const components: Components = {};
  HEADINGS.forEach((markdownTag, index) => {
    const Tag = HEADINGS[Math.min(HEADINGS.length - 1, index + FIRST_HEADING - 1)] ?? 'h6';
    components[markdownTag] = ({
      node: _node,
      ...props
    }: ComponentPropsWithoutRef<'h1'> & ExtraProps) => <Tag {...props} />;
  });
  return components;
}

const numberOf = (node: HastNode, name: string): number => {
  const value = node.properties?.[name];
  return typeof value === 'number' ? value : 0;
};

function TableElement({ node }: ComponentPropsWithoutRef<'table'> & ExtraProps) {
  const { complete, describedBy } = useContext(Reading);
  if (node === undefined) return null;
  return (
    <SheetTable
      grid={gridOf(node)}
      number={numberOf(node, 'dataSheetTable')}
      total={numberOf(node, 'dataSheetTables')}
      complete={complete}
      describedBy={describedBy}
    />
  );
}

const COMPONENTS: Components = { ...shifted(), table: TableElement };

function containsRawHtml(node: HastNode): boolean {
  return node.type === 'raw' || (node.children ?? []).some(containsRawHtml);
}

export const NO_TABLE = 'The text read holds no table.';

/** A paragraph of this application's own, a note when `note`. */
function paragraph(text: string, note: boolean): HastNode {
  return {
    type: 'element',
    tagName: 'p',
    properties: note ? { role: 'note', className: ['notice'] } : {},
    children: [{ type: 'text', value: text }],
  };
}

/** The tables of a tree, in document order. */
function tablesOf(node: HastNode): HastNode[] {
  if (node.type === 'element' && node.tagName === 'table') return [node];
  return (node.children ?? []).flatMap(tablesOf);
}

export interface SheetMarkdownProps {
  markdown: string;
  complete: boolean;
  /** The element that says how the keyboard moves in a table. */
  describedBy?: string | undefined;
}

/** The text with its tables drawn as tables; says when it holds none. */
export function SheetMarkdown({ markdown, complete, describedBy }: SheetMarkdownProps) {
  // Set by the first step, before rehype-sanitize removes raw HTML.
  const found = { rawHtml: false };
  const detectRawHtml = () => (tree: HastNode) => {
    found.rawHtml = containsRawHtml(tree);
  };
  // The numbers and the notices are this application's own, put in after sanitizing.
  const numberTables = () => (tree: { children?: HastNode[] }) => {
    const tables = (tree.children ?? []).flatMap(tablesOf);
    tables.forEach((table, index) => {
      Object.assign(table, {
        properties: {
          ...table.properties,
          dataSheetTable: index + 1,
          dataSheetTables: tables.length,
        },
      });
    });
    const notices = [
      ...(found.rawHtml ? [paragraph(EMBEDDED_HTML_NOTICE, true)] : []),
      ...(tree.children ?? []),
      ...(complete && tables.length === 0 ? [paragraph(NO_TABLE, false)] : []),
    ];
    tree.children = notices;
  };
  return (
    <Reading value={{ complete, describedBy }}>
      <div className="document-body">
        <Markdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={[detectRawHtml, rehypeSanitize, numberTables]}
          skipHtml
          components={COMPONENTS}
        >
          {markdown}
        </Markdown>
      </div>
    </Reading>
  );
}
