/**
 * What the MCP Apps qualification expects to see, and how it judges what it saw.
 * Pure: no I/O and no imports, so every rule is testable without a browser.
 *
 * Expected strings come from tests/fixtures/views/*.json as rendered by
 * ui/src/features/{documents/SourceExcerpt,history/Changes,history/Timeline,views/Layout}.tsx.
 * No view tolerates an alert. The present fixture retains a dataset
 * (tests/fixtures/views/present-metrics-dataset.json), so its DataTable and Chart must be
 * drawn from it: the "unavailable" text they show without one is refused by name, and
 * judgePresentDataset decides from the observed chart, tables and tool calls.
 */

export const APP_RESOURCE_URI = 'ui://okf-jawn/app.html';
export const APP_ONLY_TOOLS = ['read_object', 'show'];
/** Prefix of the stderr line the harness writes per tool call (TOOL_CALL_LOG_PREFIX in src/main.rs). */
export const TOOL_CALL_LOG_PREFIX = 'okf-qualify-mcp-apps tool-call ';
/** Where the present view's retained dataset lives and which binding names it. */
export const PRESENT_DATASET = {
  binding: 'metrics',
  fixture: 'tests/fixtures/views/present-metrics-dataset.json',
};
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
      // The summary of the data table Chart.tsx draws under a chart it could resolve.
      'Source data',
    ],
    // Layout.tsx's fallbacks for a binding without rows; refused even if they lose role="alert".
    mustNotContain: [CHANGES_ONLY, TIMELINE_ONLY, 'Dataset unavailable', 'chart data or specification unavailable'],
    alerts: [],
    dataset: PRESENT_DATASET,
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

/** Tool calls the harness reported on stderr, in order; any other line is ignored. */
export function toolCallsFrom(stderrText) {
  const calls = [];
  for (const line of String(stderrText ?? '').split(/\r?\n/)) {
    if (!line.startsWith(TOOL_CALL_LOG_PREFIX)) continue;
    let record;
    try {
      record = JSON.parse(line.slice(TOOL_CALL_LOG_PREFIX.length));
    } catch {
      continue; // a line cut off by process teardown is not a report
    }
    if (record !== null && typeof record === 'object' && typeof record.tool === 'string') calls.push(record);
  }
  return calls;
}

/**
 * What the dataset must look like once rendered: DataTable's own projection of the rows
 * (ui/src/features/views/DataTable.tsx: columns in first-seen order, String(value ?? '')).
 * `rows` is the parsed fixture and `digest` the SHA-256 of its exact bytes.
 */
export function datasetExpectation({ binding, digest, rows }) {
  if (!Array.isArray(rows) || rows.length === 0) throw new Error('the dataset must be a non-empty JSON array of rows');
  const columns = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  return {
    binding,
    digest,
    rows: rows.length,
    columns,
    cells: rows.map((row) => columns.map((column) => String(row[column] ?? ''))),
  };
}

const sameJson = (left, right) => JSON.stringify(left) === JSON.stringify(right);

/**
 * Whether the present view rendered its retained dataset, judged only from what was seen:
 * `observed.alerts` and `observed.text` (App frame), `observed.svgs` / `observed.svg_marks`
 * (the Vega chart), `observed.tables` (every <table>: caption, visible, in_details, columns,
 * rows of cell text) and `observed.tool_calls` (harness reports while this view rendered).
 * `record` is what the receipt states; its status is "exercised" only when nothing is wrong.
 */
export function judgePresentDataset(expected, observed) {
  const problems = [];
  const alerts = [...(observed?.alerts ?? [])].map((alert) => alert.trim());
  const tables = (observed?.tables ?? []).filter((table) => table.caption === expected.binding);
  const dataTable = tables.find((table) => table.visible && !table.in_details) ?? null;
  const chartTable = tables.find((table) => table.in_details) ?? null;
  const calls = observed?.tool_calls ?? [];
  const reads = calls.filter((call) => call.tool === 'read_object' && call.ok === true && call.object === expected.digest);
  const shows = calls.filter((call) => call.tool === 'show' && call.ok === true);
  const refused = calls.filter((call) => call.ok !== true);
  const svgs = observed?.svgs ?? 0;
  const marks = observed?.svg_marks ?? 0;

  if (observed === null || observed === undefined) problems.push('the App frame was not observed');
  if (alerts.length) problems.push(`the App frame shows alerts: ${JSON.stringify(alerts)}`);
  if (svgs < 1) problems.push('no chart svg in the App frame');
  else if (marks < 1) problems.push('the chart svg has no mark element');

  if (!dataTable) problems.push(`no visible data table captioned ${expected.binding}`);
  else {
    if (dataTable.rows.length !== expected.rows) {
      problems.push(`the data table has ${dataTable.rows.length} body rows, the dataset has ${expected.rows}`);
    }
    if (!sameJson(dataTable.columns, expected.columns)) {
      problems.push(`the data table columns are ${JSON.stringify(dataTable.columns)}, the dataset has ${JSON.stringify(expected.columns)}`);
    }
    const shown = new Set(dataTable.rows.flat());
    const missing = [...new Set(expected.cells.flat())].filter((cell) => !shown.has(cell));
    if (missing.length) problems.push(`the data table does not show the dataset values ${JSON.stringify(missing)}`);
    else if (dataTable.rows.length === expected.rows && !sameJson(dataTable.rows, expected.cells)) {
      problems.push('the data table cells are not the dataset rows in order');
    }
    const unseen = [...new Set(expected.cells.flat())].filter((cell) => !(observed?.text ?? '').includes(cell));
    if (unseen.length) problems.push(`the dataset values ${JSON.stringify(unseen)} are not in the visible text`);
  }
  // SPEC §10: every chart has an accessible data-table representation.
  if (!chartTable) problems.push("the chart has no data table of its own (Chart's Source data)");
  else if (chartTable.rows.length !== expected.rows) {
    problems.push(`the chart's own data table has ${chartTable.rows.length} body rows, the dataset has ${expected.rows}`);
  }

  if (shows.length < 1) problems.push('the App did not call show');
  if (reads.length < 2) {
    problems.push(`the App called read_object for ${expected.digest} ${reads.length} time(s); a ranged read needs at least 2`);
  } else {
    if (!reads.some((call) => call.has_more === true)) problems.push('no read_object block reported has_more');
    if (reads.at(-1).has_more !== false) problems.push('the last read_object block still reported has_more');
    if (reads[0].offset !== '0') problems.push(`the first read_object block starts at ${reads[0].offset}, not 0`);
  }
  if (refused.length) problems.push(`the harness refused tool calls: ${JSON.stringify(refused)}`);

  return {
    ok: problems.length === 0,
    problems,
    record: {
      status: problems.length === 0 ? 'exercised' : 'failed',
      binding: expected.binding,
      digest: expected.digest,
      rows: expected.rows,
      read_object_calls: reads.length,
      read_object_offsets: reads.map((call) => call.offset),
      show_calls: shows.length,
      chart_svg: svgs >= 1 && marks >= 1,
      chart_marks: marks,
      table_rows: dataTable ? dataTable.rows.length : null,
      chart_table_rows: chartTable ? chartTable.rows.length : null,
      alerts,
      problems,
    },
  };
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
