/** Instants show in the reader's locale and time zone; the wire value stays machine-readable. */

import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { formatInstant, Instant } from '../../src/lib/instant';

describe('instants', () => {
  it('converts a UTC instant into the given locale and time zone', () => {
    // 23:30 UTC on 1 January is 08:30 on 2 January in Tokyo.
    const shown = formatInstant('2026-01-01T23:30:00Z', 'en-GB', 'Asia/Tokyo');
    expect(shown).toContain('2 Jan 2026');
    expect(shown).toContain('08:30');
  });

  it("renders the reader's own locale and zone and keeps the UTC value as dateTime", () => {
    const value = '2026-03-04T05:06:07Z';
    render(<Instant value={value} />);
    const expected = new Intl.DateTimeFormat(undefined, {
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(new Date(value));
    const time = screen.getByText(expected);
    expect(time.tagName).toBe('TIME');
    expect(time.getAttribute('dateTime')).toBe(value);
    expect(time.textContent).not.toContain('2026-03-04T05:06:07Z');
  });

  it('shows a value that is not an instant as received, flagged, not guessed', () => {
    render(<Instant value="sometime in March" />);
    const time = screen.getByText(/sometime in March/);
    expect(time.textContent).toContain('not a recognised time');
  });
});
