/**
 * The criteria of the MCP Apps qualification and the envelope its receipt carries.
 *
 * Every judgement the run makes becomes one criterion; the receipt's result is
 * foldCriteria over them (scripts/lib/receipt-envelope.mjs) and nothing else decides it.
 * The ids a run can emit are fixed here and pinned in ../criteria.json: a criterion that is
 * not judged is still listed, as not_judged, so dropping a check is a visible change.
 *
 * A `fail` is a statement about the App bundle or about what the protocol libraries carried.
 * Anything the machine did (no browser, no network, a build that broke) is a harness error
 * and leaves its criteria not_judged.
 */

import { envelopeFailures, foldCriteria, statusOf } from '../../../scripts/lib/receipt-envelope.mjs';
import { PROTOCOL_CRITERIA, PROTOCOL_RULES } from './protocol.mjs';
import { datasetObservation } from './observe.mjs';
import { APP_RESOURCE_URI, UPSTREAM_HOST_RULES, VIEWS, judgePresentDataset, judgeView, partitionAxe } from './views.mjs';

/** The verification.json gate this receipt is evidence for. */
export const GATE = 'mcp-apps-protocol-qualification';

/**
 * Process exit codes, shared by every harness: 0 PASS, 2 FAIL, 3 INCOMPLETE; 1 (EXIT_REFUSED)
 * is a run that was refused before it started and wrote no receipt.
 */
export { EXIT_CODES, EXIT_REFUSED, exitCodeFor } from '../../../scripts/lib/receipt-envelope.mjs';

/**
 * What a reader of the receipt alone must know before reading "exercised" or a passing result:
 * which parts of the run were the product and which were this harness.
 */
export const SCOPE = Object.freeze({
  summary:
    'This run renders the real MCP App bundle in a real MCP Apps host, against a fixture MCP server. It qualifies the App side and the protocol libraries; it does not test the product server.',
  real: Object.freeze([
    'the MCP App bundle (ui/dist-apps/app.html), built by this run with the product build script',
    'the View code in that bundle: the product Source, Changes, Timeline and Present views, with the chart and data table',
    'the host implementation: the ext-apps basic-host example, in Chromium',
  ]),
  fixture: Object.freeze([
    'the MCP server is okf-qualify-mcp-apps, this harness; it is not the product server',
    'the render tools, show and read_object answer from committed files under tests/fixtures/views',
    'read_object is declared with the product schemas from api/mcp-tools.json, but its handler is the harness',
  ]),
  not_covered: Object.freeze([
    "the product server's show and read_object handlers",
    "the product blob store and the product's authorization of a read",
    'any host other than basic-host: claude.ai and ChatGPT belong to the gate mcp-apps-web-hosts',
  ]),
  reading_note:
    '"dataset_exercised" and present_dataset.status "exercised" mean the App fetched the fixture dataset through the harness read_object and drew it. They do not mean the product read_object tool was called.',
});

const BUNDLE_RULES = Object.freeze({
  'bundle/single_app_resource': `The bundle manifest this run built lists exactly one resource, ${APP_RESOURCE_URI}.`,
  'bundle/mime_type': "Every mimeType in that manifest is the ext-apps SDK's RESOURCE_MIME_TYPE.",
  'bundle/react_production_build': "The built App bundle does not contain react-dom's development build: its DevTools notice is absent from the file.",
  'bundle/served_as_html': 'The harness, loading the bundle the manifest names, reads every resource back through its own resources/read handler as an HTML document.',
});

const VIEW_RULES = Object.freeze({
  text: (view) =>
    `In the App frame of the reference host, ${view.tool} shows the ${view.view} view's own text for the committed fixture and none of another view's, and no waiting, disconnected or unparsed-result message.`,
  no_alert: () =>
    'The App frame holds no element with role="alert". Only the App frame is read: an alert in the host page or in the sandbox proxy frame is not seen by this rule.',
  accessibility: () => 'axe analysed the App frame and found no serious or critical violation in it.',
});

const DATASET_RULES = Object.freeze({
  chart_marks:
    'The chart svg holds exactly as many mark elements (children of g.role-mark) as the dataset has rows. Equality is right for this fixture because its chart is one unit bar with a plain nominal field against a plain quantitative field: Vega-Lite compiles that to one rect per row, and nothing aggregates, bins, stacks, filters or layers. The expectation is derived from the fixture; another specification is refused, not guessed.',
  table_rows:
    'A visible table captioned with the binding shows the dataset: the same columns, the same cells row by row in order, and every value in the visible text.',
  chart_source_table:
    'The chart\'s own "Source data" table (inside <details>) holds the dataset cell by cell: the same columns and the same rows in order.',
  show_calls: 'While the view rendered, the harness server logged at least one successful show call.',
  read_object_calls:
    "While the view rendered, the harness server logged successful read_object calls for the dataset's digest that form one or more complete reads: each starts at offset 0, every block starts where the one before ended, and the last ends at the dataset's size with has_more false, after more than one block.",
  no_refused_call: 'While the view rendered, the harness server refused no tool call.',
  dataset_exercised:
    'The App fetched the retained dataset through the harness read_object and drew it: every dataset rule of this view passed and the App frame showed no alert. Fixture evidence; the product server is not involved.',
});

