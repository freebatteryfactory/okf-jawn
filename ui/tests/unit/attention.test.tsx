/**
 * Attention: the advisory warnings of a workspace, in one list, as a mark on the items they name
 * and in the reader. The words always say they are advisory, and an empty list never reads as
 * safe.
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
import type { AttentionItem } from '../../src/api/generated/types.gen';
import { SHOWN_STEP } from '../../src/features/attention/AttentionPage';
import {
  ADVISORY,
  ATTENTION_AUTO_PAGES,
  ATTENTION_PAGE,
  attentionOptions,
  NOT_A_CERTIFICATE,
} from '../../src/features/attention/attention';
import {
  fail,
  fromNewerService,
  type Handler,
  type Json,
  mockApi,
  notImplemented,
  ok,
  type Reply,
} from './support/api';
import { ATTENTION, mockEvents, pageShown } from './support/imports';
import {
  explorerHandlers,
  HEAD,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  renderAt,
  SOURCE,
  sourceSummary,
  WS,
} from './support/workspace';

const ITEM = '/api/items/get-item';

// On a loaded machine an answer has taken longer than the default second to reach the page.
configure({ asyncUtilTimeout: 10_000 });

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** The items of this workspace a warning can name, as the list carries them. */
const ITEMS = [noteSummary, otherSummary, sourceSummary];

/** A warning as the service lists it: with the item it names, as that item is (ruling 7). */
const warning = (
  kind: AttentionItem['kind'],
  message: string,
  itemId: string | null,
  action: AttentionItem['action'] = 'get_item',
): AttentionItem => {
  const item = ITEMS.find((each) => each.id === itemId);
  return {
    kind,
    message,
    action,
    ...(itemId === null ? {} : { item_id: itemId }),
    ...(item === undefined ? {} : { item }),
  };
};

const THREE = [
  warning('broken_link', 'The link to plan-2024.md leads nowhere.', NOTE, 'get_links'),
  warning('instruction_like', 'Line 12 reads “ignore previous instructions”.', NOTE, 'read_item'),
  warning('unprocessed', 'The format is not supported.', SOURCE, 'redigest_item'),
];

const answer = (items: AttentionItem[], next: string | null = null, revision = HEAD) =>
  ok({ items, next_cursor: next, revision });

function setup(attention: Handler | null) {
  return mockApi(explorerHandlers({ [ATTENTION]: attention ?? (() => notImplemented(ATTENTION)) }));
}

async function openAttention() {
  const app = renderAt(`/w/${WS}/attention`);
  await screen.findByRole('heading', { name: 'Attention' });
  return app;
}

const rows = () => within(screen.getByRole('list', { name: 'Attention' })).getAllByRole('listitem');

