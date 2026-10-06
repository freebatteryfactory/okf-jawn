/**
 * Orchestrate Phase 0 MCP Apps protocol qualification.
 *
 * Steps: rebuild dist-apps, build the harness binary, harness --check, Streamable HTTP
 * protocol check over all four render tools and the app-only show and read_object tools,
 * then (unless OKF_MCP_APPS_PROTOCOL_ONLY=1) the official basic-host example rendering each
 * of the four views under Playwright with axe, and an optional ngrok URL
 * (OKF_MCP_APPS_NGROK=1) that is always closed before the receipt is written.
 *
 * This file only performs effects. The order of the run, what a failed step means and when a
 * receipt is written or recorded are in lib/qualify.mjs; what is judged is in lib/criteria.mjs,
 * lib/protocol.mjs and lib/views.mjs; how a view is watched is in lib/observe.mjs.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/mcp-apps/run.mjs [--record]
 * Exit code (scripts/lib/receipt-envelope.mjs, the same numbers as every harness): 0 PASS,
 * 2 FAIL (a criterion about the App or the protocol failed), 3 INCOMPLETE (something was not
 * judged: a protocol-only run, a missing browser, a build that broke). 1 is a run refused
 * before it started, with no receipt written: a dirty tree, or a defect in this script. The
 * receipt's result is the fold of its criteria; the receipt of the previous run is removed
 * first, so the file never describes another run.
 * host_render reports only evidence found under .artifacts/.../hosts/ and never a PASS.
 *
 * Every receipt field is computed from something this run observed, or says that it was
 * not run; instructions belong here, not in the receipt. Manual host testing: start
 * `okf-qualify-mcp-apps --http 127.0.0.1:18765` (or set OKF_MCP_APPS_HTTP) with
 * OKF_MCP_APPS_NGROK=1, run `ngrok http 18765`, and give the host https://<ngrok-host>/mcp.
 */

import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
import { buildRelease } from '../lib/cargo.mjs';
import { BASIC_HOST, PATCHED_SERVE, SOURCE_RECORD, lsRemoteArgs, patchServe, rawUrl, sourceProblems, sourceRecord, tagCommit } from './lib/basic-host.mjs';
import { EXIT_REFUSED } from './lib/criteria.mjs';
import { DOM_SELECTORS, observeView, readAppDom } from './lib/observe.mjs';
import { killProcessTree, spawnGroup, waitForListening } from './lib/process.mjs';
import { observeProtocol } from './lib/protocol.mjs';
import { qualify } from './lib/qualify.mjs';
import { MCP_APPS_INPUTS, PRESENT_DATASET, VIEWS, chartForBinding, datasetExpectation } from './lib/views.mjs';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const PROTOCOL_ONLY =
  process.env.OKF_MCP_APPS_PROTOCOL_ONLY === '1' ||
  process.env.OKF_MCP_APPS_PROTOCOL_ONLY === 'true';
const uiDir = join(root, 'ui');
const distApps = join(uiDir, 'dist-apps');
const manifestPath = join(distApps, 'manifest.json');
const outDir = join(root, '.artifacts/qualification/mcp-apps');
const receiptPath = join(outDir, 'receipt.json');
const hostsDir = join(outDir, 'hosts');
const basicHostDir = join(outDir, 'basic-host');
const criteriaPath = fileURLToPath(new URL('./criteria.json', import.meta.url));
const HARNESS_PACKAGE = 'okf-qualify-mcp-apps';
const HTTP_PORT = Number(process.env.OKF_MCP_APPS_PORT ?? '18765');
const HTTP_BIND = `127.0.0.1:${HTTP_PORT}`;
const MCP_URL = `http://127.0.0.1:${HTTP_PORT}/mcp`;
const NGROK_ENABLED = process.env.OKF_MCP_APPS_NGROK === '1';