/** The dataset rules judged from the App frame; the others are judged from the server's tool-call log. */
const FRAME_DATASET_RULES = ['chart_marks', 'table_rows', 'chart_source_table'];
const LOG_DATASET_RULES = ['show_calls', 'read_object_calls', 'no_refused_call'];

/** The criterion ids one view contributes, in the order they are judged. */
export function viewCriterionIds(view) {
  const rules = [...Object.keys(VIEW_RULES), ...(view.dataset ? Object.keys(DATASET_RULES) : [])];
  return rules.map((rule) => `${view.tool}/${rule}`);
}

/** Every criterion id a run of `views` emits, all required, in the order the run judges them. */
export function criterionIds(views = VIEWS) {
  return [...Object.keys(BUNDLE_RULES), ...PROTOCOL_CRITERIA, ...views.flatMap(viewCriterionIds)];
}

/** The same ids sorted: what ../criteria.json must list, exactly. */
export const requiredCriterionIds = (views = VIEWS) => criterionIds(views).sort();

/** Whether a criterion id belongs to a view render (needs the reference host and a browser). */
export const isRenderCriterion = (id) => id.startsWith('render_');

/** The rule behind each criterion id, in words. */
export function criterionRules(views = VIEWS) {
  const rules = { ...BUNDLE_RULES, ...PROTOCOL_RULES };
  for (const view of views) {
    for (const [rule, text] of Object.entries(VIEW_RULES)) rules[`${view.tool}/${rule}`] = text(view);
    if (view.dataset) for (const [rule, text] of Object.entries(DATASET_RULES)) rules[`${view.tool}/${rule}`] = text;
  }
  return rules;
}

const judged = (id, problems) =>
  problems.length ? { id, required: true, result: 'fail', detail: problems.join('; ') } : { id, required: true, result: 'pass' };
const notJudged = (id, detail) => ({ id, required: true, result: 'not_judged', detail });

/**
 * The bundle criteria. `built` is what the build left: `manifest_resources`, `sdk_mime_type`
 * and `react_development_build` (read from the bundle file). `check` is the harness's
 * --check report. Either may be null when that step never ran: its criteria are then absent.
 */
export function bundleCriteria({ built, check }) {
  const criteria = [];
  if (built) {
    const resources = built.manifest_resources;
    const single = [];
    if (resources.length !== 1) single.push(`the manifest lists ${resources.length} resources`);
    else if (resources[0].uri !== APP_RESOURCE_URI) single.push(`the manifest uri is ${resources[0].uri}`);
    const mismatched = resources.filter((resource) => resource.mimeType !== built.sdk_mime_type);
    criteria.push(
      judged('bundle/single_app_resource', single),
      judged(
        'bundle/mime_type',
        resources.length === 0
          ? ['the manifest lists no resource']
          : mismatched.map((resource) => `${resource.name} has mimeType ${resource.mimeType}, the SDK constant is ${built.sdk_mime_type}`),
      ),
      built.react_development_build === null
        ? notJudged('bundle/react_production_build', 'the manifest names no bundle file to read')
        : judged('bundle/react_production_build', built.react_development_build ? ["the bundle contains react-dom's development build"] : []),
    );
  }
  if (check) {
    const resources = check.resources ?? [];
    criteria.push(
      judged(
        'bundle/served_as_html',
        resources.length === 0
          ? ['the harness reported no resource']
          : resources.filter((resource) => resource.readable !== true).map((resource) => `${resource.name} was not read back as an HTML document`),
      ),
    );
  }
  return criteria;
}

/**
 * The criteria of one view from what observeView saw, the record the receipt keeps for it
 * and any harness error the observation implies. `expected` is the dataset expectation of a
 * view that has one, else null.
 */
