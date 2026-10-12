/**
 * A keyboard retry never drops focus to the page, in every place the workspace offers one: the
 * failure notice stays mounted while the operation is asked again, then focus is on what the retry
 * loaded, or still on the retry button when it failed again. One case per caller of the notice,
 * and the tree's failed row, which is a row of the tree, not a notice.
 */

import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SearchHit } from '../../src/api/generated/types.gen';
import { fail, type Handler, type Json, mockApi, ok, type Reply } from './support/api';
import {
  explorerHandlers,
  HEAD,
  itemDocument,
  listing,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  readResponse,
  renderAt,
  sourceSummary,
  sourcesOfNote,
  WS,
  workspace,
} from './support/workspace';

afterEach(() => {
  vi.unstubAllGlobals();
});

const WORKSPACES = '/api/workspaces/list-workspaces';
const OPEN = '/api/workspaces/open-workspace';
const LIST = '/api/items/list-items';
const GET = '/api/items/get-item';
const READ = '/api/reads/read-item';
const SOURCES = '/api/reads/get-sources';
const SEARCH = '/api/search/search-items';
const NEW = 'e'.repeat(40);

const second = { ...workspace, id: '77777777-7777-4777-8777-777777777777', name: 'Second' };

function hit(itemId: string, title: string): SearchHit {
  return {
    title,
    snippet: `…${title}…`,
    score: 1,
    source: {
      workspace_id: WS,
      item_id: itemId,
      path: `notes/${title}.md`,
      revision: HEAD,
      selection: { kind: 'lines', range: { start: 1, end: 2 } },
      locations: [],
    },
  };
}

const cursorOf = (body: Json): unknown =>
  typeof body.page === 'object' && body.page !== null && 'cursor' in body.page
    ? body.page.cursor
    : null;

type Outcome = 'answers' | 'fails again';

/**
 * The operation a case studies: its requests fail until the test starts holding them, then each
 * waits for the test to say how it ends. Other requests on the same path get the good answer.
 */
function studied(good: Handler, isStudied: (body: Json) => boolean = () => true) {
  let holding = false;
  const held: Array<(outcome: Outcome) => void> = [];
  const handler: Handler = (body) => {
    if (!isStudied(body)) return good(body);
    if (!holding) return fail(500, 'internal', 'storage stumbled');
    return new Promise<Reply>((resolve) => {
      held.push((outcome) =>
        resolve(
          outcome === 'answers' ? good(body) : fail(500, 'internal', 'storage stumbled again'),
        ),
      );
    });
  };
  return {
    handler,
    hold: () => {
      holding = true;
    },
    inFlight: () => held.length,
    end: (outcome: Outcome) => {
      const give = held.shift();
      if (give === undefined) throw new Error('no request of the studied operation is in flight');
      give(outcome);
    },
  };
}

interface Caller {
  /** The place, as `FailureNotice` is called there. */
  name: string;
  url: string;
  /** The path of the operation that fails, and what it answers when it works. */
  path: string;
  good: Handler;
  /** Which requests on that path are the failing operation's (all of them, unless given). */
  isStudied?: () => (body: Json) => boolean;
  /** What the person does to meet the failure, when opening the page is not enough. */
  reach?: () => Promise<void>;
  /** The notice's words for what was being done. */
  context: string;
  /** What the operation's own status says while it loads; not said during a retry. */
  loadingLine?: string;
  /** That line is a note, not a status. */
  loadingLineIsNote?: boolean;
  /** The element keyboard focus is on after the retry loaded the content. */
  loaded: () => HTMLElement | null | undefined;
}

/** The element around these words that script can focus (`tabindex="-1"`): a part, a notice. */
const focusableAround = (words: HTMLElement | null) =>
  words?.closest<HTMLElement>('[tabindex="-1"]') ?? null;

/** The words of every status on the page, or of every note. */
const linesOf = (role: 'status' | 'note') =>
  screen.queryAllByRole(role).map((line) => line.textContent ?? '');

const press = async (name: string) => {
  const button = await screen.findByRole('button', { name });
  act(() => button.focus());
  await userEvent.keyboard('{Enter}');
};