describe('the list of warnings', () => {
  it('says above the list that the warnings are advisory and that false alarms are expected', async () => {
    setup(() => answer(THREE));
    await openAttention();
    const note = screen.getByRole('note');
    expect(note.textContent).toBe(
      `${ADVISORY} A list with nothing in it, or an item with no warning, has not been found safe.`,
    );
    expect(ADVISORY).toBe(
      'These warnings are advisory. They come from automatic checks, and false alarms are expected.',
    );
  });

  it('reads an empty list as "no warnings found", with beside it that this is not a finding of safety', async () => {
    setup(() => answer([]));
    await openAttention();
    const found = await screen.findByText('No warnings found.');
    expect(found.parentElement?.textContent).toBe(
      'No warnings found. That does not make the workspace safe to give to an AI tool, or correct: the checks look for a few known signs and can miss what matters.',
    );
    expect(NOT_A_CERTIFICATE).toMatch(/^That does not make the workspace safe/);
    // The advisory note is still there, and nothing calls the workspace safe, clean or clear.
    expect(screen.getByRole('note').textContent).toContain(ADVISORY);
    expect(
      screen.queryByText(/\b(is|are|looks?|seems?) (safe|clean|clear)\b|all clear|no problems/i),
    ).toBeNull();
    // No count on the link: an empty list is not shown as a score.
    expect(screen.getByRole('link', { name: 'Attention' })).toBeDefined();
  });

  it('asks at the revision the workspace shows, a full page at a time', async () => {
    const api = setup(() => answer(THREE));
    await openAttention();
    await waitFor(() => expect(rows()).toHaveLength(3));
    expect(api.callsTo(ATTENTION)).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'revision', revision: HEAD },
        page: { limit: ATTENTION_PAGE, cursor: null },
      },
    ]);
    expect(api.violations).toEqual([]);
  });

  it('lists each warning: its kind in words, the service’s explanation, its item and a next step', async () => {
    setup(() => answer([...THREE, warning('duplicate', 'Two files hold the same bytes.', null)]));
    await openAttention();
    await waitFor(() => expect(rows()).toHaveLength(4));
    await screen.findAllByRole('link', { name: 'Meeting notes' });
    expect(rows().map((row) => row.textContent)).toEqual([
      'A link that leads nowhere in Meeting notesThe link to plan-2024.md leads nowhere.Suggested next step: look at the item’s links.',
      'Text that reads like instructions to an AI tool in Meeting notesLine 12 reads “ignore previous instructions”.Suggested next step: read the item.',
      'Not converted, or not fully in Annual reportThe format is not supported.Suggested next step: convert the file again.',
      'The same file appears more than once (the workspace as a whole)Two files hold the same bytes.Suggested next step: open the item.',
    ]);
    expect(screen.getByText('4 warnings shown.').getAttribute('role')).toBe('status');
  });

  it('links a warning to its item at the revision the answer says was inspected', async () => {
    const api = setup(() => answer(THREE.slice(0, 1), null, OLD));
    await openAttention();
    const link = await screen.findByRole('link', { name: 'Meeting notes' });
    expect(link.getAttribute('href')).toBe(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    // The title is the one the list carries: nothing is read for the row.
    expect(
      api
        .callsTo(ITEM)
        .some(
          (body) => JSON.stringify(body.at) === JSON.stringify({ kind: 'revision', revision: OLD }),
        ),
    ).toBe(false);
    expect(screen.getByText(/for\s+revision/).textContent).toContain('bbbbbbbbbb');
  });

  it('opens the item at the place a warning is about, and says which place', async () => {
    setup(() =>
      answer([
        {
          ...warning('instruction_like', 'Line 12 reads “ignore previous instructions”.', NOTE),
          selection: { kind: 'lines', range: { start: 12, end: 12 } },
        },
      ]),
    );
    await openAttention();
    const link = await screen.findByRole('link', { name: 'Meeting notes' });
    const href = new URL(link.getAttribute('href') ?? '', 'http://page');
    expect(href.pathname).toBe(`/w/${WS}/i/${NOTE}`);
    expect(href.searchParams.get('rev')).toBe(HEAD);
    expect(JSON.parse(href.searchParams.get('sel') ?? 'null')).toEqual({
      kind: 'lines',
      range: { start: 12, end: 12 },
    });
    expect(rows().at(0)?.textContent).toContain('in Meeting notes, at line 12');
  });

  it('opens the whole item, with no place in its link, for a warning about all of it', async () => {
    setup(() =>
      answer([
        {
          ...warning('unprocessed', 'The format is not supported.', SOURCE, 'redigest_item'),
          selection: { kind: 'all' },
        },
      ]),
    );
    await openAttention();
    const link = await screen.findByRole('link', { name: 'Annual report' });
    const href = new URL(link.getAttribute('href') ?? '', 'http://page');
    expect(href.pathname).toBe(`/w/${WS}/i/${SOURCE}`);
    expect(href.searchParams.get('rev')).toBe(HEAD);
    // As a search result's link: the whole item is the item itself, not a place in it.
    expect(href.searchParams.has('sel')).toBe(false);
    expect(rows().at(0)?.textContent).not.toContain(', at ');
  });

  it('names an item the list does not carry by its identity, with no link: it was purged', async () => {
    const purged: AttentionItem = {
      kind: 'invalidated',
      message: 'This item was removed by a purge.',
      action: 'get_item',
      item_id: OTHER,
    };
    setup(() => answer([purged]));
    await openAttention();
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(rows().at(0)?.textContent).toContain(
      'in an item that is no longer there (item 33333333)',
    );
    expect(within(rows().at(0) ?? document.body).queryByRole('link')).toBeNull();
  });

  it('links uploads waiting to be imported to the uploads the service holds', async () => {
    setup(() =>
      answer([
        warning(
          'uploads_waiting',
          '3 uploads are complete and not imported (2 MB).',
          null,
          'list_uploads',
        ),
      ]),
    );
    await openAttention();
    const link = await screen.findByRole('link', {
      name: 'See the uploads waiting to be imported',
    });
    expect(link.getAttribute('href')).toBe(`/w/${WS}/import#uploads-held`);
    expect(rows().at(0)?.textContent).toContain('Uploaded files waiting to be imported');
    expect(rows().at(0)?.textContent).not.toContain('Suggested next step');
  });

  it('says that a warning is of a kind this page does not know, by the service’s name for it', async () => {
    const odd: AttentionItem = {
      ...warning('stale', 'A macro runs when the file is opened.', NOTE),
      kind: fromNewerService('poisoned_macro'),
    };
    setup(() => answer([odd]));
    await openAttention();
    const row = (await screen.findByText('A macro runs when the file is opened.')).closest('li');
    expect(row?.querySelector('strong')?.textContent).toBe(
      'A warning of a kind this page does not know (“poisoned_macro”)',
    );
    // In the reader's notice too.
    cleanup();
    setup(() => answer([odd]));
    renderAt(`/w/${WS}/i/${NOTE}`);
    const notice = (await screen.findByText('Attention:')).closest('[role="note"]');
    expect(notice?.textContent).toContain(
      'A warning of a kind this page does not know (“poisoned_macro”): A macro runs when the file is opened.',
    );
    expect(document.body.textContent).not.toContain('undefined');
  });

  it('names a suggested operation it has no words for as the wire names it', async () => {
    setup(() => answer([warning('stale', 'Marked to be checked by June.', NOTE, 'list_items')]));
    await openAttention();
    await screen.findByRole('link', { name: 'Meeting notes' });
    expect(rows().at(0)?.textContent).toContain('Due for a check in Meeting notes');
    expect(rows().at(0)?.textContent).toContain('Suggested next step: the operation “list_items”.');
  });

  it('shows a failed listing in plain words with its retry, never as "no warnings"', async () => {
    let answers = 0;
    setup(() => {
      answers += 1;
      return answers === 1 ? fail(500, 'internal', 'the index could not be read') : answer(THREE);
    });
    await openAttention();
    await screen.findByText(
      'Listing this workspace’s warnings: Something went wrong in the service.',
    );
    expect(screen.queryByText('No warnings found.')).toBeNull();
    const again = screen.getByRole('button', { name: 'Try again' });
    again.focus();
    fireEvent.click(again);
    await waitFor(() => expect(rows()).toHaveLength(3));
    expect(document.activeElement).toBe(rows().at(0));
  });

  it('reads "not available in this build" where Attention is not built, never as an empty list', async () => {
    setup(null);
    await openAttention();
    await screen.findByText('Listing this workspace’s warnings: Not available in this build.');
    expect(screen.queryByText('No warnings found.')).toBeNull();
    expect(screen.getByRole('link', { name: 'Attention' })).toBeDefined();
  });

  it('asks again on "Check again"', async () => {
    let list = THREE;
    const api = setup(() => answer(list));
    await openAttention();
    await waitFor(() => expect(rows()).toHaveLength(3));
    list = THREE.slice(0, 1);
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(api.callsTo(ATTENTION)).toHaveLength(2);
    expect(screen.getByText('1 warning shown.')).toBeDefined();
  });
});