// The harness reads the bundle and the fixtures this run names, never ones an inherited
// OKF_MCP_APPS_DIST or OKF_MCP_APPS_FIXTURES points at: the receipt cites the repository's.
const fixturesDir = join(root, 'tests/fixtures/views');
const harnessEnv = { ...process.env, OKF_MCP_APPS_DIST: distApps, OKF_MCP_APPS_FIXTURES: fixturesDir };

function run(cmd, args, options = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(cmd, args, {
      cwd: options.cwd ?? root,
      env: options.env ?? process.env,
      stdio: options.stdio ?? 'inherit',
    });
    let stdout = '';
    let stderr = '';
    child.stdout?.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr?.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => resolveRun({ code: code ?? 1, stdout, stderr }));
  });
}

async function waitForHttp(url, attempts = 60) {
  for (let i = 0; i < attempts; i += 1) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 1_500);
    try {
      await fetch(url, { method: 'GET', signal: controller.signal });
      clearTimeout(timer);
      return;
    } catch {
      clearTimeout(timer);
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`timed out waiting for ${url}`);
}

/** Rebuild the UI and read back what the build left: the manifest and the bundle file. */
async function buildBundle() {
  const { RESOURCE_MIME_TYPE } = createRequire(join(uiDir, 'package.json'))('@modelcontextprotocol/ext-apps');
  const build = await run('bun', ['--bun', 'run', 'build'], { cwd: uiDir });
  if (build.code !== 0) throw new Error(`the UI build (bun --bun run build in ui/) exited ${build.code}`);
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  const manifest_resources = Array.isArray(manifest.resources) ? manifest.resources : [];
  const first = manifest_resources[0]?.name;
  return {
    manifest_resources,
    sdk_mime_type: RESOURCE_MIME_TYPE,
    // The build inherits this process's environment; the bundle itself says what came out.
    app_html: typeof first === 'string' ? await readFile(join(distApps, `${first}.html`), 'utf8') : null,
    node_env: process.env.NODE_ENV,
  };
}

/** Build once, then run the binary itself: its stderr and exit code are ours to read. */
async function buildHarness() {
  const path = await buildRelease(root, HARNESS_PACKAGE);
  const bytes = await readFile(path);
  return { path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), args: ['--http', HTTP_BIND] };
}

/** What the committed fixtures say the App must show, and what the product declares for read_object. */
async function expectations() {
  const bytes = await readFile(join(root, PRESENT_DATASET.fixture));
  const present = JSON.parse(await readFile(join(root, PRESENT_DATASET.present), 'utf8'));
  const catalog = JSON.parse(await readFile(join(root, 'api/mcp-tools.json'), 'utf8'));
  const product_read_object = (catalog.tools ?? []).find((tool) => tool.name === 'read_object');
  if (!product_read_object) throw new Error('api/mcp-tools.json declares no read_object');
  return {
    dataset: datasetExpectation({
      binding: PRESENT_DATASET.binding,
      digest: createHash('sha256').update(bytes).digest('hex'),
      rows: JSON.parse(bytes.toString('utf8')),
      bytes: bytes.length,
      chart: chartForBinding(present, PRESENT_DATASET.binding),
    }),
    product_read_object,
  };
}

async function checkHarness(harness) {
  const checkRun = await run(harness.path, ['--check'], { env: harnessEnv, stdio: ['ignore', 'pipe', 'pipe'] });
  const text = checkRun.stdout.trim();
  const start = text.indexOf('{');
  const end = text.lastIndexOf('}');
  let report = null;
  if (start >= 0 && end > start) {
    try {
      report = JSON.parse(text.slice(start, end + 1));
    } catch {
      report = null;
    }
  }
  return { code: checkRun.code, report, stderr: checkRun.stderr };
}

async function startHarness(harness) {
  const proc = spawnGroup(harness.path, harness.args, {
    env: {
      ...harnessEnv,
      // Explicit either way: an inherited value must not lift Host protection.
      OKF_MCP_APPS_NGROK: NGROK_ENABLED && !PROTOCOL_ONLY ? '1' : '0',
    },
  });
  try {
    const listening = await waitForListening(proc, { pattern: /okf-qualify-mcp-apps listening on (\S+)/, label: HARNESS_PACKAGE });
    return {
      endpoint: listening[1],
      toolLog: () => proc.stderr(),
      async stop() {
        const exited_before_teardown = proc.child.exitCode !== null;
        const exit = await killProcessTree(proc);
        return { exited_before_teardown, exit, stderr_tail: proc.stderr().slice(-2000) };
      },
    };
  } catch (error) {
    await killProcessTree(proc);
    throw error;
  }
}

