/**
 * The comparison page (`/w/<id>/compare`), where the page tool `open_comparison` lands: a thin,
 * addressable shell over M3's one diff reader. Both versions are exact revisions in the address
 * and are shown "as of"; an address that does not name two revisions is refused in the words the
 * workspace route already uses for a link that is not valid, and asks nothing.
 */
import { cleanup, configure, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mockApi, ok } from './support/api';
import { changeOf, DIFF, diffOf } from './support/history';
import { explorerHandlers, HEAD, NOTE, OLD, renderAt, WS } from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function setup() {
  return mockApi(
    explorerHandlers({
      [DIFF]: (body) => ok(diffOf(body, [changeOf('notes/meeting.md', 'Draft.', 'Published.')])),
    }),
  );
}

describe('the comparison page', { timeout: 30_000 }, () => {
  it('shows the Changes between the two revisions its address names, each "as of"', async () => {
    const api = setup();
    renderAt(`/w/${WS}/compare?from=${OLD}&to=${HEAD}&item=${NOTE}`);
    const page = await screen.findByRole('region', { name: 'Comparison' });
    expect(within(page).getByRole('heading', { level: 2, name: 'Comparison' })).toBeTruthy();
    const changes = await within(page).findByRole('region', { name: 'What changed' });
    expect(changes.querySelector('ins')?.textContent).toBe('Added: + Published.\n');
    expect(page.textContent).toContain(
      `One item as of revision ${OLD.slice(0, 10)}, compared with the item as of revision ${HEAD.slice(0, 10)}.`,
    );
    expect(api.callsTo(DIFF)).toEqual([{ workspace_id: WS, from: OLD, to: HEAD, item_id: NOTE }]);
    expect(api.violations).toEqual([]);
  });

  it('refuses an address that does not name two revisions, and asks nothing', async () => {
    const api = setup();
    renderAt(`/w/${WS}/compare?from=latest&to=${HEAD}`);
    const page = await screen.findByRole('region', { name: 'Comparison' });
    const refused = within(page).getByRole('alert');
    expect(refused.textContent).toContain('This link is not valid.');
    expect(refused.textContent).toContain('Nothing was requested.');
    expect(api.callsTo(DIFF)).toEqual([]);
  });
});
