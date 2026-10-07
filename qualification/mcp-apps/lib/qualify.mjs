/**
 * One MCP Apps qualification run: the order of its steps and what each failure means.
 *
 * Every effect (git, the UI build, cargo, the harness process, the MCP client, the reference
 * host, the browser, the files written) is injected, so the rules below are tested without
 * any of them:
 *
 * - The previous receipt is removed before anything else. A run that cannot produce a
 *   receipt (a dirty tree has no commit to cite; a killed process writes nothing) must not
 *   leave another run's receipt in its place.
 * - Once the tree is known clean, every run writes a receipt. A step the machine could not
 *   do (build, start, connect, launch) is recorded as harness_error; the criteria it would
 *   have judged are listed not_judged, and the result folds to INCOMPLETE, never FAIL.
 *   harness_error is one sentence per stopped step (what stopped, and the first line of why);
 *   harness_error_detail keeps the whole message for whoever has to repair the machine.
 * - The receipt is complete and sealed before it is written; its result is the fold of its
 *   criteria. A run records nothing: `bun qualification/record.mjs mcp-apps` records a
 *   finished receipt of any result, and refuses one whose envelope cannot be trusted.
 */

import { scrubReceiptPaths } from '../../../scripts/lib/provenance.mjs';
import {
  GATE,
  SCOPE,
  bundleCriteria,
  completeCriteria,
  criterionRules,
  exitCodeFor,
  isRenderCriterion,
  sealEnvelope,
  sectionStatus,
  viewCriteria,
} from './criteria.mjs';
import { judgeProtocol } from './protocol.mjs';
import { APP_ONLY_TOOLS, APP_RESOURCE_URI, UPSTREAM_HOST_RULES, VIEWS, appBundleBuild, ngrokRecord, transportRecord } from './views.mjs';

const SKIPPED = 'skipped: protocol-only run';
/** The first line of a message, bounded: a harness_error is a sentence, not a log. */
const firstLine = (text) => (String(text).split(/\r?\n/).map((line) => line.trim()).find(Boolean) ?? '').slice(0, 300);
/** The sentence for one stopped step. */
const sentence = ({ what, detail }) => (detail === null || firstLine(detail) === '' ? what : `${what}: ${firstLine(detail)}`);

/**
 * Run the qualification through `effects` and return `{ receipt, exitCode }`.
 * `options`: `protocolOnly`, `pinned` (criteria.json's required ids), `config`
 * (`harness`, `mcp_url`, `http_port`, `ngrok`), `paths` (pathContext of scripts/lib/provenance.mjs, which
 * every path in the receipt is rewritten against; required, so a run cannot write raw paths) and, for tests, `views`.
 */
