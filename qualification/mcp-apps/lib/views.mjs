/**
 * What the MCP Apps qualification expects to see, and how it judges what it saw.
 * Pure: no I/O and no imports, so every rule is testable without a browser. Nothing here
 * decides the run: lib/criteria.mjs turns these judgements into criteria and the fold decides.
 *
 * Expected strings come from tests/fixtures/views/*.json as rendered by
 * ui/src/features/{documents/SourceExcerpt,history/Changes,history/Timeline,views/Layout}.tsx.
 * No view tolerates an alert. The present fixture retains a dataset
 * (tests/fixtures/views/present-metrics.dataset.json), so its DataTable and Chart must be
 * drawn from it: the "unavailable" text they show without one is refused by name, and
 * judgePresentDataset decides from the observed chart, tables and tool calls.
 *
 * In the record judgePresentDataset returns, a field named `expected_...` is what the
 * committed fixtures say; every other field is something the App frame or the harness
 * server's own tool-call log showed during the render.
 */

export const APP_RESOURCE_URI = 'ui://okf-jawn/app.html';
export const APP_ONLY_TOOLS = ['read_object', 'show'];
/** Prefix of the stderr line the harness writes per tool call (TOOL_CALL_LOG_PREFIX in src/main.rs). */
export const TOOL_CALL_LOG_PREFIX = 'okf-qualify-mcp-apps tool-call ';
/** Where the present view's retained dataset lives, which binding names it and which fixture draws it. */
export const PRESENT_DATASET = {
  binding: 'metrics',
  fixture: 'tests/fixtures/views/present-metrics.dataset.json',
  present: 'tests/fixtures/views/present-response.json',
};
/**
 * Repo-relative paths whose change after the cited commit makes an MCP Apps receipt stale:
 * everything the run executes or renders. Sorted; the reason stands beside each path.
 */
export const MCP_APPS_INPUTS = [
  '.bun-version', // the Bun that runs the bundler, this orchestrator and basic-host
  '.cargo/config.toml', // cargo settings the harness binary is built under
  'Cargo.lock', // crate versions the harness binary is built from
  'Cargo.toml', // workspace dependency pins, lints and release profile of the harness
  'api/mcp-apps.json', // the App declaration ui/scripts/bundle-app.mjs builds the manifest from
  'api/mcp-tools.json', // the product read_object declaration compiled into the harness binary
  'bun.lock', // versions of the App's packages, the MCP client, Playwright and axe
  'bunfig.toml', // how Bun installs and runs here: the isolated linker and env = false
  'package.json', // the workspace and overrides bun.lock is resolved under
  'qualification/lib', // cargo.mjs: how the harness binary is built and found
  'qualification/mcp-apps', // this orchestrator, its rules, criteria.json and the harness server
  'rust-toolchain.toml', // the compiler that builds the harness
  'scripts/lib/provenance.mjs', // the receipt header and the clean-tree rule
  'scripts/lib/receipt-envelope.mjs', // the fold that decides the result and the envelope rules
  'tests/fixtures/views', // what the render tools, show and read_object serve
  // Of ui/, only what the run builds from. The App bundle's bytes depend on the modules
  // reachable from src/mcp-apps/main.tsx and on Tailwind class words anywhere under ui/src
  // (styles.css narrows the scan with source("./")), on scripts/bundle-app.mjs with
  // scripts/app-declaration.ts, and on the package versions. `bun --bun run build` also runs
  // the workspace build first (vite.config.ts, index.html) and scripts/bundle-docs.mjs, and
  // Vite resolves tsconfig.json for the TypeScript it transforms. Tests, lint and test
  // configuration and prose under ui/ are not read by anything this run executes.
  'ui/index.html',
  'ui/package.json',
  'ui/scripts',
  'ui/src',
  'ui/tsconfig.json',
  'ui/vite.config.ts',
];
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

/** The Vega-Lite specification the present fixture's one Chart element draws for `binding`. */
export function chartForBinding(present, binding) {
  const elements = Object.values(present?.view?.spec?.elements ?? {});
  const charts = elements.filter((element) => element?.type === 'Chart' && element.props?.binding === binding);
  if (charts.length !== 1) {
    throw new Error(`the present fixture must have exactly one Chart element bound to ${binding}; found ${charts.length}`);
  }
  const name = charts[0].props.chart;
  const spec = present.view.charts?.[name];
  if (spec === null || typeof spec !== 'object') throw new Error(`the present fixture has no chart named ${name}`);
  return spec;
}