describe('a check that fails after the list was read', () => {
  it('keeps the warnings and their marks on screen, with the failure beside them', async () => {
    let fails = false;
    setup(() => (fails ? fail(500, 'internal', 'the index could not be read') : answer(THREE)));
    await openAttention();
    await waitFor(() => expect(rows()).toHaveLength(3));
    fails = true;
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    await screen.findByText('Listing the warnings again: Something went wrong in the service.');
    expect(rows()).toHaveLength(3);
    expect(screen.getByText('3 warnings shown.')).toBeDefined();
    expect(screen.getByRole('link', { name: 'Attention (3)' })).toBeDefined();
  });

  it('keeps an empty answer as it was answered, with the failure beside it', async () => {
    let fails = false;
    setup(() => (fails ? fail(500, 'internal', 'the index could not be read') : answer([])));
    await openAttention();
    await screen.findByText('No warnings found.');
    fails = true;
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
    await screen.findByText('Listing the warnings again: Something went wrong in the service.');
    expect(screen.queryByText('No warnings found.')).toBeNull();
    expect(screen.getByText('Previous check found no warnings.')).toBeDefined();
    expect(screen.getByText(/Showing the previous check/)).toBeDefined();
  });
});

describe('a long list', () => {
  /** Six pages of twenty warnings; each names the note. */
  const PAGES = 6;
  const PER_PAGE = 20;
  const paged: Handler = (body) => {
    const cursor = (body.page as Json).cursor;
    const page = typeof cursor === 'string' ? Number(cursor) : 0;
    const items = Array.from({ length: PER_PAGE }, (_, index) =>
      warning('missing_description', `Warning ${page * PER_PAGE + index + 1}`, NOTE),
    );
    return answer(items, page + 1 < PAGES ? String(page + 1) : null);
  };
  const listedByItself = ATTENTION_AUTO_PAGES * PER_PAGE;

  it('asks for the first pages by itself and no more, and says that more exist', async () => {
    const api = setup(paged);
    await openAttention();
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(ATTENTION_AUTO_PAGES));
    await screen.findByRole('link', { name: `Attention (${listedByItself}+)` });
    // Give a further page every chance to be asked for: none is.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(api.callsTo(ATTENTION)).toHaveLength(ATTENTION_AUTO_PAGES);
    expect(api.callsTo(ATTENTION).map((body) => (body.page as Json).cursor)).toEqual([
      null,
      '1',
      '2',
      '3',
      '4',
    ]);
    expect(rows()).toHaveLength(SHOWN_STEP);
    expect(
      screen.getByText(
        `${SHOWN_STEP} warnings shown; more exist. The marks in the folders and on the tabs cover the warnings listed so far.`,
      ),
    ).toBeDefined();
  });

  it('does not scan on idle time or new observers, then rechecks explicitly when the person returns', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const api = setup(paged);
    renderAt(`/w/${WS}`);
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(ATTENTION_AUTO_PAGES));
    // Ten minutes on, well past the time other lists count as fresh.
    await act(() => vi.advanceTimersByTimeAsync(600_000));
    // An item is opened: its tab and its reader read the same list.
    fireEvent.click(await screen.findByRole('treeitem', { name: /Annual report/ }));
    await screen.findByRole('tab', { name: 'Annual report' });
    expect(api.callsTo(ATTENTION)).toHaveLength(ATTENTION_AUTO_PAGES);
    // The person comes back to the window.
    act(() => {
      window.dispatchEvent(new Event('focus'));
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await act(() => vi.advanceTimersByTimeAsync(5_000));
    // A return is a lifecycle check, including at the same head; its retained five pages refresh.
    expect(api.callsTo(ATTENTION)).toHaveLength(ATTENTION_AUTO_PAGES * 2);
    vi.useRealTimers();
  });

  it('asks anew at a new head', async () => {
    const NEW = 'e'.repeat(40);
    let head = HEAD;
    const api = mockApi(
      explorerHandlers({
        '/api/workspaces/open-workspace': () =>
          ok({
            id: WS,
            name: 'Workshop notes',
            description: 'Notes from the workshop',
            head,
            created_at: '2026-01-01T23:30:00.000Z',
            permissions: ['read'],
          }),
        [ATTENTION]: (body) => answer(THREE, null, (body.at as Json).revision as string),
      }),
    );
    renderAt(`/w/${WS}`);
    await screen.findByRole('link', { name: 'Attention (3)' });
    expect(api.callsTo(ATTENTION)).toHaveLength(1);
    head = NEW;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(2));
    expect(api.callsTo(ATTENTION).map((body) => (body.at as Json).revision)).toEqual([HEAD, NEW]);
  });

  it('asks once for a further page that fails, not again and again by itself', async () => {
    const api = setup((body) =>
      (body.page as Json).cursor === 'next'
        ? fail(500, 'internal', 'the checks stopped')
        : answer(THREE, 'next'),
    );
    await openAttention();
    await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(2));
    await screen.findByText(/Something went wrong in the service\./);
    // Left alone, the page asks no more: the failure has its own retry.
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(api.callsTo(ATTENTION)).toHaveLength(2);
    // The warnings that were read stay, and the link still says that more exist.
    expect(rows()).toHaveLength(3);
    expect(screen.getByRole('link', { name: 'Attention (3+)' })).toBeDefined();
  });

  it('brings more rows with "Show more warnings", then a further page, with focus on the first it brought', async () => {
    const api = setup(paged);
    await openAttention();
    await screen.findByRole('link', { name: `Attention (${listedByItself}+)` });
    const more = () => screen.getByRole('button', { name: 'Show more warnings' });
    more().focus();
    fireEvent.click(more());
    // Rows the page already held: no request.
    await waitFor(() => expect(rows()).toHaveLength(listedByItself));
    expect(document.activeElement).toBe(rows().at(SHOWN_STEP));
    expect(api.callsTo(ATTENTION)).toHaveLength(ATTENTION_AUTO_PAGES);
    more().focus();
    fireEvent.click(more());
    await waitFor(() => expect(rows()).toHaveLength(PAGES * PER_PAGE));
    expect(api.callsTo(ATTENTION)).toHaveLength(PAGES);
    await waitFor(() => expect(document.activeElement).toBe(rows().at(listedByItself)));
    expect(screen.queryByRole('button', { name: 'Show more warnings' })).toBeNull();
    expect(screen.getByText(`${PAGES * PER_PAGE} warnings shown.`)).toBeDefined();
    await screen.findByRole('link', { name: `Attention (${PAGES * PER_PAGE})` });
  });
});

