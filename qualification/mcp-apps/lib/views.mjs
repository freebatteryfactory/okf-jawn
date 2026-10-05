/**
 * What the MCP Apps qualification expects to see, and how it judges what it saw.
 * Pure: no I/O and no imports, so every rule is testable without a browser.
 *
 * Expected strings come from tests/fixtures/views/*.json as rendered by
 * ui/src/features/{documents/SourceExcerpt,history/Changes,history/Timeline,views/Layout}.tsx.
 * The present fixture has no materialized dataset, so its DataTable and Chart show the
 * two honest "unavailable" alerts listed below; any other alert fails the view.
 */

export const APP_RESOURCE_URI = 'ui://okf-jawn/app.html';
export const APP_ONLY_TOOLS = ['show'];
/** Rules that fire on the upstream basic-host chrome; tolerated there, never in the App frame. */
export const UPSTREAM_HOST_RULES = ['color-contrast', 'frame-title'];

const REVISION = '0123456789abcdef0123456789abcdef01234567';
const SOURCE_ONLY = 'Contract-valid ReadItemResponse fixture for the MCP Apps harness.';
const CHANGES_ONLY = 'Harness fixture line.';
const TIMELINE_ONLY = 'Qualification timeline fixture';
const PRESENT_ONLY = 'Six-component catalog';

export const VIEWS = [
  {
    tool: 'render_source',
    view: 'source',
    mustContain: ['fixtures/qualification-source.md', REVISION, 'Qualification source', SOURCE_ONLY],
    mustNotContain: [CHANGES_ONLY, TIMELINE_ONLY, PRESENT_ONLY],
    alerts: [],
  },
  {
    tool: 'render_changes',
    view: 'changes',
    mustContain: ['Changes', '89abcdef0123456789abcdef0123456789abcdef', 'fixtures/qualification-source.md', CHANGES_ONLY],
    mustNotContain: [SOURCE_ONLY, TIMELINE_ONLY, PRESENT_ONLY],
    alerts: [],
  },
  {
    tool: 'render_timeline',
    view: 'timeline',
    mustContain: ['Timeline', TIMELINE_ONLY, 'okf-qualify-mcp-apps', '89abcdef0123456789abcdef0123456789abcdef'],
    mustNotContain: [SOURCE_ONLY, CHANGES_ONLY, PRESENT_ONLY],
    alerts: [],
  },
  {
    tool: 'render_present',
    view: 'present',
    mustContain: [
      PRESENT_ONLY,
      'Metrics chart',
      SOURCE_ONLY,
      `fixtures/qualification-source.md @ ${REVISION}`,
      `fixtures/qualification-metrics.json @ ${REVISION}`,
    ],
    mustNotContain: [CHANGES_ONLY, TIMELINE_ONLY],
    alerts: ['Dataset unavailable: metrics', 'Resolved chart data or specification unavailable.'],
  },
];

const BLOCKING_TEXT = [
  /Host connection failed/i,
  /Waiting for a tool result from the connected host\./,
  /does not match a supported Source, Changes, Timeline, or Present schema/,
];

/** The official basic-host URL that auto-calls one tool (query keys from its src/index.tsx). */
export function basicHostUrl(tool, { server = 'okf-qualify-mcp-apps', port = 8080 } = {}) {
  const params = new URLSearchParams({ server, tool, call: 'true', theme: 'hide' });
  return `http://127.0.0.1:${port}/?${params}`;
}

/** Whether the App frame shows exactly this view: its text, no other view's, and only its expected alerts. */
export function judgeView(view, observed) {
  const text = observed?.text ?? '';
  const alerts = [...(observed?.alerts ?? [])].map((alert) => alert.trim()).sort();
  const expected_alerts = [...view.alerts].sort();
  const missing = view.mustContain.filter((needle) => !text.includes(needle));
  const foreign = view.mustNotContain.filter((needle) => text.includes(needle));
  const blocking = BLOCKING_TEXT.filter((pattern) => pattern.test(text)).map(String);
  const ok =
    observed !== null &&
    observed !== undefined &&
    missing.length === 0 &&
    foreign.length === 0 &&
    blocking.length === 0 &&
    JSON.stringify(alerts) === JSON.stringify(expected_alerts);
  return { ok, missing, foreign, blocking, alerts, expected_alerts };
}

