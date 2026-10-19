/**
 * WebMCP page tools (ruling B5; SPEC section 9: "Page tools expose the selected
 * source/revision/range, open a comparison/source, preview names, and prepare a proposal. The
 * small set uses the page's existing authenticated client functions. No Verify/Approve page
 * tools.").
 *
 * The browser's model context is the W3C draft's: `document.modelContext.registerTool(tool,
 * { signal })` returns a Promise, and aborting the signal is the only way to unregister. The fake
 * (`support/webmcp.ts`) records every registration and every abort. Five tools are registered,
 * only where the browser offers WebMCP and the person turned the setting on; each pins the
 * selection and revision on screen when it runs; `prepare_proposal` fills the Propose change
 * form and never sends it.
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
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NamingRules, Permission, Workspace } from '../../src/api/generated/types.gen';
import { WEBMCP_SETTING_KEY } from '../../src/features/settings/webmcp-setting';
import { type ModelContextTool, registerPageTools } from '../../src/webmcp/register';
import { type Handler, mockApi, ok, type Reply, rulesAnswer } from './support/api';
import { changeOf, DIFF, diffOf, LOG, OLDER, VERSIONS } from './support/history';
import { PROPOSE, proposalOf } from './support/proposals';
import {
  blockStorage,
  FIVE,
  installModelContext,
  READ_ONLY,
  removeModelContext,
} from './support/webmcp';
import {
  explorerHandlers,
  HEAD,
  itemDocument,
  NOTE,
  noteSummary,
  OLD,
  OTHER,
  otherSummary,
  renderAt,
  SOURCE,
  WS,
  workspace,
} from './support/workspace';

configure({ asyncUtilTimeout: 10_000 });

const OPEN = '/api/workspaces/open-workspace';
const GET_ITEM = '/api/items/get-item';
const RULES = '/api/conventions/get-rules';
const PREVIEW = '/api/conventions/preview-names';
const APPLY = '/api/conventions/apply-names';
const LIST = '/api/workspaces/list-workspaces';
const SETTING = 'Let an assistant in this browser use the page tools (WebMCP)';

const as = (...permissions: Permission[]): Workspace => ({ ...workspace, permissions });

const savedRules: NamingRules = {
  schema_version: 1,
  rules: [
    {
      glob: '**/*.md',
      case: 'kebab',
      separator: '-',
      prefix: 'w-',
      date_order: 'ymd',
      date_format: '%Y-%m-%d',
      strip_version_suffix: true,
      extension_policy: 'preserve',
      alias_preservation: true,
      collision: 'reject',
    },
  ],
};

const planAnswer: Handler = (body) => {
  const at = body.at as { revision?: unknown } | undefined;
  return ok({
    base_revision: typeof at?.revision === 'string' ? at.revision : HEAD,
    rules_digest: 'c'.repeat(64),
    entries: [{ item_id: NOTE, from: 'notes/meeting.md', to: 'notes/w-meeting.md', warnings: [] }],
    duplicate_observations: [],
    warnings: [],
    considered: 3,
    unchanged: 2,
    refused: 0,
    archived_left_out: 0,
  });
};

function setup(overrides: Record<string, Handler> = {}, who: Workspace = workspace) {
  return mockApi(
    explorerHandlers({
      [OPEN]: () => ok(who),
      [LIST]: () => ok({ items: [who], next_cursor: null }),
      [RULES]: (body) => ok(rulesAnswer(savedRules, body)),
      [PREVIEW]: planAnswer,
      [DIFF]: (body) => ok(diffOf(body, [changeOf('notes/meeting.md', 'Draft.', 'Published.')])),
      ...overrides,
    }),
  );
}

const turnOn = () => window.localStorage.setItem(WEBMCP_SETTING_KEY, 'on');

/** The item's text is on screen: the shell, and the tools with it, are there. */
const opened = (title = 'Meeting notes') => screen.findByText(`Text of ${title}`);