/** Connect the MCP client the UI ships with and let lib/protocol.mjs ask its questions. */
async function protocolObservation() {
  const { Client, StreamableHTTPClientTransport } = await import(
    pathToFileURL(join(uiDir, 'node_modules/@modelcontextprotocol/client/dist/index.mjs')).href
  );
  const client = new Client({ name: 'okf-qualify-mcp-apps-protocol', version: '0.1.0' });
  await client.connect(new StreamableHTTPClientTransport(new URL(MCP_URL)));
  try {
    return await observeProtocol(client, { views: VIEWS });
  } finally {
    await client.close().catch(() => {});
  }
}

const sha256File = async (path) => createHash('sha256').update(await readFile(path)).digest('hex');

/**
 * The reference host's sources on disk, as lib/basic-host.mjs requires them: fetched at the
 * commit the tag named, recorded with their hashes, and still those bytes now.
 */
async function ensureBasicHost() {
  const recordPath = join(basicHostDir, SOURCE_RECORD);
  const hashesOnDisk = async () =>
    Object.fromEntries(
      await Promise.all(BASIC_HOST.files.map(async (file) => [file, await sha256File(join(basicHostDir, file)).catch(() => null)])),
    );
  let record = await readFile(recordPath, 'utf8').then(JSON.parse, () => null);
  let status = 'cached';
  if (record === null) {
    // Nothing recorded (first run, or a cache from before the record existed): fetch afresh.
    status = 'fetched';
    await rm(basicHostDir, { recursive: true, force: true });
    await mkdir(basicHostDir, { recursive: true });
    const listed = await run('git', lsRemoteArgs(), { stdio: ['ignore', 'pipe', 'pipe'] });
    if (listed.code !== 0) throw new Error(`git ls-remote ${BASIC_HOST.repository} exited ${listed.code}: ${listed.stderr.trim()}`);
    const commit = tagCommit(listed.stdout, BASIC_HOST.tag);
    const files = {};
    for (const file of BASIC_HOST.files) {
      const response = await fetch(rawUrl(commit, file));
      if (!response.ok) throw new Error(`fetch basic-host ${file} at ${commit}: HTTP ${response.status}`);
      const body = Buffer.from(await response.arrayBuffer());
      const target = join(basicHostDir, file);
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, body);
      files[file] = createHash('sha256').update(body).digest('hex');
    }
    record = { repository: BASIC_HOST.repository, tag: BASIC_HOST.tag, commit, fetched_at: new Date().toISOString(), files };
    await writeFile(recordPath, `${JSON.stringify(record, null, 2)}\n`);
  }
  const hashes = await hashesOnDisk();
  const problems = sourceProblems(record, hashes);
  if (problems.length) {
    throw new Error(`the basic-host sources in ${basicHostDir} are not the ones recorded as fetched (${problems.join('; ')}); delete that directory to fetch them again`);
  }
  const packageJson = JSON.parse(await readFile(join(basicHostDir, 'package.json'), 'utf8'));
  if (`v${packageJson.version}` !== BASIC_HOST.tag) {
    throw new Error(`basic-host at ${record.commit} is ${packageJson.name}@${packageJson.version}, not ${BASIC_HOST.tag}`);
  }
  // The fetched serve.ts stays as fetched; the patched copy is what runs.
  await writeFile(join(basicHostDir, PATCHED_SERVE), patchServe(await readFile(join(basicHostDir, 'serve.ts'), 'utf8')));
  const install = await run('bun', ['install'], { cwd: basicHostDir });
  if (install.code !== 0) throw new Error(`basic-host bun install exited ${install.code}`);
  return sourceRecord({
    status,
    record,
    hashes,
    packageJson,
    // The example has no lockfile upstream; this is the one bun install resolved here.
    lockfileSha256: await sha256File(join(basicHostDir, 'bun.lock')).catch(() => null),
  });
}