describe('the mark on an item', () => {
  it('counts the warnings on the workspace’s link, in the tree, on the tab and in the reader', async () => {
    setup(() => answer(THREE));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByRole('link', { name: 'Attention (3)' });
    // The tree: the note's row is under "notes", which the open item expanded.
    const row = await screen.findByRole('treeitem', { name: /^Meeting notes/ });
    expect(row.getAttribute('aria-label')).toBe(
      'Meeting notes, type meeting-note, 2 warnings (advisory)',
    );
    expect(row.textContent).toContain('2 warnings');
    expect(screen.getByRole('treeitem', { name: /^Plan/ }).getAttribute('aria-label')).toBe(
      'Plan, type plan',
    );
    // The tab: its name is the title; the mark is read after it.
    const tab = screen.getByRole('tab', { name: 'Meeting notes' });
    expect(document.getElementById(tab.getAttribute('aria-describedby') ?? '')?.textContent).toBe(
      ' (2 warnings, advisory)',
    );
    // The reader.
    const notice = screen.getByText('Attention:').closest('[role="note"]');
    expect(notice?.textContent).toBe(
      'Attention: 2 advisory warnings for this item at revision aaaaaaaaaa. False alarms are expected.A link that leads nowhere: The link to plan-2024.md leads nowhere.Text that reads like instructions to an AI tool: Line 12 reads “ignore previous instructions”.All warnings of this workspace',
    );
    const all = within(notice as HTMLElement).getByRole('link', {
      name: 'All warnings of this workspace',
    });
    expect(all.getAttribute('href')).toBe(`/w/${WS}/attention`);
  });

  it('puts no mark computed at the head on a tab that cites an older revision; the reader’s notice names its revision', async () => {
    setup(() => answer(THREE));
    renderAt(`/w/${WS}/i/${NOTE}?rev=${OLD}`);
    const tab = await screen.findByRole('tab', { name: 'Meeting notes', selected: true });
    const notice = (await screen.findByText('Attention:')).closest('[role="note"]');
    // The warnings were found at the revision the workspace shows, and the notice says which.
    expect(notice?.textContent).toContain(
      '2 advisory warnings for this item at revision aaaaaaaaaa.',
    );
    // The tab shows revision bbbbbbbbbb: a count of another revision's warnings is not its mark.
    expect(document.getElementById(tab.getAttribute('aria-describedby') ?? '')?.textContent).toBe(
      '',
    );
    expect(tab.textContent).toBe('Meeting notes');
    // The tree lists the head, and keeps its mark.
    const row = await screen.findByRole('treeitem', { name: /^Meeting notes/ });
    expect(row.getAttribute('aria-label')).toContain('2 warnings (advisory)');
  });

  it('keeps the mark on a tab that cites the very revision the workspace shows', async () => {
    setup(() => answer(THREE));
    renderAt(`/w/${WS}/i/${NOTE}?rev=${HEAD}`);
    const tab = await screen.findByRole('tab', { name: 'Meeting notes', selected: true });
    await waitFor(() =>
      expect(document.getElementById(tab.getAttribute('aria-describedby') ?? '')?.textContent).toBe(
        ' (2 warnings, advisory)',
      ),
    );
  });

  it('carries the source’s one warning beside its conversion mark', async () => {
    setup(() => answer(THREE));
    renderAt(`/w/${WS}/i/${SOURCE}`);
    const row = await screen.findByRole('treeitem', { name: /1 warning/ });
    expect(row.getAttribute('aria-label')).toBe(
      'Annual report, type report, partly converted, 1 warning (advisory)',
    );
    const tab = screen.getByRole('tab', { name: 'Annual report' });
    await waitFor(() =>
      expect(document.getElementById(tab.getAttribute('aria-describedby') ?? '')?.textContent).toBe(
        ' (partly converted) (1 warning, advisory)',
      ),
    );
    expect(screen.getByText('Attention:').closest('[role="note"]')?.textContent).toContain(
      'Attention: 1 advisory warning for this item',
    );
  });

  it('shows no mark and no notice for an item no warning names, without calling it safe', async () => {
    setup(() => answer(THREE));
    renderAt(`/w/${WS}/i/${OTHER}`);
    await screen.findByRole('link', { name: 'Attention (3)' });
    await screen.findByText('Text of Plan');
    expect(screen.queryByText('Attention:')).toBeNull();
    const tab = screen.getByRole('tab', { name: 'Plan' });
    expect(document.getElementById(tab.getAttribute('aria-describedby') ?? '')?.textContent).toBe(
      '',
    );
    expect(screen.queryByText(/no warnings|safe/i)).toBeNull();
  });

  it('marks nothing where the warnings could not be read', async () => {
    setup(() => fail(500, 'internal', 'the index could not be read'));
    renderAt(`/w/${WS}/i/${NOTE}`);
    await screen.findByText('Text of Meeting notes');
    const row = await screen.findByRole('treeitem', { name: /^Meeting notes/ });
    expect(row.getAttribute('aria-label')).toBe('Meeting notes, type meeting-note');
    expect(screen.getByRole('link', { name: 'Attention' })).toBeDefined();
    expect(screen.getByText('Attention:')).toBeDefined();
    expect(
      screen.getByText("Checking this item's warnings: Something went wrong in the service."),
    ).toBeDefined();
  });
});

