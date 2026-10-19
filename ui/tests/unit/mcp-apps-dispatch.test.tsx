/**
 * Shared MCP App: result dispatch (Source / Changes / Timeline / Present / unknown text), the
 * wire boundary of its bridge to the host, and the App's `ViewData` over the server tools.
 *
 * The App's `ViewData` (mcp-apps/view-data.ts, which main.tsx gives every View) is the one place
 * where a tool call's undefined optionals are omitted: once, as the call leaves for the host, and
 * nowhere in a feature component. main.tsx mounts its own App into #root when it is imported, so
 * each wire-boundary test puts #root in the document and imports the module afresh. The ext-apps
 * hook is replaced by a host double that records what `callServerTool` receives;
 * `omitUndefined` is the real function, counted.
 */

import { act, render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import changesFixture from '../../../tests/fixtures/views/changes-diff.json';
import presentFixture from '../../../tests/fixtures/views/present-response.json';
import sourceFixture from '../../../tests/fixtures/views/source-read-item.json';
import timelineFixture from '../../../tests/fixtures/views/timeline-log.json';
import type { GetObjectRequest, ReadItemRequest } from '../../src/api/generated/types.gen';
import { zReadItemResponse } from '../../src/api/generated/zod.gen';
import type { ViewData } from '../../src/features/views/data';
import { AppResult } from '../../src/mcp-apps/main';
import { serverToolData } from '../../src/mcp-apps/view-data';

interface ToolCall {
  name: string;
  arguments?: Record<string, unknown>;
}
type ToolResultHandler = (message: { structuredContent?: unknown }) => void;
/** A View's reads for a dispatch that makes none: every read is refused. */
const noReads = serverToolData(async () => ({
  isError: true,
  content: [{ type: 'text', text: 'no read expected' }],
}));

/** The host double: what the bridge sent, how the host answers, and the handler main.tsx registered. */
const host = vi.hoisted(() => ({
  sent: [] as ToolCall[],
  answer: (async () => ({})) as (call: ToolCall) => Promise<unknown>,
  app: null as null | {
    ontoolresult?: ToolResultHandler;
    callServerTool: (call: ToolCall) => Promise<unknown>;
  },
}));

vi.mock('@modelcontextprotocol/ext-apps/react', () => ({
  useApp: (options: { onAppCreated?: (app: unknown) => void }) => {
    if (host.app === null) {
      host.app = {
        callServerTool: async (call: ToolCall) => {
          host.sent.push(call);
          return host.answer(call);
        },
      };
      options.onAppCreated?.(host.app);
    }
    return { app: host.app, error: null, isConnected: true };
  },
}));

vi.mock('../../src/lib/wire', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../src/lib/wire')>();
  return { ...actual, omitUndefined: vi.fn(actual.omitUndefined) };
});

describe('MCP App AppResult dispatch', () => {
  it('renders Source from a ReadItemResponse', () => {
    render(<AppResult result={sourceFixture} data={noReads} />);
    expect(screen.getByText(/Qualification source/i)).toBeTruthy();
  });

  it('renders Changes from a DiffResponse', () => {
    render(<AppResult result={changesFixture} data={noReads} />);
    expect(screen.getByRole('heading', { name: 'Changes' })).toBeTruthy();
  });

  it('renders Timeline from a LogResponse', () => {
    render(<AppResult result={timelineFixture} data={noReads} />);
    expect(screen.getByRole('heading', { name: 'Timeline' })).toBeTruthy();
    expect(screen.getByText('Qualification timeline fixture')).toBeTruthy();
  });

  it('renders Present from a PresentResponse', async () => {
    const present = {
      ...presentFixture,
      resolved_bindings: [],
    };
    render(
      <AppResult
        result={present}
        data={serverToolData(vi.fn(async () => ({ structuredContent: {} })))}
      />,
    );
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
  });

  it('isolates a chart the server reports failed: its own alert says why, the View still renders', async () => {
    const present = {
      ...presentFixture,
      resolved_bindings: [],
      charts: [
        {
          chart: { kind: 'named', name: 'metrics_chart' },
          status: {
            status: 'failed',
            reason: 'too_large',
            message: 'the dataset has 9000000 rows',
          },
        },
      ],
    };
    render(
      <AppResult
        result={present}
        data={serverToolData(vi.fn(async () => ({ structuredContent: {} })))}
      />,
    );
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
    const chart = screen.getByRole('heading', { name: 'Metrics chart' }).closest('section');
    expect(chart?.querySelector('[role="alert"]')?.textContent).toBe(
      'Dataset too large: the dataset has 9000000 rows',
    );
    // The other components of the same View are still on the page.
    expect(screen.getByText('Unresolved: venue')).toBeTruthy();
  });

  it('falls back to text for an unknown structuredContent shape', () => {
    render(<AppResult result={{ unexpected: true }} data={noReads} />);
    expect(screen.getByRole('status').textContent).toMatch(/unexpected/);
  });
});