const callers: Caller[] = [
  {
    name: 'the workspace list (routes/index.tsx)',
    url: '/',
    path: WORKSPACES,
    good: () => ok({ items: [workspace], next_cursor: null }),
    context: 'Listing your workspaces',
    loadingLine: 'Contacting the workspace service…',
    loaded: () => screen.queryByRole('link', { name: 'Workshop notes' }),
  },
  {
    name: 'more workspaces (routes/index.tsx)',
    url: '/',
    path: WORKSPACES,
    good: (body) =>
      cursorOf(body) === 'more'
        ? ok({ items: [second], next_cursor: null })
        : ok({ items: [workspace], next_cursor: 'more' }),
    isStudied: () => (body) => cursorOf(body) === 'more',
    reach: () => press('Show more workspaces'),
    context: 'Listing more workspaces',
    loaded: () => screen.queryByRole('link', { name: 'Second' }),
  },
  {
    name: 'opening a workspace, which resolves the shell’s head (routes/w.$workspaceId.tsx)',
    url: `/w/${WS}`,
    path: OPEN,
    good: () => ok(workspace),
    context: 'Opening this workspace',
    loadingLine: 'Opening the workspace…',
    loaded: () => screen.queryByRole('heading', { level: 1, name: 'Workshop notes' }),
  },
  {
    name: 'the tree’s root listing (FolderTree.tsx)',
    url: `/w/${WS}`,
    path: LIST,
    good: (body) =>
      body.folder === 'notes'
        ? ok(listing([], [noteSummary, otherSummary]))
        : ok(listing(['notes'], [sourceSummary])),
    context: 'Listing this workspace',
    loadingLine: 'Loading folders…',
    loaded: () => screen.queryByRole('treeitem', { name: 'notes' }),
  },
  {
    name: 'the source panel (SourcePanel.tsx)',
    url: `/w/${WS}/i/${NOTE}`,
    path: SOURCES,
    good: () => ok(sourcesOfNote),
    context: 'Listing this item’s sources',
    loadingLine: 'Loading sources…',
    loaded: () =>
      within(screen.getByRole('region', { name: 'Source panel' })).queryByRole('heading', {
        level: 2,
        name: 'Sources',
      }),
  },
  {
    name: 'search (SearchPanel.tsx)',
    url: `/w/${WS}?q=budget`,
    path: SEARCH,
    good: () => ok({ hits: [hit(NOTE, 'Meeting notes')], revision: HEAD }),
    context: 'Searching',
    loadingLine: 'Searching…',
    loaded: () => screen.queryByRole('link', { name: /Meeting notes/ }),
  },
  {
    name: 'more search results (SearchPanel.tsx)',
    url: `/w/${WS}?q=budget`,
    path: SEARCH,
    good: (body) =>
      cursorOf(body) === 'more'
        ? ok({ hits: [hit(OTHER, 'Plan')], revision: HEAD })
        : ok({ hits: [hit(NOTE, 'Meeting notes')], revision: HEAD, next_cursor: 'more' }),
    isStudied: () => (body) => cursorOf(body) === 'more',
    reach: () => press('Show more results'),
    context: 'Loading more results',
    loadingLine: 'Loading more results…',
    loaded: () => screen.queryByRole('link', { name: /Plan/ }),
  },
  {
    name: 'the reader’s properties (DocumentView.tsx)',
    url: `/w/${WS}/i/${NOTE}`,
    path: GET,
    good: () => ok(itemDocument(noteSummary, { owner: 'Ada' })),
    context: 'Reading this item’s properties',
    loadingLine: 'Loading properties…',
    loaded: () => screen.queryByText('Properties (3)'),
  },
  {
    name: 'the reader’s text (DocumentView.tsx)',
    url: `/w/${WS}/i/${NOTE}`,
    path: READ,
    good: () => ok(readResponse(noteSummary, 'Text of Meeting notes')),
    context: 'Reading this item',
    loadingLine: 'Reading…',
    loaded: () => focusableAround(screen.queryByText('Text of Meeting notes')),
  },
  {
    name: 'the reader’s continuation (DocumentView.tsx)',
    url: `/w/${WS}/i/${NOTE}`,
    path: READ,
    good: (body) =>
      body.cursor === 'part-2'
        ? ok(readResponse(noteSummary, 'Part two'))
        : ok(readResponse(noteSummary, 'Part one', { truncated: true, next_cursor: 'part-2' })),
    isStudied: () => (body) => body.cursor === 'part-2',
    reach: () => press('Continue reading'),
    context: 'Continuing this reading',
    loadingLine: 'Reading more…',
    loaded: () => screen.queryByRole('group', { name: 'Part 2 of this reading' }),
  },
  {
    name: 'the reader’s check of a cited revision against the head (DocumentView.tsx)',
    url: `/w/${WS}/i/${NOTE}?rev=${OLD}`,
    path: OPEN,
    good: () => ok(workspace),
    // The first request opens the workspace; the reader's own check comes after it.
    isStudied: () => {
      let opens = 0;
      return () => {
        opens += 1;
        return opens > 1;
      };
    },
    context: 'Checking for the latest version',
    loadingLine: 'Checking whether it is the latest version…',
    loadingLineIsNote: true,
    loaded: () => focusableAround(screen.queryByText(/it is not the latest version/)),
  },
  {
    name: 'the reader’s "Read the latest version" (DocumentView.tsx)',
    url: `/w/${WS}/i/${NOTE}?rev=${OLD}`,
    path: READ,
    good: (body) => {
      const at = body.at;
      const latest = typeof at === 'object' && at !== null && 'kind' in at && at.kind === 'latest';
      return ok(
        readResponse({ ...noteSummary, revision: latest ? NEW : OLD }, 'Text of Meeting notes'),
      );
    },
    isStudied: () => (body) =>
      typeof body.at === 'object' &&
      body.at !== null &&
      'kind' in body.at &&
      body.at.kind === 'latest',
    reach: () => press('Read the latest version'),
    context: 'Reading the latest version',
    // The revision notice, once it names the revision the retry read.
    loaded: () => {
      const notice = focusableAround(screen.queryByText(/You are reading revision/));
      return notice?.textContent?.includes(`reading revision ${NEW.slice(0, 10)}`) ? notice : null;
    },
  },
];