/**
 * How many mark elements Vega draws for `spec` over `rows`.
 *
 * Only one shape is known here, the fixture's: a single unit `bar` with a plain nominal or
 * ordinal field on one axis and a plain quantitative field on the other. Vega-Lite compiles
 * that to one rect mark whose data is the dataset itself: nothing aggregates, bins, stacks,
 * filters or layers, so every row is one rect and the count equals the row count by
 * construction. A row whose quantitative value is not a finite number would be dropped
 * (Vega-Lite filters invalid values), so such a dataset is refused as well. Any other
 * specification has another expectation and is refused here instead of guessed.
 */
export function expectedChartMarks(spec, rows) {
  const refuse = (why) => {
    throw new Error(`the chart's mark count is not the dataset's row count by construction: ${why}`);
  };
  if (spec === null || typeof spec !== 'object' || Array.isArray(spec)) refuse('the specification is not an object');
  const beyondOneUnit = ['layer', 'concat', 'hconcat', 'vconcat', 'facet', 'repeat', 'spec', 'transform', 'params'].filter((key) => key in spec);
  if (beyondOneUnit.length) refuse(`it has ${beyondOneUnit.join(', ')}`);
  if (spec.mark !== 'bar') refuse(`its mark is ${JSON.stringify(spec.mark)}, not "bar"`);
  const encoding = spec.encoding ?? {};
  if (!sameJson(Object.keys(encoding).sort(), ['x', 'y'])) refuse(`it encodes ${JSON.stringify(Object.keys(encoding))}, not exactly x and y`);
  for (const channel of ['x', 'y']) {
    const definition = encoding[channel];
    if (definition === null || typeof definition !== 'object' || !sameJson(Object.keys(definition).sort(), ['field', 'type'])) {
      refuse(`channel ${channel} is ${JSON.stringify(definition)}, not a plain { field, type }`);
    }
  }
  const types = [encoding.x.type, encoding.y.type];
  const measures = ['x', 'y'].filter((channel) => encoding[channel].type === 'quantitative');
  const categories = ['x', 'y'].filter((channel) => ['nominal', 'ordinal'].includes(encoding[channel].type));
  if (measures.length !== 1 || categories.length !== 1) refuse(`its axes are ${JSON.stringify(types)}, not one category and one quantity`);
  const measure = encoding[measures[0]].field;
  const category = encoding[categories[0]].field;
  for (const [index, row] of rows.entries()) {
    if (typeof row[measure] !== 'number' || !Number.isFinite(row[measure])) refuse(`row ${index} has no finite ${measure}`);
    if (row[category] === null || row[category] === undefined) refuse(`row ${index} has no ${category}`);
  }
  return rows.length;
}

/**
 * The records a chart and a table draw from a typed Dataset, one per row keyed by column name.
 *
 * Judged as strictly as ui/src/features/views/PresentView.tsx `parseDataset`: the Dataset must be
 * an object with a non-empty `columns` array, `text_origin` "converter" (SPEC R5, decision O2),
 * distinct column names, and every row exactly as wide as the columns. Nothing is padded or cut.
 */
export function datasetRecords(dataset) {
  if (dataset === null || typeof dataset !== 'object' || Array.isArray(dataset)) {
    throw new Error('the dataset must be a Dataset object with columns and rows, not bare records');
  }
  if (dataset.text_origin !== 'converter') {
    throw new Error(`the dataset text is not the converter's (text_origin is ${JSON.stringify(dataset.text_origin)}); no chart is drawn from it`);
  }
  if (!Array.isArray(dataset.columns) || dataset.columns.length === 0) throw new Error('the dataset must have a non-empty columns array');
  if (!Array.isArray(dataset.rows)) throw new Error('the dataset must have a rows array');
  const names = new Set();
  for (const column of dataset.columns) {
    if (typeof column?.name !== 'string') throw new Error('every dataset column must have a name');
    if (names.has(column.name)) throw new Error(`the dataset has two columns named "${column.name}"`);
    names.add(column.name);
  }
  return dataset.rows.map((cells, index) => {
    if (!Array.isArray(cells) || cells.length !== dataset.columns.length) {
      throw new Error(`dataset row ${index} has ${Array.isArray(cells) ? cells.length : 'no'} values for ${dataset.columns.length} columns`);
    }
    return Object.fromEntries(dataset.columns.map((column, at) => [column.name, cells[at] ?? null]));
  });
}

/**
 * What the dataset must look like once rendered: DataTable's own projection of the records
 * (ui/src/features/views/DataTable.tsx: columns in dataset order, String(value ?? '')), and the
 * number of marks its chart draws. `dataset` is the parsed typed Dataset fixture, `digest` the
 * SHA-256 of its exact bytes, `bytes` their count and `chart` the specification that draws it.
 */