const names = (tools: readonly { name: string }[]) => tools.map((tool) => tool.name).sort();

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
  removeModelContext();
  window.localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('Registering the page tools', { timeout: 30_000 }, () => {
  it('registers nothing while the setting is off', async () => {
    setup();
    const context = installModelContext();
    renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    expect(context.registrations).toEqual([]);
  });

  it('registers nothing where storage is blocked: such a browser acts as off', async () => {
    setup();
    const context = installModelContext();
    const unblock = blockStorage();
    try {
      renderAt(`/w/${WS}/i/${NOTE}`);
      await opened();
      expect(context.registrations).toEqual([]);
    } finally {
      unblock();
    }
  });

  it('leaves the page working where the browser offers no WebMCP, with the setting on', async () => {
    const api = setup();
    removeModelContext();
    turnOn();
    renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    expect('modelContext' in document).toBe(false);
    expect(api.violations).toEqual([]);
  });

  it('registers exactly the five tools when the setting is on, the four that only read marked so', async () => {
    setup();
    const context = installModelContext();
    turnOn();
    renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.registrations).toHaveLength(5));
    expect(names(context.registrations.map((entry) => entry.tool))).toEqual([...FIVE].sort());
    for (const { tool, options, aborted } of context.registrations) {
      expect(aborted).toBe(false);
      expect(options.signal).toBeInstanceOf(AbortSignal);
      expect(tool.name).not.toMatch(/(?:^|[_.-])(?:verify|approve|review)/i);
      expect(tool.description.length).toBeGreaterThan(20);
      expect(tool.inputSchema).toMatchObject({ type: 'object' });
      expect(tool.annotations?.readOnlyHint === true).toBe(READ_ONLY.includes(tool.name));
    }
  });

  it('aborts every registration when the setting is turned off, and registers again when it is turned on', async () => {
    setup();
    const context = installModelContext();
    turnOn();
    renderAt(`/w/${WS}/settings`);
    const page = await screen.findByRole('region', { name: 'Settings' });
    await waitFor(() => expect(context.live()).toHaveLength(5));
    const box = within(page).getByRole('checkbox', { name: SETTING });
    fireEvent.click(box);
    await waitFor(() => expect(context.registrations.every((entry) => entry.aborted)).toBe(true));
    expect(context.registrations).toHaveLength(5);
    fireEvent.click(box);
    await waitFor(() => expect(context.live()).toHaveLength(5));
    expect(context.registrations).toHaveLength(10);
    expect(names(context.live().map((entry) => entry.tool))).toEqual([...FIVE].sort());
  });

  it('aborts every registration when the person leaves the workspace', async () => {
    setup();
    const context = installModelContext();
    turnOn();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    await act(() => router.navigate({ to: '/' }));
    await screen.findByRole('link', { name: 'Workshop notes' });
    expect(context.registrations.every((entry) => entry.aborted)).toBe(true);
  });

  it('aborts every registration when the Explorer unmounts', async () => {
    setup();
    const context = installModelContext();
    turnOn();
    const { unmount } = renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    unmount();
    expect(context.registrations.every((entry) => entry.aborted)).toBe(true);
  });

  it('refuses any tool outside the five, and anything named like verify, approve or review, registering none of them', () => {
    const context = installModelContext();
    const tool = (name: string): ModelContextTool => ({
      name,
      description: `A tool named ${name}.`,
      inputSchema: { type: 'object' },
      execute: () => Promise.resolve(null),
    });
    const signal = new AbortController().signal;
    for (const name of ['verify_item', 'approve_proposal', 'review', 'accept_proposal']) {
      expect(() => registerPageTools([tool('get_selection'), tool(name)], { signal })).toThrow(
        name,
      );
    }
    expect(context.registrations).toEqual([]);
  });

  it('says on the Settings page what the browser took, and what it refused in its own words', async () => {
    setup();
    installModelContext((tool) =>
      tool.name === 'open_source'
        ? Promise.reject(new DOMException('A tool of that name exists.', 'InvalidStateError'))
        : Promise.resolve(undefined),
    );
    turnOn();
    renderAt(`/w/${WS}/settings`);
    const page = await screen.findByRole('region', { name: 'Settings' });
    await within(page).findByText('The browser took 4 of the 5 page tools.');
    expect(page.textContent).toContain(
      'The browser refused open_source: A tool of that name exists.',
    );
  });

  it('takes a registration the browser refuses without an unhandled rejection, and says so', async () => {
    const context = installModelContext(() =>
      Promise.reject(new DOMException('A tool of that name exists.', 'InvalidStateError')),
    );
    const refused: string[] = [];
    registerPageTools(
      [
        {
          name: 'get_selection',
          description: 'What is on screen.',
          inputSchema: { type: 'object' },
          execute: () => Promise.resolve(null),
        },
      ],
      { signal: new AbortController().signal, onRefused: (name) => refused.push(name) },
    );
    await waitFor(() => expect(refused).toEqual(['get_selection']));
    expect(context.registrations).toHaveLength(1);
  });
});