async function buildBasicHost() {
  // Avoid concurrently+cross-env PATH issues on Windows: two vite builds with env.
  for (const input of ['index.html', 'sandbox.html']) {
    const built = await run('bun', ['x', 'vite', 'build'], {
      cwd: basicHostDir,
      env: { ...process.env, INPUT: input, NODE_ENV: 'development' },
    });
    if (built.code !== 0) throw new Error(`basic-host vite build of ${input} exited ${built.code}`);
  }
}

/** Fetch (or reuse), build and serve the reference host, pointed at the harness server. */
async function startHost() {
  const source = await ensureBasicHost();
  await buildBasicHost();
  const host = spawnGroup('bun', [PATCHED_SERVE], {
    cwd: basicHostDir,
    env: {
      ...process.env,
      SERVERS: JSON.stringify([MCP_URL]),
      // Must match hardcoded SANDBOX_PROXY_BASE_URL in basic-host src/implementation.ts.
      HOST_PORT: '8080',
      SANDBOX_PORT: '8081',
    },
  });
  try {
    let serving = false;
    const exitedEarly = host.exited.then(({ code, error }) => {
      if (serving) return;
      throw new Error(
        `basic-host server exited before serving (exit ${code ?? 'none'}${error ? `, ${error}` : ''})\n${host.stderr().slice(-2000)}`,
      );
    });
    try {
      await Promise.race([waitForHttp('http://127.0.0.1:8080/api/servers'), exitedEarly]);
    } finally {
      serving = true; // from here an exit is teardown, not a rejection nobody awaits
    }
    return { source, stop: () => killProcessTree(host) };
  } catch (error) {
    await killProcessTree(host);
    throw error;
  }
}

/** The driver lib/observe.mjs watches a view through: Playwright calls and nothing else. */
function playwrightDriver(page, AxeBuilder, toolLog) {
  return {
    now: () => Date.now(),
    toolLog,
    open: (url) => page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 }),
    hostText: () => page.locator('body').innerText(),
    frames: () => page.frames(),
    readFrame: (frame) => frame.evaluate(readAppDom, DOM_SELECTORS).catch(() => null),
    wait: (ms) => page.waitForTimeout(ms),
    async screenshot(name) {
      const path = join(outDir, `basic-host-${name}.png`);
      await page.screenshot({ path, fullPage: true });
      return path;
    },
    axe: () => new AxeBuilder({ page }).analyze(),
  };
}

