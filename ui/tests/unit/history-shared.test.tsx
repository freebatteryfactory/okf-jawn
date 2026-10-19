/**
 * The shared Timeline and Changes (SPEC section 9: "Components are shared with the Explorer"):
 * what each says of a version and of a change, without Git's vocabulary (section 3: "Ordinary
 * editing and navigation must not require Git vocabulary"), with instants in the reader's own
 * locale and time zone, and with one bounded diff reader for every comparison.
 */
import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { Commit, DiffResponse, FileChange } from '../../src/api/generated/types.gen';
import { MAX_DIFF_LINES } from '../../src/features/drafts/patch';
import { Changes } from '../../src/features/history/Changes';
import { Timeline } from '../../src/features/history/Timeline';
import { formatInstant } from '../../src/lib/instant';

const FROM = '0123456789abcdef0123456789abcdef01234567';
const TO = '89abcdef0123456789abcdef0123456789abcdef';
const AT = '2026-10-05T12:00:00.000Z';

/** A patch as Git writes it for one file: its header, then one hunk. */
const PATCH = [
  'diff --git a/notes/meeting.md b/notes/meeting.md',
  'index 1111111..2222222 100644',
  '--- a/notes/meeting.md',
  '+++ b/notes/meeting.md',
  '@@ -3,3 +3,3 @@',
  ' # Meeting notes',
  '-The budget is draft.',
  '+The budget is published.',
  ' End.',
  '',
].join('\n');

const change = (extra: Partial<FileChange> = {}): FileChange => ({
  old_path: 'notes/meeting.md',
  new_path: 'notes/meeting.md',
  kind: 'modified',
  binary: false,
  patch: PATCH,
  ...extra,
});

const diff = (changes: FileChange[], extra: Partial<DiffResponse> = {}): DiffResponse => ({
  from: FROM,
  to: TO,
  changes,
  warnings: [],
  ...extra,
});

const commit = (extra: Partial<Commit> = {}): Commit => ({
  revision: TO,
  parents: [FROM],
  message: 'Publish the budget',
  author: 'ada',
  committed_at: AT,
  ...extra,
});

