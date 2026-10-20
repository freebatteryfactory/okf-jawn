/**
 * "Continue reading" and keyboard focus (UI M4a, ruling A1: the carried focus defect).
 *
 * The defect the owner names: the person presses "Continue reading" and, before its answer
 * comes, chooses another selection of the same item whose reading is already cached in more
 * parts than were asked for. The pending focus request (a part number) was served against that
 * cached reading, and focus jumped into one of its parts, away from the control the person used.
 * The same request also moved focus the person had moved elsewhere meanwhile (to the properties,
 * the search box, another pane): a part that arrived took focus from there, and a continuation
 * that failed put it back on "Continue reading".
 *
 * The fix (`reader/continuation.ts`): the request is bound to the reading it was made for (the
 * whole request: selection, revision, view), so another selection, revision or view, a reading
 * served from the cache included, cancels it; and focus moves only from where the continuation
 * left it: the button, or the page body (a pointer press that focused nothing, or the button
 * leaving with the last part), in the commit that shows the part.
 */
import { act, configure, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fail, type Json, mockApi, type Reply } from './support/api';
import { held, partsOf, sectionOf, sectionsOf } from './support/m4a-reader';
import { explorerHandlers, NOTE, OLD, renderAt, WS } from './support/workspace';

// Several requests a test: on a machine shared with other builds, a wait gets room (no check is weakened).
configure({ asyncUtilTimeout: 5_000 });

afterEach(() => {
  vi.unstubAllGlobals();
});

const READ = '/api/reads/read-item';

/** Where keyboard focus is in each commit that changes `root`, read before any passive effect. */
function focusPerCommit(root: HTMLElement, when: () => boolean): () => Array<Element | null> {
  const seen: Array<Element | null> = [];
  // A mutation observer's callback is a microtask: it runs once the commit and its layout effects
  // are done, before the passive effects React schedules for after the paint.
  const observer = new MutationObserver(() => {
    if (when()) seen.push(document.activeElement);
  });
  observer.observe(root, { childList: true, subtree: true, characterData: true });
  return () => {
    observer.disconnect();
    return seen;
  };
}

async function openNote(url = `/w/${WS}/i/${NOTE}`) {
  renderAt(url);
  const article = await screen.findByRole('article', { name: 'Meeting notes' });
  await within(article).findByText('Part 1 of the text');
  return article;
}

/** The in-text outline's entry for `heading`, its disclosure opened. */
async function outlineEntry(article: HTMLElement, heading: string) {
  const summary = await within(article).findByText(/^Outline \(\d+\)$/);
  const details = summary.closest('details');
  if (details !== null && !details.open) await userEvent.click(summary);
  const nav = within(article).getByRole('navigation', { name: 'Outline' });
  return within(nav).getByRole('button', { name: heading });
}

describe('the owner’s carried defect: a continuation pending when the selection changes (A1)', () => {
  it('keeps focus on the control that chose a selection already read in several parts', async () => {
    const answer = held(sectionsOf);
    const api = mockApi(explorerHandlers({ [READ]: answer.handler }));
    const budget = encodeURIComponent(JSON.stringify(sectionOf('Budget')));
    renderAt(`/w/${WS}/i/${NOTE}?sel=${budget}`);
    const article = await screen.findByRole('article', { name: 'Meeting notes' });
    // Budget is read to its end: its reading is cached in three parts.
    await within(article).findByText('Budget part 1');
    for (const number of [2, 3]) {
      within(article).getByRole('button', { name: 'Continue reading' }).focus();
      await userEvent.keyboard('{Enter}');
      await within(article).findByText(`Budget part ${number}`);
    }
    // Agenda is chosen, and its first part read.
    await userEvent.click(await outlineEntry(article, 'Agenda'));
    await within(article).findByText('Agenda part 1');

    // "Continue reading" of Agenda, its answer held back.
    const more = within(article).getByRole('button', { name: 'Continue reading' });
    act(() => more.focus());
    answer.hold();
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(answer.waiting()).toBe(1));
    // Meanwhile Budget is chosen again from the keyboard: its reading comes from the cache.
    const entry = await outlineEntry(article, 'Budget');
    act(() => entry.focus());
    await userEvent.keyboard('{Enter}');
    await within(article).findByText('Budget part 3');
    answer.release();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(document.activeElement).toBe(await outlineEntry(article, 'Budget'));
    expect(document.activeElement?.closest('[data-part]')).toBeNull();
    expect(api.violations).toEqual([]);
  });
});