const SERIOUS = new Set(['serious', 'critical']);
/** Frame hops to a node: axe gives one target entry per frame boundary plus one for the element. */
const depthOf = (node) => (Array.isArray(node?.target) ? node.target.length - 1 : 0);

/**
 * Split serious/critical axe violations by where their nodes live.
 * `appDepth` is the App document's frame depth (2 in basic-host: host -> sandbox -> App).
 * Host chrome may violate only UPSTREAM_HOST_RULES; the App frame may violate nothing.
 */
export function partitionAxe(results, appDepth) {
  const inApp = (node) => depthOf(node) >= appDepth;
  const everyNode = ['violations', 'passes', 'incomplete'].flatMap((key) =>
    (results?.[key] ?? []).flatMap((rule) => rule.nodes ?? []),
  );
  const app_frame = [];
  const host_blocking = [];
  const host_tolerated = [];
  for (const rule of results?.violations ?? []) {
    if (!SERIOUS.has(rule.impact)) continue;
    const appNodes = (rule.nodes ?? []).filter(inApp);
    const hostNodes = (rule.nodes ?? []).filter((node) => !inApp(node));
    const entry = (nodes) => ({ id: rule.id, impact: rule.impact, nodes: nodes.map((node) => node.target) });
    if (appNodes.length) app_frame.push(entry(appNodes));
    if (hostNodes.length) {
      (UPSTREAM_HOST_RULES.includes(rule.id) ? host_tolerated : host_blocking).push(entry(hostNodes));
    }
  }
  return {
    app_frame_analysed: appDepth > 0 && everyNode.some(inApp),
    app_frame,
    host_blocking,
    host_tolerated,
  };
}

/** Passed only when every view in VIEWS has a passed result. */
export function basicHostVerdict(results) {
  const failed = [];
  for (const view of VIEWS) {
    const result = results.find((item) => item.tool === view.tool);
    if (!result) failed.push({ tool: view.tool, error: 'not rendered' });
    else if (result.status !== 'passed') {
      failed.push({ tool: view.tool, error: result.error ?? 'failed without an error message' });
    }
  }
  return { status: failed.length === 0 ? 'passed' : 'failed', failed };
}

/** Reasons the run must exit non-zero; empty when it may exit 0. */
export function runProblems({ protocol, basicHost, protocolOnly }) {
  const problems = [];
  if (protocol?.status !== 'passed') {
    problems.push(`protocol_check ${protocol?.status ?? 'missing'}: ${protocol?.error ?? 'no detail'}`);
  }
  if (!protocolOnly && basicHost?.status !== 'passed') {
    const detail =
      basicHost?.error ??
      (basicHost?.failed ?? []).map((item) => `${item.tool}: ${item.error}`).join('; ') ??
      'no detail';
    problems.push(`basic_host ${basicHost?.status ?? 'missing'}: ${detail}`);
  }
  return problems;
}

/** The tunnel lifecycle as a receipt may state it. An opened tunnel must have been closed. */
export function ngrokRecord({ enabled, opened_at, closed_at, public_url, local_port, note }) {
  if (!enabled) {
    return { status: 'not_run', opened_at: null, closed_at: null, public_url: null, local_port, note };
  }
  if (opened_at === null || opened_at === undefined) {
    return { status: 'not_opened', opened_at: null, closed_at: closed_at ?? null, public_url: null, local_port, note };
  }
  if (typeof closed_at !== 'string') {
    throw new Error('ngrok was opened but not closed; a receipt never records an open tunnel');
  }
  return { status: 'closed', opened_at, closed_at, public_url: public_url ?? null, local_port, note };
}