describe('Changes', () => {
  it('names the two versions shortened, each with its whole revision in a title, and never only by revision', () => {
    render(<Changes result={diff([change()])} />);
    expect(screen.getByRole('heading', { level: 2, name: 'Changes' })).toBeDefined();
    const region = screen.getByRole('region', { name: 'Changes' });
    expect(region.textContent).toContain(
      `From revision ${FROM.slice(0, 10)} to revision ${TO.slice(0, 10)}`,
    );
    expect(region.textContent).not.toContain(FROM);
    expect(region.textContent).not.toContain(TO);
    const codes = [...region.querySelectorAll('code')].map((code) => code.getAttribute('title'));
    expect(codes).toContain(FROM);
    expect(codes).toContain(TO);
  });

  it('shows each changed line, added and removed in words and signs, and no part of Git’s patch header', () => {
    render(<Changes result={diff([change()])} />);
    const region = screen.getByRole('region', { name: 'Changes' });
    const text = region.textContent ?? '';
    for (const header of ['diff --git', 'index 1111111', '--- a/', '+++ b/', '@@']) {
      expect(text).not.toContain(header);
    }
    expect(text).toContain('Changed: notes/meeting.md');
    const removed = region.querySelector('del');
    const added = region.querySelector('ins');
    expect(removed?.textContent).toBe('Removed: − The budget is draft.\n');
    expect(added?.textContent).toBe('Added: + The budget is published.\n');
    expect(text).toContain('From line 3 of the file');
  });

  it('says what each change is: added, moved with both paths, deleted, and a file that is not text', () => {
    render(
      <Changes
        result={diff([
          change({ kind: 'added', old_path: null, new_path: 'notes/new.md' }),
          change({
            kind: 'moved',
            old_path: 'notes/old.md',
            new_path: 'archive/old.md',
            patch: '',
          }),
          change({ kind: 'removed', old_path: 'notes/gone.md', new_path: null }),
          change({ new_path: 'report.pdf', old_path: 'report.pdf', binary: true, patch: '' }),
        ])}
      />,
    );
    const text = screen.getByRole('region', { name: 'Changes' }).textContent ?? '';
    expect(text).toContain('Added: notes/new.md');
    expect(text).toContain('Moved: notes/old.md, now archive/old.md');
    expect(text).toContain('No line of its text changed.');
    expect(text).toContain('Deleted: notes/gone.md');
    expect(text).toContain('This file is not text, so its lines cannot be compared.');
  });

  it('bounds the lines put on the page over all files and counts the rest', () => {
    const many = (count: number, mark: string) =>
      [
        '@@ -1,1 +1,1 @@',
        ...Array.from({ length: count }, (_, line) => `+${mark} ${line}`),
        '',
      ].join('\n');
    render(
      <Changes
        result={diff([
          change({ new_path: 'a.md', old_path: 'a.md', patch: many(MAX_DIFF_LINES - 10, 'a') }),
          change({ new_path: 'b.md', old_path: 'b.md', patch: many(25, 'b') }),
        ])}
      />,
    );
    const region = screen.getByRole('region', { name: 'Changes' });
    expect(region.querySelectorAll('ins')).toHaveLength(MAX_DIFF_LINES);
    expect(region.textContent).toContain(
      'and 15 more lines: this change is too long to show whole.',
    );
  });

  it('says so when nothing changed, and shows the comparison’s warnings', () => {
    render(
      <Changes
        result={diff([], {
          warnings: [{ code: 'binary_skipped', message: 'One file was not compared.' }],
        })}
      />,
    );
    const region = screen.getByRole('region', { name: 'Changes' });
    expect(region.textContent).toContain('Nothing changed between these versions.');
    expect(within(region).getByRole('note').textContent).toBe('One file was not compared.');
  });

  it('takes the name and heading level of the place it is shown in', () => {
    render(<Changes result={diff([change()])} title="What Rewind will change" level={4} />);
    expect(
      screen.getByRole('heading', { level: 4, name: 'What Rewind will change' }),
    ).toBeDefined();
    expect(screen.getByRole('region', { name: 'What Rewind will change' })).toBeDefined();
  });
});

describe('Timeline', () => {
  it('lists each version by what it says and when, in the reader’s locale, its revision shortened', () => {
    render(<Timeline result={{ commits: [commit()], next_cursor: null }} />);
    expect(screen.getByRole('heading', { level: 2, name: 'Timeline' })).toBeDefined();
    expect(screen.getByText('Publish the budget')).toBeDefined();
    const time = screen.getByText(formatInstant(AT) ?? 'no time');
    expect(time.tagName).toBe('TIME');
    expect(time.getAttribute('dateTime')).toBe(AT);
    const entry = screen.getByRole('listitem');
    expect(entry.textContent).not.toContain(AT);
    expect(entry.textContent).not.toContain(TO);
    expect(entry.textContent).toContain(`revision ${TO.slice(0, 10)}`);
    expect(entry.querySelector('code')?.getAttribute('title')).toBe(TO);
    // The author is the label the version carries, not a verified person.
    expect(entry.textContent).toContain('author recorded as ada');
  });

  it('says so when there is no version, and gives each version the caller’s actions', () => {
    const { unmount } = render(<Timeline result={{ commits: [], next_cursor: null }} />);
    expect(screen.getByRole('region', { name: 'Timeline' }).textContent).toContain(
      'No versions to show.',
    );
    unmount();
    render(
      <Timeline
        result={{ commits: [commit(), commit({ revision: FROM, parents: [], message: 'Start' })] }}
        actions={(shown) => <button type="button">Show {shown.message}</button>}
        title="Timeline of Meeting notes"
        level={3}
      />,
    );
    expect(
      screen.getByRole('heading', { level: 3, name: 'Timeline of Meeting notes' }),
    ).toBeDefined();
    expect(screen.getByRole('button', { name: 'Show Publish the budget' })).toBeDefined();
    expect(screen.getByRole('button', { name: 'Show Start' })).toBeDefined();
  });

  it('names a version without a message as such', () => {
    render(<Timeline result={{ commits: [commit({ message: '' })] }} />);
    expect(screen.getByRole('listitem').textContent).toContain('(no description)');
  });
});