/** Import main.tsx with #root in the document, so it mounts its App; returns the counted omitUndefined. */
async function mountApp() {
  document.body.innerHTML = '<div id="root"></div>';
  await act(async () => {
    await import('../../src/mcp-apps/main');
  });
  const wire = await import('../../src/lib/wire');
  return vi.mocked(wire.omitUndefined);
}

/** Hand the mounted App a tool result, as the host does. */
async function deliver(structuredContent: unknown) {
  const handler = host.app?.ontoolresult;
  if (!handler) throw new Error('main.tsx registered no ontoolresult handler');
  await act(async () => {
    handler({ structuredContent });
  });
}

const undefinedKeys = (value: Record<string, unknown> | undefined) =>
  Object.entries(value ?? {})
    .filter(([, entry]) => entry === undefined)
    .map(([key]) => key);

describe('MCP App wire boundary', () => {
  beforeEach(() => {
    vi.resetModules();
    host.sent = [];
    host.app = null;
    host.answer = async () => ({});
  });

  afterEach(() => {
    vi.doUnmock('../../src/features/views/PresentView');
    document.body.innerHTML = '';
  });

  it('omits an undefined optional as the call leaves for the host, and keeps null, zero and the empty string', async () => {
    // A feature component that hands the App's reads an absent optional as an undefined-valued
    // key (`offset`), beside a null (`cursor`), zeros and empty strings. The typed requests
    // cannot spell an undefined optional; one still arrives at run time from an untyped spread,
    // which is what Object.assign stands for here.
    const source = {
      workspace_id: 'ws_1',
      item_id: '',
      path: 'fixtures/qualification-metrics.json',
      revision: '0123456789abcdef0123456789abcdef01234567',
      selection: { kind: 'all' as const },
    };
    const read: ReadItemRequest = {
      workspace_id: 'ws_1',
      item_id: '',
      at: { kind: 'latest' },
      view: 'text',
      selection: { kind: 'all' },
      max_bytes: 256,
      max_images: 0,
      cursor: null,
    };
    const block: GetObjectRequest = Object.assign(
      {
        source,
        object: '',
        offset: '0',
        length: 0,
      },
      { offset: undefined },
    );
    const handed: Record<string, unknown>[] = [];
    vi.doMock('../../src/features/views/PresentView', () => ({
      PresentView: ({ data }: { data: ViewData }) => {
        useEffect(() => {
          handed.push(read, block);
          void data.readItem(read).catch(() => undefined);
          void data.getObject(block).catch(() => undefined);
        }, [data]);
        return <p>probe</p>;
      },
    }));
    const omitUndefined = await mountApp();
    expect(document.body.textContent).toBe('Waiting for a tool result from the connected host.');
    await deliver(presentFixture);
    await waitFor(() => expect(host.sent.length).toBeGreaterThan(0));

    expect(host.sent.length).toBe(handed.length);
    for (const call of host.sent) {
      if (call.name === 'show') {
        expect(call.arguments).toStrictEqual(read);
      } else {
        expect(call.name).toBe('read_object');
        expect(Object.keys(call.arguments ?? {})).toEqual(['source', 'object', 'length']);
        expect(call.arguments).toStrictEqual({ source, object: '', length: 0 });
      }
      expect(undefinedKeys(call.arguments)).toEqual([]);
    }
    expect(host.sent.map((call) => call.name)).toContain('read_object');
    // What the feature component passed still holds the key: nothing before the bridge dropped it.
    expect(Object.hasOwn(block, 'offset')).toBe(true);
    // Once per call, on the object the feature component passed.
    expect(omitUndefined.mock.calls.map(([input]) => input)).toEqual(handed);
  });

  it('is the only place that omits: PresentView passes its fields through, so each host call is omitted once', async () => {
    const bound = new Map(
      presentFixture.resolved_bindings.map((binding) => [binding.source.item_id, binding.source]),
    );
    host.answer = async (call) => {
      if (call.name === 'show') {
        const source = bound.get(String(call.arguments?.item_id));
        return { structuredContent: { ...sourceFixture, source } };
      }
      // A refused dataset read is that binding's alert; by then the bridge has sent both kinds of call.
      return {
        isError: true,
        content: [{ type: 'text', text: 'not retained by this host double' }],
      };
    };
    const omitUndefined = await mountApp();
    await deliver(presentFixture);
    await waitFor(() =>
      expect(document.body.textContent).toContain('not retained by this host double'),
    );

    const keys = {
      show: ['workspace_id', 'item_id', 'at', 'view', 'selection', 'max_bytes', 'max_images'],
      read_object: ['source', 'object', 'offset', 'length'],
    };
    expect([...new Set(host.sent.map((call) => call.name))].sort()).toEqual([
      'read_object',
      'show',
    ]);
    expect(omitUndefined.mock.calls.length).toBe(host.sent.length);
    for (const [index, call] of host.sent.entries()) {
      // The bridge was handed every field PresentView builds, and sent them all on.
      expect(omitUndefined.mock.calls[index]?.[0]).toStrictEqual(call.arguments);
      expect(Object.keys(call.arguments ?? {})).toEqual(
        call.name === 'show' ? keys.show : keys.read_object,
      );
      expect(undefinedKeys(call.arguments)).toEqual([]);
    }
  });
});