export function datasetExpectation({ binding, digest, dataset, bytes, chart }) {
  const rows = datasetRecords(dataset);
  if (rows.length === 0) throw new Error('the dataset must have a non-empty rows array');
  if (!Number.isInteger(bytes) || bytes <= 0) throw new Error('the dataset byte length must be a positive integer');
  const columns = dataset.columns.map((column) => column.name);
  return {
    binding,
    digest,
    bytes,
    rows: rows.length,
    columns,
    cells: rows.map((row) => columns.map((column) => String(row[column] ?? ''))),
    chart_marks: expectedChartMarks(chart, rows),
  };
}

const sameJson = (left, right) => JSON.stringify(left) === JSON.stringify(right);

/** Why `table` is not the dataset, cell by cell; empty when it is. `name` starts each sentence. */
function tableProblems(name, table, expected) {
  const problems = [];
  if (!sameJson(table.columns, expected.columns)) {
    problems.push(`${name} columns are ${JSON.stringify(table.columns)}, the dataset has ${JSON.stringify(expected.columns)}`);
  }
  if (table.rows.length !== expected.rows) {
    problems.push(`${name} has ${table.rows.length} body rows, the dataset has ${expected.rows}`);
    return problems;
  }
  const shown = new Set(table.rows.flat());
  const missing = [...new Set(expected.cells.flat())].filter((cell) => !shown.has(cell));
  if (missing.length) problems.push(`${name} does not show the dataset values ${JSON.stringify(missing)}`);
  else if (!sameJson(table.rows, expected.cells)) problems.push(`${name} cells are not the dataset rows in order`);
  return problems;
}

/**
 * Group the App's successful `read_object` reports for one object into resolutions of a
 * binding: each starts at offset 0 and every later block must start where an earlier one
 * ended, until a block says there is no more, at exactly `totalBytes`. Two resolutions may
 * interleave (a development build of React runs the effect twice); a gap, an overlap, a
 * read that stops early or one that never finishes is a problem.
 */
export function readResolutions(reads, totalBytes) {
  const problems = new Set();
  const open = [];
  let complete = 0;
  for (const read of reads) {
    const offset = typeof read.offset === 'string' && /^\d+$/.test(read.offset) ? Number(read.offset) : null;
    if (offset === null || !Number.isInteger(read.bytes) || read.bytes < 0) {
      problems.add(`a read_object report carries no usable range: ${JSON.stringify(read)}`);
      continue;
    }
    if (read.total_size !== String(totalBytes)) {
      problems.add(`read_object reported total_size ${JSON.stringify(read.total_size)}, the dataset has ${totalBytes} bytes`);
      continue;
    }
    let resolution = null;
    if (offset === 0) {
      resolution = { next: 0, blocks: 0 };
      open.push(resolution);
    } else {
      resolution = open.find((item) => item.next === offset) ?? null;
    }
    if (resolution === null) {
      problems.add(`a read_object block starts at ${offset}, where no read from offset 0 had arrived`);
      continue;
    }
    resolution.next += read.bytes;
    resolution.blocks += 1;
    if (read.has_more === true) {
      if (read.bytes === 0) problems.add(`the read_object block at ${offset} made no progress`);
      continue;
    }
    open.splice(open.indexOf(resolution), 1);
    if (resolution.next !== totalBytes) problems.add(`a read ended at byte ${resolution.next} of ${totalBytes}`);
    else if (resolution.blocks < 2) problems.add('a read returned the dataset in one block; the ranged loop was not exercised');
    else complete += 1;
  }
  for (const resolution of open) {
    problems.add(`a read that started at offset 0 stopped at byte ${resolution.next} of ${totalBytes} without a final block`);
  }
  return { complete, problems: [...problems] };
}

/**
 * Whether the present view rendered its retained dataset, judged only from what was seen:
 * `observed.alerts` and `observed.text` (App frame), `observed.svgs` / `observed.svg_marks`
 * (the Vega chart), `observed.tables` (every <table>: caption, visible, in_details, columns,
 * rows of cell text) and `observed.tool_calls` (harness reports while this view rendered).
 * `checks` holds the problems of each rule separately; `record` is what the receipt states,
 * and its status is "exercised" only when no rule has a problem.
 */
