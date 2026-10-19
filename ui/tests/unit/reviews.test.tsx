/**
 * An item's reviews beside it: coverage said exactly as the service computed it for the revision
 * read (SPEC section 8: "Later changes display previous coverage accurately instead of extending
 * the badge. Imported human-looking metadata is not authenticated review evidence"), Verify
 * through a revision-bound confirmation that shows what it covers ("Human confirmation requires
 * an explicit revision-bound browser action"), who may verify, an archived workspace, and the
 * coverage kept current by the workspace's events.
 */
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Workspace } from '../../src/api/generated/types.gen';
import { formatInstant } from '../../src/lib/instant';
import { fail, type Handler, mockApi, ok, type Reply } from './support/api';
import { eventOf, mockEvents } from './support/imports';
import {
  CONFIRM,
  CONFIRMATION_ID,
  confirmationFor,
  REVIEW,
  REVIEWED_AT,
  REVIEWS,
  reviewOf,
  reviewsAnswer,
} from './support/reviews';
import {
  DIGEST,
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  renderAt,
  WS,
  workspace,
} from './support/workspace';

const OPEN = '/api/workspaces/open-workspace';
const ITEM = '/api/items/get-item';

configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** A person who may verify items here. */
const reviewer: Workspace = { ...workspace, permissions: ['read', 'review'] };

function setup(overrides: Record<string, Handler> = {}, who = reviewer) {
  return mockApi(explorerHandlers({ [OPEN]: () => ok(who), ...overrides }));
}

async function reviewsPanel(url = `/w/${WS}/i/${NOTE}`) {
  renderAt(url);
  return screen.findByRole('region', { name: 'Reviews' });
}