describe("the MCP App's ViewData, over the server tools", () => {
  const cited = presentFixture.resolved_bindings[1]?.source;
  if (!cited) throw new Error('the fixture has a second binding');
  const read: ReadItemRequest = {
    workspace_id: cited.workspace_id,
    item_id: cited.item_id,
    at: { kind: 'revision', revision: cited.revision },
    view: 'text',
    selection: { kind: 'all' },
    max_bytes: 65536,
    max_images: 0,
  };
  const block: GetObjectRequest = {
    source: { ...cited, selection: { kind: 'all' } },
    object: 'a'.repeat(64),
    offset: '0',
    length: 1048576,
  };
  const part = {
    data_base64: 'e30=',
    has_more: false,
    media_type: 'application/json',
    offset: '0',
    sha256: 'a'.repeat(64),
    total_size: '2',
  };

  it('reads a source with `show` and a block with `read_object`, each request as the tool arguments', async () => {
    const sent: ToolCall[] = [];
    const data = serverToolData(async (call) => {
      sent.push(call);
      return { structuredContent: call.name === 'show' ? sourceFixture : part };
    });
    expect(await data.readItem(read)).toStrictEqual(zReadItemResponse.parse(sourceFixture));
    expect(await data.getObject(block)).toStrictEqual(part);
    expect(sent).toStrictEqual([
      { name: 'show', arguments: read },
      { name: 'read_object', arguments: block },
    ]);
  });

  it("rejects with the tool's own words when it refuses, or a fixed sentence when it says nothing", async () => {
    const said = serverToolData(async () => ({
      isError: true,
      content: [{ type: 'text', text: 'revision is not retained by this source' }],
    }));
    await expect(said.readItem(read)).rejects.toThrow('revision is not retained by this source');
    const silent = serverToolData(async () => ({ isError: true, content: [] }));
    await expect(silent.readItem(read)).rejects.toThrow('The host refused to read the source.');
    await expect(silent.getObject(block)).rejects.toThrow('The host refused to read the dataset.');
  });

  it('rejects structured content the generated schema does not read', async () => {
    const data = serverToolData(async () => ({ structuredContent: { markdown: 7 } }));
    await expect(data.readItem(read)).rejects.toThrow(/markdown/);
    await expect(data.getObject(block)).rejects.toThrow(/data_base64/);
  });
});