describe('What each page tool does', { timeout: 30_000 }, () => {
  it('get_selection says what is on screen when it runs: the item, its exact revision and selection', async () => {
    // The plan had another title at the older revision.
    const api = setup({
      [GET_ITEM]: (body) => {
        const at = body.at as { revision?: unknown } | undefined;
        if (body.item_id !== OTHER) return ok(itemDocument(noteSummary));
        return ok(
          itemDocument(
            at?.revision === OLD ? { ...otherSummary, title: 'Plan, as it was' } : otherSummary,
          ),
        );
      },
    });
    const context = installModelContext();
    turnOn();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    expect(await context.run('get_selection')).toMatchObject({
      workspace_id: WS,
      head: HEAD,
      item: {
        item_id: NOTE,
        revision: HEAD,
        revision_named_by_link: false,
        selection: { kind: 'all' },
        title: 'Meeting notes',
        path: 'notes/meeting.md',
        kind: 'note',
      },
    });
    const lines = { kind: 'lines' as const, range: { start: 2, end: 4 } };
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
        search: { rev: OLD, sel: lines },
      }),
    );
    await opened('Plan');
    expect(await context.run('get_selection')).toMatchObject({
      head: HEAD,
      item: {
        item_id: OTHER,
        revision: OLD,
        revision_named_by_link: true,
        selection: lines,
        title: 'Plan, as it was',
      },
    });
    // Every item was asked of the service at an exact revision, never at "latest".
    for (const body of api.callsTo(GET_ITEM)) expect(body.at).toMatchObject({ kind: 'revision' });
    expect(api.violations).toEqual([]);
  });

  it('a tool keeps the selection it was called with when the page moves on before it answers', async () => {
    // While `hold` is set, `get_item` answers only when released: the tool's own request is then
    // still in flight when the person opens another item.
    const waiting: Array<() => void> = [];
    let hold = false;
    setup({
      [GET_ITEM]: (body) => {
        const answer = ok(itemDocument(body.item_id === OTHER ? otherSummary : noteSummary));
        if (!hold) return answer;
        return new Promise<Reply>((resolve) => {
          waiting.push(() => resolve(answer));
        });
      },
    });
    const context = installModelContext();
    turnOn();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    const LATER = '9'.repeat(40);
    hold = true;
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
        search: { rev: LATER },
      }),
    );
    const answer = context.run('get_selection');
    // The page's own read of the item and the tool's are both waiting.
    await waitFor(() => expect(waiting.length).toBeGreaterThanOrEqual(2));
    hold = false;
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: NOTE },
        search: {},
      }),
    );
    await opened();
    act(() => {
      for (const release of waiting) release();
    });
    expect(await answer).toMatchObject({
      item: { item_id: OTHER, revision: LATER, title: 'Plan' },
    });
  });

  it('open_source opens an item at the revision it names, else at the head shown when it runs', async () => {
    const api = setup();
    const context = installModelContext();
    turnOn();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    expect(await context.run('open_source', { item_id: OTHER })).toMatchObject({
      item_id: OTHER,
      revision: HEAD,
      title: 'Plan',
    });
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${OTHER}`));
    expect(router.state.location.search).toMatchObject({ rev: HEAD });
    const pages = { kind: 'pages' as const, range: { start: 2, end: 3 } };
    expect(
      await context.run('open_source', { item_id: SOURCE, revision: OLD, selection: pages }),
    ).toMatchObject({ item_id: SOURCE, revision: OLD, selection: pages });
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/i/${SOURCE}`));
    expect(router.state.location.search).toMatchObject({ rev: OLD, sel: pages });
    await expect(context.run('open_source', { item_id: 'not-an-item' })).rejects.toThrow(
      'not valid',
    );
    expect(router.state.location.pathname).toBe(`/w/${WS}/i/${SOURCE}`);
    expect(api.violations).toEqual([]);
  });

  it('open_comparison lands on the rendering the Timeline shows for the same pair', async () => {
    const versions = { [LOG]: () => ok({ commits: VERSIONS, next_cursor: null }) };
    /** What the one diff reader drew of each changed file. */
    const drawn = (region: HTMLElement) =>
      [...region.querySelectorAll('.changed-file')].map((file) => file.outerHTML);
    // The Timeline: "Draft the budget" (OLD) against the version before it (OLDER).
    const timelineApi = setup(versions);
    renderAt(`/w/${WS}/timeline`);
    const entry = (await screen.findByText('Draft the budget')).closest('li');
    if (entry === null) throw new Error('no Timeline entry for “Draft the budget”');
    fireEvent.click(within(entry).getByRole('button', { name: 'Show its changes' }));
    const timeline = await within(entry).findByRole('region', { name: 'Its changes' });
    const asked = timelineApi.callsTo(DIFF);
    expect(asked).toEqual([{ workspace_id: WS, from: OLDER, to: OLD }]);
    const timelineFiles = drawn(timeline);
    expect(timelineFiles).toHaveLength(1);
    cleanup();
    // A new page: the tool's comparison of the same pair asks the same and draws the same.
    const api = setup(versions);
    const context = installModelContext();
    turnOn();
    renderAt(`/w/${WS}`);
    await screen.findByRole('tree', { name: 'Folders' });
    await waitFor(() => expect(context.live()).toHaveLength(5));
    await context.run('open_comparison', { from: OLDER, to: OLD });
    const page = await screen.findByRole('region', { name: 'Comparison' });
    const compared = await within(page).findByRole('region', { name: 'What changed' });
    expect(api.callsTo(DIFF)).toEqual(asked);
    expect(drawn(compared)).toEqual(timelineFiles);
    expect(compared.querySelector('.meta')?.textContent).toBe(
      timeline.querySelector('.meta')?.textContent,
    );
    expect(api.violations).toEqual([]);
  });

  it('open_comparison opens the Changes between two revisions, to the one on screen by default', async () => {
    const api = setup();
    const context = installModelContext();
    turnOn();
    const { router } = renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    expect(await context.run('open_comparison', { from: OLD })).toMatchObject({
      from: OLD,
      to: HEAD,
      item_id: null,
      changed_files: 1,
    });
    const page = await screen.findByRole('region', { name: 'Comparison' });
    expect(router.state.location.pathname).toBe(`/w/${WS}/compare`);
    expect(router.state.location.search).toMatchObject({ from: OLD, to: HEAD });
    await within(page).findByRole('region', { name: 'What changed' });
    expect(page.textContent).toContain('Published.');
    expect(api.callsTo(DIFF).at(-1)).toEqual({ workspace_id: WS, from: OLD, to: HEAD });
    await context.run('open_comparison', { from: OLD, to: HEAD, item_id: NOTE });
    await waitFor(() => expect(router.state.location.search).toMatchObject({ item: NOTE }));
    expect(api.callsTo(DIFF).at(-1)).toEqual({
      workspace_id: WS,
      from: OLD,
      to: HEAD,
      item_id: NOTE,
    });
    // An item on screen at a cited revision: the comparison runs to that revision.
    await act(() =>
      router.navigate({
        to: '/w/$workspaceId/i/$itemId',
        params: { workspaceId: WS, itemId: OTHER },
        search: { rev: OLD },
      }),
    );
    await opened('Plan');
    expect(await context.run('open_comparison', { from: OLDER })).toMatchObject({
      from: OLDER,
      to: OLD,
    });
    expect(api.callsTo(DIFF).at(-1)).toEqual({ workspace_id: WS, from: OLDER, to: OLD });
    expect(api.violations).toEqual([]);
  });

  it('preview_names previews the saved rules at the head shown when it runs, and renames nothing', async () => {
    const api = setup();
    const context = installModelContext();
    turnOn();
    renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    expect(await context.run('preview_names')).toMatchObject({
      base_revision: HEAD,
      considered: 3,
      entries: [{ item_id: NOTE, from: 'notes/meeting.md', to: 'notes/w-meeting.md' }],
    });
    expect(api.callsTo(PREVIEW)).toEqual([
      {
        workspace_id: WS,
        at: { kind: 'revision', revision: HEAD },
        selection: { kind: 'workspace' },
        rules: savedRules,
      },
    ]);
    const other: NamingRules = { schema_version: 1, rules: [] };
    await context.run('preview_names', { folder: 'notes', rules: other });
    expect(api.callsTo(PREVIEW).at(-1)).toEqual({
      workspace_id: WS,
      at: { kind: 'revision', revision: HEAD },
      selection: { kind: 'folder', folder: 'notes', recursive: true },
      rules: other,
    });
    expect(api.callsTo(APPLY)).toEqual([]);
    expect(api.violations).toEqual([]);
  });

  it('prepare_proposal fills the Propose change form for the person and sends nothing', async () => {
    const api = setup({ [PROPOSE]: () => ok(proposalOf()) }, as('read', 'propose'));
    const context = installModelContext();
    turnOn();
    renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await screen.findByRole('button', { name: 'Propose a change…' });
    await waitFor(() => expect(context.live()).toHaveLength(5));
    const text = '# Meeting notes\n\nThe budget is published.\n';
    expect(
      await context.run('prepare_proposal', {
        title: 'Publish the budget',
        description: 'The board approved it on Monday.',
        body: text,
      }),
    ).toMatchObject({ sent: false, item_id: NOTE, base_revision: HEAD });
    const title = await screen.findByRole('textbox', { name: 'Title' });
    expect((title as HTMLInputElement).value).toBe('Publish the budget');
    const why = screen.getByRole('textbox', { name: 'Why' }) as HTMLTextAreaElement;
    expect(why.value).toBe('The board approved it on Monday.');
    const proposed = screen.getByRole('textbox', {
      name: 'The text you propose',
    }) as HTMLTextAreaElement;
    expect(proposed.value).toBe(text);
    expect(screen.getByText(/An assistant in this browser filled in this form/)).toBeTruthy();
    expect(api.callsTo(PROPOSE)).toEqual([]);
    // Only the person sends it.
    fireEvent.click(screen.getByRole('button', { name: 'Propose' }));
    await screen.findByRole('link', { name: 'Open the proposal' });
    expect(api.callsTo(PROPOSE)).toHaveLength(1);
    expect(api.callsTo(PROPOSE).at(0)).toMatchObject({
      base_revision: HEAD,
      title: 'Publish the budget',
      changes: [{ kind: 'edit', item_id: NOTE, body: text }],
    });
    expect(api.violations).toEqual([]);
  });

  it('prepare_proposal says why it cannot, and fills nothing, for a person who may not propose', async () => {
    const api = setup({ [PROPOSE]: () => ok(proposalOf()) }, as('read', 'write'));
    const context = installModelContext();
    turnOn();
    renderAt(`/w/${WS}/i/${NOTE}`);
    await opened();
    await waitFor(() => expect(context.live()).toHaveLength(5));
    await expect(
      context.run('prepare_proposal', { title: 'Publish', body: '# Meeting notes' }),
    ).rejects.toThrow('permission to propose');
    expect(screen.queryByRole('textbox', { name: 'Title' })).toBeNull();
    expect(api.callsTo(PROPOSE)).toEqual([]);
  });

  it('a tool run after the setting was turned off does nothing', async () => {
    setup();
    const context = installModelContext();
    turnOn();
    const { router } = renderAt(`/w/${WS}/settings`);
    const page = await screen.findByRole('region', { name: 'Settings' });
    await waitFor(() => expect(context.live()).toHaveLength(5));
    const open = context.live().find((entry) => entry.tool.name === 'open_source');
    fireEvent.click(within(page).getByRole('checkbox', { name: SETTING }));
    await waitFor(() => expect(context.live()).toEqual([]));
    await expect(
      open?.tool.execute({ item_id: OTHER }, { signal: new AbortController().signal }),
    ).rejects.toThrow('turned off');
    expect(router.state.location.pathname).toBe(`/w/${WS}/settings`);
  });
});