describe('"Continue reading" and keyboard focus (A1)', () => {
  it('puts focus on the last part in the commit that shows it, never on the page body', async () => {
    mockApi(explorerHandlers({ [READ]: partsOf(2) }));
    const article = await openNote();
    const stop = focusPerCommit(
      article,
      () => article.textContent?.includes('Part 2 of the text') ?? false,
    );
    within(article).getByRole('button', { name: 'Continue reading' }).focus();
    await userEvent.keyboard('{Enter}');
    const part = await within(article).findByRole('group', { name: 'Part 2 of this reading' });
    await waitFor(() => expect(document.activeElement).toBe(part));
    const seen = stop();
    expect(seen.length).toBeGreaterThan(0);
    // In the first commit that shows the last part, focus is already on it.
    expect(seen[0]).toBe(part);
    expect(seen).not.toContain(document.body);
  });

  it('leaves focus where the person moved it while the part was read', async () => {
    const answer = held(partsOf(3));
    mockApi(explorerHandlers({ [READ]: answer.handler }));
    const article = await openNote();
    const button = within(article).getByRole('button', { name: 'Continue reading' });
    act(() => button.focus());
    answer.hold();
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(answer.waiting()).toBe(1));
    // The person goes on to the properties while the part is read.
    const elsewhere = await within(article).findByText('Properties (3)');
    act(() => elsewhere.focus());
    answer.release();
    await within(article).findByText('Part 2 of the text');
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(document.activeElement).toBe(elsewhere);
  });

  it('leaves focus where the person moved it when the continuation fails', async () => {
    const answer = held(() => fail(500, 'internal', 'storage stumbled'));
    const parts = partsOf(3);
    mockApi(
      explorerHandlers({
        [READ]: (body: Json) => (body.cursor === null ? parts(body) : answer.handler(body)),
      }),
    );
    const article = await openNote();
    const button = within(article).getByRole('button', { name: 'Continue reading' });
    act(() => button.focus());
    answer.hold();
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(answer.waiting()).toBe(1));
    const elsewhere = await within(article).findByText('Properties (3)');
    act(() => elsewhere.focus());
    answer.release();
    await within(article).findByText(
      'Continuing this reading: Something went wrong in the service.',
    );
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(document.activeElement).toBe(elsewhere);
  });

  it('moves focus to each part it loads while focus stays on the button, the last one too', async () => {
    mockApi(explorerHandlers({ [READ]: partsOf(3) }));
    const article = await openNote();
    for (const number of [2, 3]) {
      within(article).getByRole('button', { name: 'Continue reading' }).focus();
      await userEvent.keyboard('{Enter}');
      const part = await within(article).findByRole('group', {
        name: `Part ${number} of this reading`,
      });
      await waitFor(() => expect(document.activeElement).toBe(part));
    }
    expect(within(article).queryByRole('button', { name: 'Continue reading' })).toBeNull();
    expect(within(article).getByText('Part 3 read.').getAttribute('role')).toBe('status');
  });

  it('moves focus to the part a pointer press loaded, the press having left focus on the body', async () => {
    mockApi(explorerHandlers({ [READ]: partsOf(2) }));
    const article = await openNote();
    // WebKit on macOS does not focus a button it presses: focus stays on the body.
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    within(article).getByRole('button', { name: 'Continue reading' }).click();
    const part = await within(article).findByRole('group', { name: 'Part 2 of this reading' });
    await waitFor(() => expect(document.activeElement).toBe(part));
  });

  it('keeps focus on the button through a failed continuation, and the retry moves it to the part', async () => {
    let failures = 1;
    const parts = partsOf(2);
    mockApi(
      explorerHandlers({
        [READ]: (body: Json): Reply | Promise<Reply> => {
          if (body.cursor === 'part-2' && failures > 0) {
            failures -= 1;
            return fail(500, 'internal', 'storage stumbled');
          }
          return parts(body);
        },
      }),
    );
    const article = await openNote();
    const button = within(article).getByRole('button', { name: 'Continue reading' });
    button.focus();
    await userEvent.keyboard('{Enter}');
    await within(article).findByText(
      'Continuing this reading: Something went wrong in the service.',
    );
    await waitFor(() => expect(button.getAttribute('aria-disabled')).toBe('false'));
    expect(document.activeElement).toBe(button);
    expect(within(article).getByText('The rest of this reading could not be read.')).toBeTruthy();
    within(article).getByRole('button', { name: 'Try again' }).focus();
    await userEvent.keyboard('{Enter}');
    const part = await within(article).findByRole('group', { name: 'Part 2 of this reading' });
    await waitFor(() => expect(document.activeElement).toBe(part));
  });

  it('continues a pinned revision at that revision, focus on the part it loaded', async () => {
    const api = mockApi(explorerHandlers({ [READ]: partsOf(2, OLD) }));
    const article = await openNote(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    within(article).getByRole('button', { name: 'Continue reading' }).focus();
    await userEvent.keyboard('{Enter}');
    const part = await within(article).findByRole('group', { name: 'Part 2 of this reading' });
    await waitFor(() => expect(document.activeElement).toBe(part));
    const reads = api.callsTo(READ).filter((body) => body.view === 'text');
    expect(reads.map((body) => body.at)).toEqual([
      { kind: 'revision', revision: OLD },
      { kind: 'revision', revision: OLD },
    ]);
    expect(api.violations).toEqual([]);
  });
});