export function viewCriteria(view, expected, observation) {
  const id = (rule) => `${view.tool}/${rule}`;
  const frame = observation.frame ?? null;
  const unseen = 'the App frame was not observed';
  const harness_errors = [];
  const verdict = judgeView(view, frame);

  const criteria = [
    judged(id('text'), [
      ...(frame === null ? [`${unseen}: no frame two levels below the host page showed a document`] : []),
      ...(verdict.missing.length ? [`missing ${JSON.stringify(verdict.missing)}`] : []),
      ...(verdict.foreign.length ? [`shows text of another state ${JSON.stringify(verdict.foreign)}`] : []),
      ...(verdict.blocking.length ? [`shows ${JSON.stringify(verdict.blocking)}`] : []),
    ]),
    frame === null
      ? notJudged(id('no_alert'), unseen)
      : judged(
          id('no_alert'),
          JSON.stringify(verdict.alerts) === JSON.stringify(verdict.expected_alerts) ? [] : [`the App frame shows alerts ${JSON.stringify(verdict.alerts)}`],
        ),
  ];

  let axe = null;
  if (!observation.axe) criteria.push(notJudged(id('accessibility'), 'axe runs only once the view has rendered'));
  else {
    axe = partitionAxe(observation.axe, observation.app_frame_depth ?? 0);
    if (!axe.app_frame_analysed) {
      criteria.push(notJudged(id('accessibility'), 'axe did not analyse the App frame'));
      harness_errors.push(`axe did not analyse the App frame of ${view.tool}`);
    } else {
      criteria.push(judged(id('accessibility'), axe.app_frame.length ? [`App frame serious/critical: ${JSON.stringify(axe.app_frame)}`] : []));
    }
    if (axe.host_blocking.length) {
      // The reference host's own page, not the App: nothing the product can fix.
      harness_errors.push(
        `the reference host's own chrome has serious or critical axe violations outside the tolerated upstream rules ${JSON.stringify(UPSTREAM_HOST_RULES)} while showing ${view.tool}: ${JSON.stringify(axe.host_blocking.map((rule) => rule.id))}`,
      );
    }
  }

  let dataset = null;
  if (view.dataset) {
    dataset = judgePresentDataset(expected, datasetObservation(observation));
    for (const rule of FRAME_DATASET_RULES) criteria.push(frame === null ? notJudged(id(rule), unseen) : judged(id(rule), dataset.checks[rule]));
    for (const rule of LOG_DATASET_RULES) criteria.push(judged(id(rule), dataset.checks[rule]));
    criteria.push(judged(id('dataset_exercised'), dataset.problems));
  }

  const failed = criteria.filter((criterion) => criterion.result === 'fail');
  return {
    criteria,
    harness_errors,
    record: {
      tool: view.tool,
      view: view.view,
      status: statusOf(foldCriteria(criteria, harness_errors[0] ?? null)),
      ...(failed.length ? { error: failed.map((criterion) => `${criterion.id}: ${criterion.detail}`).join('; ') } : {}),
      url: observation.url ?? null,
      screenshot: observation.screenshot ?? null,
      app_frame_depth: observation.app_frame_depth ?? null,
      feature_text: (frame?.text ?? '').slice(0, 400),
      alerts: verdict.alerts,
      expected_alerts: verdict.expected_alerts,
      ...(dataset ? { present_dataset: dataset.record } : {}),
      ...(axe ? { axe } : {}),
    },
  };
}

/**
 * The full list for the receipt: every id `views` can emit, in order, with the judged
 * criterion where there is one and not_judged (with `why(id)`) where there is none. A judged
 * criterion with an unknown id is kept at the end, so the seal can refuse it.
 */
export function completeCriteria(judgedCriteria, why, views = VIEWS) {
  const byId = new Map(judgedCriteria.map((criterion) => [criterion.id, criterion]));
  const ids = criterionIds(views);
  return [
    ...ids.map((id) => byId.get(id) ?? notJudged(id, why(id))),
    ...judgedCriteria.filter((criterion) => !ids.includes(criterion.id)),
  ];
}

/**
 * The envelope: gate, result, harness_error, criteria and not_judged. The result is the fold
 * and only the fold. `pinned` is the `required` list of ../criteria.json: when the criteria
 * given here are not exactly those (one missing, one not required, one extra), the harness
 * itself is wrong, which is a harness error and so never a PASS.
 */
export function sealEnvelope({ criteria, harnessErrors = [], pinned }) {
  const errors = [...harnessErrors];
  const draft = { gate: GATE, result: foldCriteria(criteria, null), harness_error: null, criteria };
  const required = Array.isArray(criteria) ? criteria.filter((criterion) => criterion?.required === true).map((criterion) => criterion.id) : [];
  const drift = [
    ...envelopeFailures(draft, GATE, pinned),
    ...required.filter((id) => !pinned.includes(id)).map((id) => `required criterion ${id} is not pinned`),
  ];
  if (drift.length) errors.push(`the criteria this harness emitted are not the ones criteria.json pins: ${drift.join('; ')}`);
  const harness_error = errors.length ? errors.join('; ') : null;
  return {
    gate: GATE,
    result: foldCriteria(criteria, harness_error),
    harness_error,
    criteria,
    not_judged: Array.isArray(criteria)
      ? criteria.filter((criterion) => criterion?.result === 'not_judged' || criterion?.result === 'not_applicable').map((criterion) => criterion.id)
      : [],
  };
}

/** The status word of one section of the receipt, derived from its criteria: passed, failed, incomplete, or not_run when none was judged. */
export function sectionStatus(criteria, belongs) {
  const section = criteria.filter((criterion) => belongs(criterion.id));
  if (!section.some((criterion) => criterion.result === 'pass' || criterion.result === 'fail')) return 'not_run';
  return statusOf(foldCriteria(section, null));
}
