/**
 * How one view render in the reference host is watched.
 *
 * The browser is reached only through a driver (open, hostText, frames, readFrame, toolLog,
 * wait, screenshot, axe, now), so the order of the watch, which tool calls count as this
 * view's and when waiting stops are decided here and tested with a driver double. run.mjs
 * supplies the Playwright driver and nothing else.
 *
 * readAppDom is the one function that runs inside the App document. Playwright sends its
 * source to the page, so it may use nothing but its arguments and the page's globals.
 */

import { basicHostUrl, judgePresentDataset, judgeView, toolCallsFrom } from './views.mjs';

/**
 * What is read from the App document. Vega's SVG renderer draws a specification's own marks
 * as children of g.role-mark; axes, legends and titles carry other roles, so they are not
 * counted. A table inside closed <details> has no client rects.
 */
export const DOM_SELECTORS = Object.freeze({
  alert: '[role="alert"]',
  svg: 'svg',
  chart_mark: 'svg g[class~="role-mark"] > *',
  table: 'table',
  header_cell: 'thead th',
});

/** The text, alerts, chart marks and tables of the document this runs in (`doc`). */
export function readAppDom(selectors, doc = document) {
  return {
    text: doc.body?.innerText ?? '',
    alerts: Array.from(doc.querySelectorAll(selectors.alert), (node) => node.innerText ?? ''),
    svgs: doc.querySelectorAll(selectors.svg).length,
    svg_marks: doc.querySelectorAll(selectors.chart_mark).length,
    tables: Array.from(doc.querySelectorAll(selectors.table), (table) => ({
      caption: table.caption?.textContent ?? '',
      visible: table.getClientRects().length > 0,
      in_details: table.closest('details') !== null,
      columns: Array.from(table.querySelectorAll(selectors.header_cell), (cell) => cell.textContent ?? ''),
      rows: Array.from(table.tBodies[0]?.rows ?? [], (row) => Array.from(row.cells, (cell) => cell.textContent ?? '')),
    })),
  };
}

/** A failure of the reference host, the browser or the harness server: never a verdict on the App. */
export class HarnessError extends Error {}

/** What basic-host shows when it cannot reach the MCP server it was given. */
const HOST_UNREACHABLE = /Failed to connect to any servers/i;

function frameDepth(frame) {
  let depth = 0;
  for (let parent = frame.parentFrame(); parent; parent = parent.parentFrame()) depth += 1;
  return depth;
}

/** The App document among a page's frames: host page (0) -> sandbox proxy (1) -> App (2); the deepest wins. */
export function appFrameOf(frames) {
  return (
    frames
      .map((frame) => ({ frame, depth: frameDepth(frame) }))
      .filter((item) => item.depth >= 2)
      .sort((a, b) => b.depth - a.depth)[0] ?? null
  );
}

/**
 * What judgePresentDataset is given: the App frame together with this view's tool calls. A
 * frame that was never seen is an empty document, so the tool calls the server logged are
 * still counted as what they were.
 */
export const datasetObservation = (observation) => ({
  text: '',
  alerts: [],
  svgs: 0,
  svg_marks: 0,
  tables: [],
  ...(observation.frame ?? {}),
  tool_calls: observation.tool_calls ?? [],
});

/**
 * Whether waiting longer can show nothing more: the view shows its text without an alert and,
 * where it has a dataset (`expected`), the dataset is drawn and read.
 */
export function viewSettled(view, expected, observation) {
  if (!judgeView(view, observation.frame).ok) return false;
  return expected === null || judgePresentDataset(expected, datasetObservation(observation)).ok;
}

/**
 * Open `view` in the reference host and watch until it has settled or `timeoutMs` has passed.
 * Returns what was seen last: `frame` (readAppDom of the App document, or null), `tool_calls`
 * (only those the harness reported after this view was opened), the screenshot and, for a
 * settled view, the axe results. A host that cannot reach the harness, or any driver call
 * that fails, throws: that is the environment, not the view.
 */
export async function observeView(driver, view, expected, { timeoutMs = 30_000, pollMs = 500 } = {}) {
  const url = basicHostUrl(view.tool);
  // Tool calls reported from here on belong to this view's render; earlier ones (the protocol
  // check, the views before) do not.
  const logStart = driver.toolLog().length;
  await driver.open(url);
  const deadline = driver.now() + timeoutMs;
  let observation;
  for (;;) {
    const hostText = await driver.hostText();
    if (HOST_UNREACHABLE.test(hostText)) {
      throw new HarnessError(`basic-host could not reach the harness server: ${hostText.slice(0, 200)}`);
    }
    const located = appFrameOf(driver.frames());
    observation = {
      url,
      app_frame_depth: located?.depth ?? null,
      frame: located ? await driver.readFrame(located.frame) : null,
      tool_calls: toolCallsFrom(driver.toolLog().slice(logStart)),
    };
    observation.settled = viewSettled(view, expected, observation);
    if (observation.settled || driver.now() >= deadline) break;
    await driver.wait(pollMs);
  }
  observation.screenshot = await driver.screenshot(view.tool);
  // axe runs on the whole page, and only once the view is drawn.
  observation.axe = observation.settled ? await driver.axe() : null;
  return observation;
}