export function judgePresentDataset(expected, observed) {
  const alerts = [...(observed?.alerts ?? [])].map((alert) => alert.trim());
  const tables = observed?.tables ?? [];
  const bound = tables.filter((table) => table.caption === expected.binding);
  const dataTable = bound.find((table) => table.visible && !table.in_details) ?? null;
  const chartTable = bound.find((table) => table.in_details) ?? null;
  const calls = observed?.tool_calls ?? [];
  const objectReads = calls.filter((call) => call.tool === 'read_object' && call.ok === true);
  const reads = objectReads.filter((call) => call.object === expected.digest);
  const shows = calls.filter((call) => call.tool === 'show' && call.ok === true);
  const refused = calls.filter((call) => call.ok !== true);
  const svgs = observed?.svgs ?? 0;
  const marks = observed?.svg_marks ?? 0;
  const resolutions = readResolutions(reads, expected.bytes);

  const checks = {
    observed: observed === null || observed === undefined ? ['the App frame was not observed'] : [],
    no_alert: alerts.length ? [`the App frame shows alerts: ${JSON.stringify(alerts)}`] : [],
    chart_marks: [],
    table_rows: [],
    chart_source_table: [],
    show_calls: shows.length < 1 ? ['the App did not call show'] : [],
    read_object_calls: [],
    no_refused_call: refused.length ? [`the harness refused tool calls: ${JSON.stringify(refused)}`] : [],
  };
  if (svgs < 1) checks.chart_marks.push('no chart svg in the App frame');
  else if (marks !== expected.chart_marks) {
    checks.chart_marks.push(`the chart svg has ${marks} mark element(s); the dataset's ${expected.rows} rows draw ${expected.chart_marks}`);
  }

  if (!dataTable) checks.table_rows.push(`no visible data table captioned ${expected.binding}`);
  else {
    checks.table_rows.push(...tableProblems('the data table', dataTable, expected));
    const unseen = [...new Set(expected.cells.flat())].filter((cell) => !(observed?.text ?? '').includes(cell));
    if (unseen.length) checks.table_rows.push(`the dataset values ${JSON.stringify(unseen)} are not in the visible text`);
  }
  // SPEC §10: every chart has an accessible data-table representation; it must be the data.
  if (!chartTable) checks.chart_source_table.push("the chart has no data table of its own (Chart's Source data)");
  else checks.chart_source_table.push(...tableProblems("the chart's own data table", chartTable, expected));

  if (reads.length === 0) checks.read_object_calls.push(`the App called read_object for ${expected.digest} 0 time(s)`);
  else {
    checks.read_object_calls.push(...resolutions.problems);
    if (resolutions.complete < 1 && resolutions.problems.length === 0) checks.read_object_calls.push('no read of the dataset finished');
  }

  const problems = Object.values(checks).flat();
  return {
    ok: problems.length === 0,
    problems,
    checks,
    record: {
      status: problems.length === 0 ? 'exercised' : 'failed',
      expected_binding: expected.binding,
      expected_digest: expected.digest,
      expected_bytes: expected.bytes,
      expected_rows: expected.rows,
      expected_chart_marks: expected.chart_marks,
      table_captions: tables.map((table) => table.caption),
      read_object_digests: [...new Set(objectReads.map((call) => call.object))],
      read_object_calls: reads.length,
      read_object_offsets: reads.map((call) => call.offset),
      read_object_bytes: reads.map((call) => call.bytes ?? null),
      read_object_total_sizes: [...new Set(reads.map((call) => call.total_size ?? null))],
      read_object_resolutions: resolutions.complete,
      show_calls: shows.length,
      chart_svgs: svgs,
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

/** Text that only react-dom's development build carries. */
export const REACT_DEVELOPMENT_MARKER = 'Download the React DevTools';

/**
 * Which React build the rendered App bundle contains, read from the bundle itself.
 * ui/scripts/bundle-app.mjs forces NODE_ENV to production, so the caller's `nodeEnv` is
 * recorded only as the environment the run had. A development build would show here, and in
 * the reads: its StrictMode runs every effect twice, so the App resolves each binding twice.
 */
export function appBundleBuild({ html, nodeEnv }) {
  return {
    node_env: typeof nodeEnv === 'string' ? nodeEnv : null,
    react_development_build: String(html).includes(REACT_DEVELOPMENT_MARKER),
    marker: REACT_DEVELOPMENT_MARKER,
  };
}

/**
 * Which transports the run exercised. This orchestrator never drives stdio, so the receipt
 * says so instead of naming it; HTTP is the endpoint the harness itself reported, if any.
 */
export function transportRecord({ requested, reported }) {
  const listening = typeof reported === 'string' && reported.length > 0;
  return {
    stdio: { status: 'not_run', reason: 'this orchestrator drives the harness over Streamable HTTP only' },
    http: listening
      ? { status: 'listening', endpoint: reported, requested }
      : { status: 'not_listening', endpoint: null, requested },
  };
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
