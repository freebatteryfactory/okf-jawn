/**
 * Markdown drawn under a heading of the page starts below it (`SafeMarkdown`'s `headingLevel`), so
 * the page's outline never skips a level: a View's source excerpt sits under the View's own h2,
 * and a source that opens with `# Title` must not put an h1 there, then the View's next h3 after it
 * (axe `heading-order`). Without a level, Markdown is drawn as it always was: `#` is an h1.
 */

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { SafeMarkdown } from '../../src/features/documents/SafeMarkdown';
import { SourceExcerpt } from '../../src/features/documents/SourceExcerpt';

afterEach(cleanup);

const OUTLINE = '# Title\n\n## Part\n\n### Detail\n\n###### Smallest\n\nText.';

/** The level of each heading drawn, in order, with its text. */
function outline(container: HTMLElement) {
  return Array.from(container.querySelectorAll('h1, h2, h3, h4, h5, h6'), (heading) => [
    heading.tagName.toLowerCase(),
    heading.textContent,
  ]);
}

describe('SafeMarkdown headings', () => {
  it('draws `#` as an h1 when no level is given, as it always has', () => {
    const { container } = render(<SafeMarkdown markdown={OUTLINE} />);
    expect(outline(container)).toEqual([
      ['h1', 'Title'],
      ['h2', 'Part'],
      ['h3', 'Detail'],
      ['h6', 'Smallest'],
    ]);
  });

  it('starts at the level given, keeps the steps between headings, and goes no deeper than h6', () => {
    const { container } = render(<SafeMarkdown markdown={OUTLINE} headingLevel={3} />);
    expect(outline(container)).toEqual([
      ['h3', 'Title'],
      ['h4', 'Part'],
      ['h5', 'Detail'],
      ['h6', 'Smallest'],
    ]);
    expect(screen.getByRole('heading', { level: 3, name: 'Title' })).toBeTruthy();
    expect(screen.getByText('Text.').tagName).toBe('P');
  });

  it('keeps everything else of a heading: its words and its inline Markdown', () => {
    const { container } = render(<SafeMarkdown markdown="# The *first* `part`" headingLevel={2} />);
    const heading = screen.getByRole('heading', { level: 2, name: 'The first part' });
    expect(heading.querySelector('em')?.textContent).toBe('first');
    expect(heading.querySelector('code')?.textContent).toBe('part');
    expect(container.querySelector('h1')).toBeNull();
  });
});

describe('SourceExcerpt headings', () => {
  const result = {
    markdown: '# Qualification source\n\nWords.',
    media: [],
    outline: [],
    receipt_id: 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
    source: {
      item_id: '11111111-2222-4333-8444-555555555555',
      path: 'notes/source.md',
      revision: '0123456789abcdef0123456789abcdef01234567',
      selection: { kind: 'all' as const },
      workspace_id: 'aaaaaaaa-bbbb-4ccc-8ddd-ffffffffffff',
    },
    truncated: false,
    view: 'text' as const,
    warnings: [],
  };

  it('draws the source at the level its place on the page gives it', () => {
    const { container } = render(<SourceExcerpt result={result} headingLevel={3} />);
    expect(outline(container)).toEqual([['h3', 'Qualification source']]);
  });

  it('draws it as before when given no level', () => {
    const { container } = render(<SourceExcerpt result={result} />);
    expect(outline(container)).toEqual([['h1', 'Qualification source']]);
  });
});