export async function qualify(effects, { protocolOnly = false, pinned, config, paths, views = VIEWS }) {
  await effects.removeReceipt();
  // A dirty tree throws here: there is no commit a receipt could cite.
  const header = await effects.header();

  /** What the machine could not do, in order: `what` stopped and, where there is one, the whole message. */
  const harnessErrors = [];
  const stopped = (what, detail = null) => harnessErrors.push({ what, detail: detail === null ? null : String(detail).slice(0, 4000) });
  const judgedCriteria = [];
  const attempt = async (what, effect) => {
    try {
      return await effect();
    } catch (error) {
      stopped(what, error?.message ?? error);
      return null;
    }
  };

  let built = null;
  let harness = null;
  let check = null;
  let expectations = null;
  let server = null;
  let serverExit = null;
  let protocol = null;
  let tunnel = null;
  let host = null;
  let browser = null;
  const viewRecords = [];

  try {
    const bundle = await attempt('the App bundle was not built', () => effects.buildBundle());
    if (bundle) {
      const html = bundle.app_html;
      built = {
        manifest_resources: bundle.manifest_resources,
        sdk_mime_type: bundle.sdk_mime_type,
        app_bundle: typeof html === 'string' ? appBundleBuild({ html, nodeEnv: bundle.node_env }) : null,
      };
      built.react_development_build = built.app_bundle ? built.app_bundle.react_development_build : null;
    }
    harness = built && (await attempt('the harness binary was not built', () => effects.buildHarness()));
    expectations = harness && (await attempt('the committed fixtures could not be read as an expectation', () => effects.expectations()));
    const checked = expectations && (await attempt('the harness --check did not run', () => effects.checkHarness(harness)));
    if (checked) {
      const unreadable = (checked.report?.resources ?? []).some((resource) => resource.readable !== true);
      if (!checked.report) stopped(`the harness --check printed no report (exit ${checked.code})`, checked.stderr);
      else if (checked.code !== 0 && !unreadable) stopped(`the harness --check exited ${checked.code}`, checked.stderr);
      else if (checked.report.dataset?.sha256 !== expectations.dataset.digest) {
        stopped(`the harness serves dataset ${checked.report.dataset?.sha256}; the committed fixture is ${expectations.dataset.digest}`);
      }
      check = checked.report ?? null;
    }
    judgedCriteria.push(...bundleCriteria({ built, check }));

    const ready = check !== null && harnessErrors.length === 0;
    server = ready ? await attempt('the harness server did not start', () => effects.startHarness(harness)) : null;
    if (server) {
      const observed = await attempt('the MCP client could not reach the harness server', () => effects.observeProtocol(server));
      if (observed) {
        protocol = judgeProtocol(
          {
            render_tools: views.map((view) => view.tool),
            app_only_tools: APP_ONLY_TOOLS,
            resource_uri: APP_RESOURCE_URI,
            mime_type: built.sdk_mime_type,
            bundle_sha256: built.manifest_resources.find((resource) => resource.uri === APP_RESOURCE_URI)?.sha256 ?? null,
            product_read_object: expectations.product_read_object,
          },
          observed,
        );
        judgedCriteria.push(...protocol.criteria);
      }
    }

    if (server && !protocolOnly) {
      if (config.ngrok) tunnel = await effects.openTunnel();
      host = harnessErrors.length === 0 ? await attempt('the reference host (basic-host) did not start', () => effects.startHost(server)) : null;
      browser = host ? await attempt('Chromium did not start', () => effects.launchBrowser(server)) : null;
      if (browser) {
        for (const view of views) {
          const observation = await attempt(`${view.tool} could not be shown in the reference host`, () =>
            browser.render(view, view.dataset ? expectations.dataset : null),
          );
          if (!observation) continue;
          const viewed = viewCriteria(view, view.dataset ? expectations.dataset : null, observation);
          judgedCriteria.push(...viewed.criteria);
          for (const error of viewed.harness_errors) stopped(error);
          viewRecords.push(viewed.record);
        }
      }
    }
  } finally {
    if (browser) await browser.close();
    if (host) await host.stop();
    if (tunnel) tunnel.closed_at = await tunnel.close();
    if (server) serverExit = await server.stop();
  }

  const why = (id) => {
    if (protocolOnly && isRenderCriterion(id)) return SKIPPED;
    return harnessErrors.length ? `not reached: ${harnessErrors[0].what}` : 'this run made no judgement for it';
  };
  const criteria = completeCriteria(judgedCriteria, why, views);
  const envelope = sealEnvelope({ criteria, harnessErrors: harnessErrors.map(sentence), pinned });
  const skipped = protocolOnly ? SKIPPED : 'OKF_MCP_APPS_NGROK is not 1';

  const unscrubbed = {
    ...header,
    component: GATE,
    ...envelope,
    harness_error_detail: harnessErrors,
    finished_at: new Date().toISOString(),
    scope: SCOPE,
    rules: criterionRules(views),
    harness: config.harness,
    protocol_only: protocolOnly,
    transport: transportRecord({ requested: config.mcp_url, reported: server?.endpoint ?? null }),
    harness_binary: harness,
    harness_process: serverExit,
    mime_check: built && {
      sdk_constant: built.sdk_mime_type,
      manifest_mime_types: built.manifest_resources.map((resource) => ({ name: resource.name, mimeType: resource.mimeType })),
    },
    bundle_sizes: built && Object.fromEntries(built.manifest_resources.map((resource) => [resource.name, resource.byteLength])),
    app_bundle: built?.app_bundle ?? null,
    resources_readable: check && Object.fromEntries((check.resources ?? []).map((resource) => [resource.name, resource.readable === true])),
    check,
    protocol_check: { status: sectionStatus(criteria, (id) => id.startsWith('protocol/')), ...(protocol?.record ?? {}) },
    basic_host: {
      status: sectionStatus(criteria, isRenderCriterion),
      ...(protocolOnly ? { reason: SKIPPED } : {}),
      source: host?.source ?? null,
      browser: browser ? { name: 'chromium', version: browser.version } : null,
      views: viewRecords,
      axe_tolerated_upstream_host_rules: UPSTREAM_HOST_RULES,
      present_dataset: viewRecords.find((item) => item.present_dataset)?.present_dataset ?? {
        status: 'not_observed',
        reason: 'no view reached the point where its dataset rendering is observed',
      },
      note: 'The ext-apps basic-host example is a reference host; this is not a claude.ai or ChatGPT claim.',
    },
    ngrok: ngrokRecord({
      enabled: Boolean(config.ngrok) && !protocolOnly,
      opened_at: tunnel?.opened_at ?? null,
      closed_at: tunnel?.closed_at ?? null,
      public_url: tunnel?.public_url ?? null,
      local_port: config.http_port,
      note: tunnel?.note ?? skipped,
    }),
    host_render: protocolOnly ? { status: 'not_run', reason: SKIPPED } : await effects.hostRenderEvidence(),
    static_bundle_smoke: {
      status: 'not_run',
      reason:
        'ui/tests/e2e/mcp-apps-static-bundle-smoke.spec.ts is a separate smoke test; this orchestrator does not run it and it is not a host-render check',
    },
    note: 'host_render (claude.ai / ChatGPT) belongs to the acceptance gate mcp-apps-web-hosts and is never invented here.',
  };

  // Every path in the receipt is written by one rule, here and nowhere else (scripts/lib/provenance.mjs).
  const receipt = scrubReceiptPaths(unscrubbed, paths);

  // Nothing was written until here: the result is known and is the fold of the criteria.
  await effects.writeReceipt(receipt);
  return { receipt, exitCode: exitCodeFor(receipt.result) };
}