/** Open the caller's page, meet its failure, and press its retry button from the keyboard. */
async function retryFromTheKeyboard(caller: Caller) {
  const operation = studied(caller.good, caller.isStudied?.());
  mockApi(explorerHandlers({ [caller.path]: operation.handler }));
  renderAt(caller.url);
  await caller.reach?.();
  const words = await screen.findByText(`${caller.context}: Something went wrong in the service.`);
  const alert = words.closest<HTMLElement>('[role="alert"]');
  const notice = alert?.parentElement;
  if (alert === null || notice === null || notice === undefined) {
    throw new Error('the failure is not shown in a notice');
  }
  const button = within(notice).getByRole('button', { name: 'Try again' });
  operation.hold();
  act(() => button.focus());
  await userEvent.keyboard('{Enter}');
  await waitFor(() => expect(operation.inFlight()).toBe(1));
  // Long enough for the caller to render the operation's state while it is asked again.
  await new Promise((resolve) => setTimeout(resolve, 30));
  return { operation, alert, notice, button };
}

describe.each(callers)('a keyboard retry at $name', (caller) => {
  it('keeps the notice mounted while the operation is asked again, then moves focus to what it loaded', async () => {
    const { operation, alert, notice, button } = await retryFromTheKeyboard(caller);
    expect(notice.isConnected).toBe(true);
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);
    expect(button.textContent).toBe('Trying again…');
    expect(button.getAttribute('aria-disabled')).toBe('true');
    expect(within(notice).getByRole('status').textContent).toBe(`${caller.context}: trying again…`);
    expect(alert.isConnected).toBe(true);
    // The notice says the retry is in flight; the operation's own loading line does not as well.
    const line = caller.loadingLine;
    if (line !== undefined) {
      const said = linesOf(caller.loadingLineIsNote ? 'note' : 'status');
      expect(said.filter((words) => words.includes(line))).toEqual([]);
    }

    operation.end('answers');
    await waitFor(() => expect(notice.isConnected).toBe(false));
    await waitFor(() => {
      const loaded = caller.loaded();
      expect(loaded).toBeTruthy();
      expect(document.activeElement).toBe(loaded);
    });
    expect(document.activeElement).not.toBe(document.body);
  });

  it('keeps focus on the retry button of the new notice when the operation fails again', async () => {
    const { operation, alert, notice, button } = await retryFromTheKeyboard(caller);
    operation.end('fails again');
    const again = await within(notice).findByText(/storage stumbled again/);
    expect(notice.isConnected).toBe(true);
    await waitFor(() => expect(button.textContent).toBe('Try again'));
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);
    expect(button.getAttribute('aria-disabled')).toBe('false');
    // The words of the second failure are a new alert.
    expect(alert.isConnected).toBe(false);
    expect(again.closest('[role="alert"]')?.textContent).toContain(`${caller.context}:`);
    expect(within(notice).getByRole('status').textContent).toBe('');
  });
});

describe('a keyboard retry at a failed row of the tree (FolderTree.tsx)', () => {
  async function failedRow() {
    const operation = studied(
      (body) =>
        body.folder === 'notes'
          ? ok(listing([], [noteSummary, otherSummary]))
          : ok(listing(['notes'], [sourceSummary])),
      (body) => body.folder === 'notes',
    );
    mockApi(explorerHandlers({ [LIST]: operation.handler }));
    renderAt(`/w/${WS}`);
    const tree = await screen.findByRole('tree', { name: 'Folders' });
    await userEvent.click(await within(tree).findByRole('treeitem', { name: 'notes' }));
    const row = await within(tree).findByRole('treeitem', { name: /Could not list this folder/ });
    operation.hold();
    act(() => row.focus());
    await userEvent.keyboard('{Enter}');
    await waitFor(() => expect(operation.inFlight()).toBe(1));
    await waitFor(() => expect(row.getAttribute('aria-label')).toBe('Loading…'));
    return { operation, tree, row };
  }

  it('keeps the row, and focus on it, while the folder is listed again, then moves focus to the first entry it brought', async () => {
    const { operation, tree, row } = await failedRow();
    expect(row.isConnected).toBe(true);
    expect(document.activeElement).toBe(row);
    operation.end('answers');
    const first = await within(tree).findByRole('treeitem', { name: /Meeting notes/ });
    await waitFor(() => expect(document.activeElement).toBe(first));
  });

  it('keeps focus on the row when the folder fails to list again', async () => {
    const { operation, tree, row } = await failedRow();
    operation.end('fails again');
    const failed = await within(tree).findByRole('treeitem', { name: /storage stumbled again/ });
    expect(failed).toBe(row);
    await waitFor(() => expect(document.activeElement).toBe(row));
  });
});