describe('Attention marks after losing contact', { timeout: 30_000 }, () => {
  for (const transition of ['hidden', 'disconnected'] as const) {
    it(`does not present retained marks as current while ${transition} or awaiting recovery`, async () => {
      // v1 freshness: retained findings remain readable as previous evidence, but their
      // tree/tab marks cannot look like a current check after live contact is lost.
      const reopening = Promise.withResolvers<void>();
      const recovery = Promise.withResolvers<Reply>();
      let holdRecovery = false;
      const api = setup(() => (holdRecovery ? recovery.promise : answer(THREE)));
      const events = mockEvents();
      const view = renderAt(`/w/${WS}/i/${NOTE}`);
      const key = attentionOptions(WS, HEAD).queryKey;
      try {
        await screen.findByText('Text of Meeting notes');
        await waitFor(() => expect(events.streams).toHaveLength(1));
        await waitFor(() => expect(view.queryClient.isFetching()).toBe(0));
        const row = () => screen.getByRole('treeitem', { name: /^Meeting notes/ });
        const tabDescription = () => {
          const tab = screen.getByRole('tab', { name: 'Meeting notes' });
          return document.getElementById(tab.getAttribute('aria-describedby') ?? '')?.textContent;
        };
        // Literal oracles are the neighboring current-mark test's exact real consumer labels.
        expect(row().getAttribute('aria-label')).toBe(
          'Meeting notes, type meeting-note, 2 warnings (advisory)',
        );
        expect(tabDescription()).toBe(' (2 warnings, advisory)');
        const requests = api.callsTo(ATTENTION).length;
        holdRecovery = true;
        events.state.hold = reopening.promise;
        if (transition === 'hidden') act(() => pageShown(false));
        else act(() => events.streams.at(0)?.end());
        await waitFor(() => expect(view.queryClient.getQueryState(key)?.isInvalidated).toBe(true));
        expect(view.queryClient.getQueryState(key)?.fetchStatus).toBe('idle');
        expect(api.callsTo(ATTENTION)).toHaveLength(requests);
        expect(view.queryClient.getQueryData(key)?.pages).toEqual([
          { revision: HEAD, items: THREE, next_cursor: null },
        ]);
        await screen.findByText(
          'Awaiting a check with the current service. Showing previous findings.',
        );
        expect(screen.getByText('Attention:').closest('[role="note"]')?.textContent).toContain(
          'Previous check: 2 advisory warnings',
        );
        expect(row().getAttribute('aria-label')).toBe('Meeting notes, type meeting-note');
        expect(row().textContent).not.toContain('2 warnings');
        expect(tabDescription()).toBe('');

        if (transition === 'hidden') act(() => pageShown(true));
        events.state.hold = null;
        await act(async () => {
          reopening.resolve();
          await reopening.promise;
        });
        await waitFor(() => expect(events.streams).toHaveLength(2));
        await waitFor(() => expect(api.callsTo(ATTENTION)).toHaveLength(requests + 1));
        // A new live stream alone does not certify the retained answer while its check waits.
        expect(row().getAttribute('aria-label')).toBe('Meeting notes, type meeting-note');
        expect(tabDescription()).toBe('');
        expect(view.queryClient.getQueryData(key)?.pages.at(0)?.items).toEqual(THREE);
        await act(async () => {
          recovery.resolve(answer(THREE.slice(0, 1)));
          await recovery.promise;
        });
        await waitFor(() =>
          expect(row().getAttribute('aria-label')).toBe(
            'Meeting notes, type meeting-note, 1 warning (advisory)',
          ),
        );
        expect(tabDescription()).toBe(' (1 warning, advisory)');
        expect(view.queryClient.getQueryState(key)?.isInvalidated).toBe(false);
        expect(screen.queryByText(/Showing previous findings/)).toBeNull();
        expect(api.callsTo(ATTENTION)).toHaveLength(requests + 1);
        expect(api.violations).toEqual([]);
      } finally {
        view.unmount();
        reopening.resolve();
        recovery.resolve(answer(THREE));
        vi.restoreAllMocks();
      }
    });
  }
});