describe('coverage beside the item', { timeout: 30_000 }, () => {
  it('asks for the reviews at the revision read and says when none is recorded', async () => {
    const api = setup({ [REVIEWS]: () => ok(reviewsAnswer([])) });
    const panel = await reviewsPanel(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    await within(panel).findByText('Not verified: no review of this item is recorded.');
    expect(api.callsTo(REVIEWS)).toEqual([
      { workspace_id: WS, item_id: NOTE, at: { kind: 'revision', revision: OLD } },
    ]);
  });

  it('says Verified only for a review that covers the revision read, with who and when in local time', async () => {
    setup({ [REVIEWS]: () => ok(reviewsAnswer([reviewOf('current')])) });
    const panel = await reviewsPanel();
    const badge = await within(panel).findByText('Verified at this revision');
    expect(badge.closest('p')?.textContent).toContain(`by grace on ${formatInstant(REVIEWED_AT)}`);
  });

  it('shows a review of an earlier revision as earlier, never as covering what is shown', async () => {
    setup({ [REVIEWS]: () => ok(reviewsAnswer([reviewOf('changed', OLD)])) });
    const panel = await reviewsPanel();
    await within(panel).findByText(/^Not verified at this revision\./);
    expect(within(panel).queryByText('Verified at this revision')).toBeNull();
    const row = within(panel).getByRole('listitem');
    expect(row.textContent).toContain('Covers an earlier revision only');
    expect(row.textContent).toContain(`revision ${OLD.slice(0, 10)}`);
  });

  it('labels a claim the file brought with it as imported, never as Verified', async () => {
    setup({
      [REVIEWS]: () => ok(reviewsAnswer([], [{ by: 'Dr. Smith', at: '2024-03-01' }])),
    });
    const panel = await reviewsPanel();
    await within(panel).findByText(/^Not verified here\./);
    expect(within(panel).queryByText('Verified at this revision')).toBeNull();
    const claim = within(panel).getByRole('listitem');
    expect(claim.textContent).toContain('Dr. Smith');
    // The file's own words, as written: not converted to a local time.
    expect(claim.textContent).toContain('2024-03-01');
    expect(claim.textContent).toContain('imported with the file, not a review recorded here');
  });

  it('says a review a purge invalidated is invalidated', async () => {
    setup({ [REVIEWS]: () => ok(reviewsAnswer([reviewOf('invalidated', OLD)])) });
    const panel = await reviewsPanel();
    const row = await within(panel).findByRole('listitem');
    expect(row.textContent).toContain('Invalidated by a purge');
    expect(within(panel).queryByText('Verified at this revision')).toBeNull();
  });
});

describe('Verify', { timeout: 30_000 }, () => {
  /** Answers for a whole Verify, recording what each step was sent. */
  function verifying(overrides: Record<string, Handler> = {}) {
    let recorded = false;
    const api = setup({
      [REVIEWS]: () => ok(reviewsAnswer(recorded ? [reviewOf('current')] : [])),
      [CONFIRM]: (body) => ok(confirmationFor(body)),
      [REVIEW]: () => {
        recorded = true;
        return ok(reviewOf('current'));
      },
      ...overrides,
    });
    return api;
  }

  /** Open Verify only after the actual item query has supplied what the dialog must cover. */
  async function openCurrentVerification() {
    const panel = await screen.findByRole('region', { name: 'Reviews' });
    const button = await within(panel).findByRole('button', { name: 'Verify\u2026' });
    await waitFor(() => expect(button.getAttribute('aria-disabled')).toBe('false'));
    fireEvent.click(button);
    return screen.findByRole('alertdialog');
  }

  it.each([
    ['another item at the same head', OTHER, HEAD],
    ['another revision of this item', NOTE, OLD],
  ] as const)(
    'keeps a completed Verify announcement with its identity, not %s',
    async (_case, itemId, revision) => {
      // SPEC section 8: a review covers the exact displayed item and revision. A's successful
      // mutation message cannot become an assertion about B merely because both cite HEAD.
      const api = verifying({ [REVIEWS]: () => ok(reviewsAnswer([])) });
      const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
      const panel = await screen.findByRole('region', { name: 'Reviews' });
      const dialog = await openCurrentVerification();
      fireEvent.click(within(dialog).getByRole('button', { name: 'Verify this revision' }));
      await within(panel).findByText(`Your review of revision ${HEAD.slice(0, 10)} is recorded.`);
      await act(() =>
        router.navigate({
          to: '/w/$workspaceId/i/$itemId',
          params: { workspaceId: WS, itemId },
          search: { rev: revision },
        }),
      );
      await waitFor(() =>
        expect(
          api
            .callsTo(REVIEWS)
            .some(
              (body) =>
                body.item_id === itemId && (body.at as { revision: string }).revision === revision,
            ),
        ).toBe(true),
      );
      const current = await screen.findByRole('region', { name: 'Reviews' });
      expect(within(current).queryByText(/^Your review of revision/)).toBeNull();
      expect(screen.queryByRole('alertdialog')).toBeNull();
      expect(api.violations).toEqual([]);
    },
  );

  it('does not show a failed Verify of A inside the dialog for B at the same revision', async () => {
    const api = verifying({
      [REVIEWS]: () => ok(reviewsAnswer([])),
      [REVIEW]: () => fail(503, 'unavailable', 'A could not be verified'),
    });
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    const first = await openCurrentVerification();
    fireEvent.click(within(first).getByRole('button', { name: 'Verify this revision' }));
    expect((await within(first).findByRole('alert')).textContent).toContain(
      'A could not be verified',
    );
    fireEvent.click(within(first).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
        search: { rev: HEAD },
      }),
    );
    await waitFor(() =>
      expect(api.callsTo(ITEM).some((body) => body.item_id === OTHER)).toBe(true),
    );
    const second = await openCurrentVerification();
    expect(
      screen.getByRole('alertdialog', { name: `Verify Plan at revision ${HEAD.slice(0, 10)}?` }),
    ).toBe(second);
    expect(within(second).queryByRole('alert')).toBeNull();
    expect(within(second).queryByText('The service said: A could not be verified')).toBeNull();
    expect(api.callsTo(REVIEW)).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it("does not let a late Verify completion for A announce success or close B's dialog", async () => {
    // Browser history can change the active item while a request is pending. The old request
    // may still finish (mutations are durable); its completion belongs only to its original item.
    let finish: ((reply: Reply) => void) | undefined;
    const api = verifying({
      [REVIEWS]: () => ok(reviewsAnswer([])),
      [REVIEW]: () =>
        new Promise<Reply>((resolve) => {
          finish = resolve;
        }),
    });
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    const first = await openCurrentVerification();
    fireEvent.click(within(first).getByRole('button', { name: 'Verify this revision' }));
    await waitFor(() => expect(finish).toBeTypeOf('function'));
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
        search: { rev: HEAD },
      }),
    );
    await waitFor(() =>
      expect(api.callsTo(ITEM).some((body) => body.item_id === OTHER)).toBe(true),
    );
    expect(screen.queryByRole('alertdialog')).toBeNull();
    const second = await openCurrentVerification();
    expect(
      screen.getByRole('alertdialog', { name: `Verify Plan at revision ${HEAD.slice(0, 10)}?` }),
    ).toBe(second);
    await act(async () => {
      finish?.(ok(reviewOf('current')));
    });
    await waitFor(() => expect(screen.queryByText(/^Your review of revision/)).toBeNull());
    expect(screen.getByRole('alertdialog')).toBe(second);
    expect(
      within(second)
        .getByRole('button', { name: 'Verify this revision' })
        .getAttribute('aria-disabled'),
    ).toBe('false');
    expect(api.callsTo(REVIEW)).toHaveLength(1);
    expect(api.violations).toEqual([]);
  });

  it('asks in a dialog that shows the revision and the exact content, then confirms that revision and records the review', async () => {
    const api = verifying();
    const panel = await reviewsPanel();
    await within(panel).findByText('Not verified: no review of this item is recorded.');
    const verify = within(panel).getByRole('button', { name: 'Verify…' });
    act(() => verify.focus());
    fireEvent.click(verify);
    const dialog = await screen.findByRole('alertdialog', {
      name: `Verify Meeting notes at revision ${HEAD.slice(0, 10)}?`,
    });
    // Focus is in the dialog.
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    expect(dialog.textContent).toContain(`revision ${HEAD.slice(0, 10)}`);
    expect(dialog.textContent).toContain(DIGEST.slice(0, 10));
    // The exact text the review will cover.
    const text = within(dialog).getByRole('textbox', { name: 'The text you verify' });
    expect(text).toBeInstanceOf(HTMLTextAreaElement);
    expect((text as HTMLTextAreaElement).value).toBe('# Meeting notes');
    expect(text.hasAttribute('readonly')).toBe(true);
    expect(api.callsTo(CONFIRM)).toEqual([]);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Verify this revision' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    const confirm = api.callsTo(CONFIRM);
    expect(confirm).toHaveLength(1);
    expect(confirm.at(0)).toMatchObject({
      workspace_id: WS,
      action: 'review',
      target: { kind: 'item', item_id: NOTE },
      revision: HEAD,
      content_digest: DIGEST,
    });
    const review = api.callsTo(REVIEW);
    expect(review).toHaveLength(1);
    expect(review.at(0)).toMatchObject({
      source: {
        workspace_id: WS,
        item_id: NOTE,
        path: noteSummary.path,
        revision: HEAD,
        selection: { kind: 'all' },
      },
      content_digest: DIGEST,
      confirmation_id: CONFIRMATION_ID,
    });
    expect(review.at(0)?.idempotency_key).not.toBe(confirm.at(0)?.idempotency_key);
    // The coverage is asked again, and says what the review now covers.
    await within(panel).findByText('Verified at this revision');
    await within(panel).findByText(`Your review of revision ${HEAD.slice(0, 10)} is recorded.`);
    // Focus is back on what opened the dialog.
    await waitFor(() => expect(document.activeElement).toBe(verify));
    expect(api.violations).toEqual([]);
  });

  it('gives focus back to Verify when the dialog is cancelled, and sends nothing', async () => {
    const api = verifying();
    const panel = await reviewsPanel();
    const verify = await within(panel).findByRole('button', { name: 'Verify…' });
    act(() => verify.focus());
    fireEvent.click(verify);
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(verify));
    expect(api.callsTo(CONFIRM)).toEqual([]);
    expect(api.callsTo(REVIEW)).toEqual([]);
  });

  it('repeats both steps under their own keys when the review failed, so the person is not asked twice', async () => {
    let tries = 0;
    const api = verifying({
      [REVIEW]: () => {
        tries += 1;
        return tries === 1 ? fail(503, 'unavailable', 'busy') : ok(reviewOf('current'));
      },
    });
    const panel = await reviewsPanel();
    fireEvent.click(await within(panel).findByRole('button', { name: 'Verify…' }));
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Verify this revision' }));
    const retry = await within(dialog).findByRole('button', { name: 'Try again' });
    fireEvent.click(retry);
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    const confirmKeys = api.callsTo(CONFIRM).map((body) => body.idempotency_key);
    const reviewKeys = api.callsTo(REVIEW).map((body) => body.idempotency_key);
    expect(reviewKeys).toHaveLength(2);
    expect(reviewKeys.at(1)).toBe(reviewKeys.at(0));
    expect(new Set(confirmKeys).size).toBe(1);
  });

  it('is not offered to a person who may not verify, and says why', async () => {
    setup({ [REVIEWS]: () => ok(reviewsAnswer([])) }, workspace);
    const panel = await reviewsPanel();
    await within(panel).findByText('Not verified: no review of this item is recorded.');
    expect(within(panel).queryByRole('button', { name: 'Verify…' })).toBeNull();
    expect(
      within(panel).getByText(
        'Verifying this item needs permission to verify items in this workspace, which you do not have.',
      ),
    ).toBeDefined();
  });

  it('stays offered in an archived workspace, says why it cannot run, and sends nothing', async () => {
    const archivedAt = '2026-01-01T08:00:00.000Z';
    const api = setup(
      { [REVIEWS]: () => ok(reviewsAnswer([])) },
      { ...reviewer, archived_at: archivedAt },
    );
    const panel = await reviewsPanel();
    const verify = await within(panel).findByRole('button', { name: 'Verify…' });
    expect(verify.getAttribute('aria-disabled')).toBe('true');
    const described = document.getElementById(verify.getAttribute('aria-describedby') ?? '');
    expect(described?.textContent).toContain('This workspace was archived on');
    fireEvent.click(verify);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(api.callsTo(CONFIRM)).toEqual([]);
  });

  it('cannot be confirmed before the item’s content is read', async () => {
    let answer: (() => void) | null = null;
    const api = verifying({
      [ITEM]: () =>
        new Promise((resolve) => {
          answer = () => resolve(ok(itemDocument(noteSummary)));
        }),
    });
    const panel = await reviewsPanel();
    const verify = await within(panel).findByRole('button', { name: 'Verify…' });
    expect(verify.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(verify);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    act(() => answer?.());
    await waitFor(() => expect(verify.getAttribute('aria-disabled')).toBe('false'));
    expect(api.callsTo(CONFIRM)).toEqual([]);
  });
});