async function launchBrowser(server) {
  const { chromium } = await import(pathToFileURL(join(uiDir, 'node_modules/@playwright/test/index.mjs')).href);
  const AxeBuilder = (await import(pathToFileURL(join(uiDir, 'node_modules/@axe-core/playwright/dist/index.js')).href)).default;
  await mkdir(outDir, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  return {
    version: browser.version(),
    async render(view, expected) {
      const context = await browser.newContext();
      try {
        return await observeView(playwrightDriver(await context.newPage(), AxeBuilder, server.toolLog), view, expected);
      } finally {
        await context.close().catch(() => {});
      }
    },
    close: () => browser.close().catch(() => {}),
  };
}

/** Start ngrok and wait for its public URL; a tunnel that will not open is a note, not a failure. */
async function openTunnel() {
  const opened_at = new Date().toISOString();
  const proc = spawnGroup('ngrok', ['http', String(HTTP_PORT), '--log=stdout']);
  let gone = false;
  proc.exited.then(() => {
    gone = true;
  });
  const deadline = Date.now() + 30_000;
  let publicUrl = null;
  while (!gone && Date.now() < deadline && !publicUrl) {
    try {
      const response = await fetch('http://127.0.0.1:4040/api/tunnels');
      if (response.ok) {
        const payload = await response.json();
        const entry = (payload.tunnels ?? []).find((item) => item.public_url?.startsWith('https://'));
        if (entry) publicUrl = entry.public_url;
      }
    } catch {
      // retry
    }
    if (!publicUrl) await new Promise((r) => setTimeout(r, 500));
  }
  return {
    opened_at,
    public_url: publicUrl ? `${publicUrl}/mcp` : null,
    note: publicUrl
      ? 'Short-lived public URL for manual claude.ai / ChatGPT connector testing; closed before this receipt was written. Not Cloudflare.'
      : `ngrok started but no public https URL appeared on 127.0.0.1:4040; stderr: ${proc.stderr().slice(0, 1000)}`,
    async close() {
      await killProcessTree(proc);
      return new Date().toISOString();
    },
  };
}

async function hostRenderEvidence() {
  let entries = [];
  try {
    entries = await readdir(hostsDir);
  } catch {
    return {
      status: 'not_run',
      meaning:
        'No files under .artifacts/qualification/mcp-apps/hosts/. Host render for claude.ai / ChatGPT remains not_run until screenshots or notes are placed there. This receipt does not invent PASS.',
      gate: 'mcp-apps-web-hosts',
    };
  }
  const files = entries.filter((name) => !name.startsWith('.'));
  if (files.length === 0) {
    return {
      status: 'not_run',
      meaning:
        'hosts/ directory exists but is empty. Place screenshots or notes from claude.ai / ChatGPT before claiming an observed host render.',
      gate: 'mcp-apps-web-hosts',
    };
  }
  return {
    status: 'evidence_present',
    gate: 'mcp-apps-web-hosts',
    files: files.map((name) => join(hostsDir, name)),
    meaning:
      'Evidence files were found under hosts/. Status is not PASS; an integrator must label each file as observed render, text-only fallback, or rejection by host and date. Harness screenshots are not host renders.',
  };
}

const effects = {
  removeReceipt: () => rm(receiptPath, { force: true }),
  async header() {
    const header = await receiptHeader(root, MCP_APPS_INPUTS);
    return header;
  },
  buildBundle,
  buildHarness,
  expectations,
  checkHarness,
  startHarness,
  observeProtocol: protocolObservation,
  openTunnel,
  startHost,
  launchBrowser,
  hostRenderEvidence,
  async writeReceipt(receipt) {
    await mkdir(hostsDir, { recursive: true });
    await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
    return receiptPath;
  },
  recordReceipt: (receipt) => recordReceipt(root, 'mcp-apps', receipt),
};

const record = process.argv.includes('--record');
try {
  const pinned = JSON.parse(await readFile(criteriaPath, 'utf8')).required;
  const { receipt, exitCode, recorded } = await qualify(effects, {
    record,
    protocolOnly: PROTOCOL_ONLY,
    pinned,
    config: { harness: HARNESS_PACKAGE, mcp_url: MCP_URL, http_port: HTTP_PORT, ngrok: NGROK_ENABLED },
  });
  process.stdout.write(`MCP Apps qualification receipt: ${receiptPath}\n`);
  if (recorded?.path) process.stdout.write(`MCP Apps receipt recorded: ${recorded.path}\n`);
  if (recorded?.refused) process.stdout.write(`MCP Apps receipt ${recorded.refused}\n`);
  for (const criterion of receipt.criteria.filter((item) => item.result !== 'pass')) {
    process.stdout.write(`${criterion.result} ${criterion.id}: ${criterion.detail ?? ''}\n`);
  }
  if (receipt.harness_error) process.stdout.write(`harness_error: ${receipt.harness_error}\n`);
  process.stdout.write(`MCP Apps qualification ${receipt.result} (${receipt.criteria.length} criteria, ${receipt.not_judged.length} not judged)\n`);
  process.exitCode = exitCode;
} catch (error) {
  // No receipt was written (for example a dirty tree): the run was refused, not judged.
  process.stderr.write(`MCP Apps qualification wrote no receipt: ${error?.stack ?? error}\n`);
  process.exitCode = EXIT_REFUSED;
}
