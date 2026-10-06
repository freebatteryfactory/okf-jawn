/**
 * Shared MCP App: result dispatch (Source / Changes / Timeline / Present / unknown text), and
 * the wire boundary of its bridge to the host.
 *
 * The bridge in mcp-apps/main.tsx is the one place where a tool call's undefined optionals are
 * omitted: once, as the call leaves for the host, and nowhere in a feature component. main.tsx
 * mounts its own App into #root when it is imported, so each wire-boundary test puts #root in
 * the document and imports the module afresh. The ext-apps hook is replaced by a host double
 * that records what `callServerTool` receives; `omitUndefined` is the real function, counted.
 */

import { act, render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import changesFixture from '../../../tests/fixtures/views/changes-diff.json';
import presentFixture from '../../../tests/fixtures/views/present-response.json';
import sourceFixture from '../../../tests/fixtures/views/source-read-item.json';
import timelineFixture from '../../../tests/fixtures/views/timeline-log.json';
import { AppResult } from '../../src/mcp-apps/main';

interface ToolCall {
  name: string;
  arguments?: Record<string, unknown>;
}
type ToolResultHandler = (message: { structuredContent?: unknown }) => void;
type CallTool = (name: string, input: Record<string, unknown>) => Promise<unknown>;

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
    render(<AppResult result={sourceFixture} callTool={vi.fn()} />);
    expect(screen.getByText(/Qualification source/i)).toBeTruthy();
  });

  it('renders Changes from a DiffResponse', () => {
    render(<AppResult result={changesFixture} callTool={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'Changes' })).toBeTruthy();
  });

  it('renders Timeline from a LogResponse', () => {
    render(<AppResult result={timelineFixture} callTool={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'Timeline' })).toBeTruthy();
    expect(screen.getByText('Qualification timeline fixture')).toBeTruthy();
  });

  it('renders Present from a PresentResponse', async () => {
    const present = {
      ...presentFixture,
      resolved_bindings: [],
    };
    render(
      <AppResult result={present} callTool={vi.fn(async () => ({ structuredContent: {} }))} />,
    );
    expect(await screen.findByRole('heading', { name: 'Six-component catalog' })).toBeTruthy();
  });

  it('falls back to text for an unknown structuredContent shape', () => {
    render(<AppResult result={{ unexpected: true }} callTool={vi.fn()} />);
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
    // A feature component that hands the bridge an absent optional as an undefined-valued key.
    const handed: Record<string, unknown>[] = [];
    vi.doMock('../../src/features/views/PresentView', () => ({
      PresentView: ({ callTool }: { callTool: CallTool }) => {
        useEffect(() => {
          const input = {
            workspace_id: 'ws_1',
            selection: undefined,
            cursor: null,
            max_images: 0,
            view: '',
          };
          handed.push(input);
          void callTool('show', input);
        }, [callTool]);
        return <p>probe</p>;
      },
    }));
    const omitUndefined = await mountApp();
    expect(document.body.textContent).toBe('Waiting for a tool result from the connected host.');
    await deliver(presentFixture);
    await waitFor(() => expect(host.sent.length).toBeGreaterThan(0));

    expect(host.sent.length).toBe(handed.length);
    for (const [index, call] of host.sent.entries()) {
      const input = handed[index];
      expect(call.name).toBe('show');
      // What the feature component passed still holds the key: nothing before the bridge dropped it.
      expect(input && Object.hasOwn(input, 'selection')).toBe(true);
      expect(Object.keys(call.arguments ?? {})).toEqual([
        'workspace_id',
        'cursor',
        'max_images',
        'view',
      ]);
      expect(call.arguments).toStrictEqual({
        workspace_id: 'ws_1',
        cursor: null,
        max_images: 0,
        view: '',
      });
      expect(undefinedKeys(call.arguments)).toEqual([]);
    }
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
      // A refusal ends the dataset read; by then the bridge has sent both kinds of call.
      return {
        isError: true,
        content: [{ type: 'text', text: 'not retained by this host double' }],
      };
    };
    const omitUndefined = await mountApp();
    await deliver(presentFixture);
    await waitFor(() => expect(document.body.textContent).toBe('not retained by this host double'));

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