describe('kept current by the workspace’s events', { timeout: 30_000 }, () => {
  it('asks again when the stream opens and when an item’s review is recorded, not for another item’s', async () => {
    const api = setup({ [REVIEWS]: () => ok(reviewsAnswer([])) });
    const events = mockEvents();
    const panel = await reviewsPanel();
    await within(panel).findByText('Not verified: no review of this item is recorded.');
    await waitFor(() => expect(events.streams).toHaveLength(1));
    // Subscribe, then ask: once open, the reviews are asked again.
    await waitFor(() => expect(api.callsTo(REVIEWS).length).toBeGreaterThanOrEqual(2));
    const before = api.callsTo(REVIEWS).length;
    act(() =>
      events.streams
        .at(0)
        ?.send('1', eventOf('reviewed', '1', { item_id: 'aaaaaaaa-0000-4000-8000-000000000000' })),
    );
    act(() => events.streams.at(0)?.send('2', eventOf('job_updated', '2')));
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(api.callsTo(REVIEWS)).toHaveLength(before);
    act(() => events.streams.at(0)?.send('3', eventOf('reviewed', '3', { item_id: NOTE })));
    await waitFor(() => expect(api.callsTo(REVIEWS)).toHaveLength(before + 1));
  });

  it('keeps what it showed when a read after an event fails, with the failure beside it', async () => {
    let failing = false;
    setup({
      [REVIEWS]: () =>
        failing ? fail(503, 'unavailable', 'busy') : ok(reviewsAnswer([reviewOf('current')])),
    });
    const events = mockEvents();
    const panel = await reviewsPanel();
    await within(panel).findByText('Verified at this revision');
    await waitFor(() => expect(events.streams).toHaveLength(1));
    failing = true;
    act(() => events.streams.at(0)?.send('1', eventOf('reviewed', '1', { item_id: NOTE })));
    const alert = await within(panel).findByRole('alert');
    expect(alert.textContent).toContain('Reading this item’s reviews again');
    expect(within(panel).getByText('Verified at this revision')).toBeDefined();
  });
});
