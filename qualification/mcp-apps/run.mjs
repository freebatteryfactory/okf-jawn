/**
 * Orchestrate Phase 0 MCP Apps protocol qualification.
 *
 * Steps: rebuild dist-apps, build the harness binary, harness --check, Streamable HTTP
 * protocol check over all four render tools and the app-only show and read_object tools,
 * then (unless OKF_MCP_APPS_PROTOCOL_ONLY=1) the official basic-host example rendering each
 * of the four views under Playwright with axe, and an optional ngrok URL
 * (OKF_MCP_APPS_NGROK=1) that is always closed before the receipt is written.
 *
 * The present view must really render its retained dataset: the App fetches it through
 * read_object in ranged blocks, and basic_host.present_dataset is computed from the chart
 * svg, the table rows and the harness's tool-call reports seen while that view rendered.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/mcp-apps/run.mjs [--record]
 * Exits non-zero when the protocol check or any view fails; the receipt is written first.
 * host_render reports only evidence found under .artifacts/.../hosts/ and never a PASS.
 *
 * Every receipt field is computed from something this run observed, or says that it was
 * not run; instructions belong here, not in the receipt. Manual host testing: start
 * `okf-qualify-mcp-apps --http 127.0.0.1:18765` (or set OKF_MCP_APPS_HTTP) with
 * OKF_MCP_APPS_NGROK=1, run `ngrok http 18765`, and give the host https://<ngrok-host>/mcp.
 */

import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
import { buildRelease } from '../lib/cargo.mjs';
import { killProcessTree, spawnGroup, waitForListening } from './lib/process.mjs';
import {
  APP_ONLY_TOOLS,
  APP_RESOURCE_URI,
  UPSTREAM_HOST_RULES,
  VIEWS,
  basicHostUrl,
  basicHostVerdict,
  datasetExpectation,
  judgePresentDataset,
  judgeView,
  ngrokRecord,
  partitionAxe,
  runProblems,
  toolCallsFrom,
  transportRecord,
} from './lib/views.mjs';

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
const HARNESS_PACKAGE = 'okf-qualify-mcp-apps';
/** The ext-apps release whose examples/basic-host is the reference host. */
const BASIC_HOST_TAG = 'v2.0.3';
const HTTP_PORT = Number(process.env.OKF_MCP_APPS_PORT ?? '18765');
const HTTP_BIND = `127.0.0.1:${HTTP_PORT}`;
const MCP_URL = `http://127.0.0.1:${HTTP_PORT}/mcp`;
const NGROK_ENABLED = process.env.OKF_MCP_APPS_NGROK === '1';
const MCP_APPS_INPUTS = [
  'qualification/mcp-apps',
  'qualification/lib',
  'tests/fixtures/views',
  'ui/src/mcp-apps',
  'ui/scripts/bundle-app.mjs',
  'Cargo.toml',
  'Cargo.lock',
];

const require = createRequire(join(uiDir, 'package.json'));
const { RESOURCE_MIME_TYPE } = require('@modelcontextprotocol/ext-apps');

function run(cmd, args, options = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(cmd, args, {
      cwd: options.cwd ?? root,
      env: options.env ?? process.env,
      stdio: options.stdio ?? 'inherit',
      shell: options.shell ?? false,
    });
    let stdout = '';
    let stderr = '';
    if (child.stdout) {
      child.stdout.on('data', (chunk) => {
        stdout += chunk;
        if (options.echo !== false) process.stdout.write(chunk);
      });
    }
    if (child.stderr) {
      child.stderr.on('data', (chunk) => {
        stderr += chunk;
        if (options.echo !== false) process.stderr.write(chunk);
      });
    }
    child.on('error', reject);
    child.on('close', (code) => {
      resolveRun({ code: code ?? 1, stdout, stderr, child });
    });
    if (options.retainChild) {
      resolveRun({ code: 0, stdout, stderr, child, pending: true });
    }
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

async function protocolCheck(mcpUrl, manifest) {
  const clientMod = await import(
    pathToFileURL(join(uiDir, 'node_modules/@modelcontextprotocol/client/dist/index.mjs')).href
  );
  const { Client, StreamableHTTPClientTransport } = clientMod;
  const client = new Client({ name: 'okf-qualify-mcp-apps-protocol', version: '0.1.0' });
  const transport = new StreamableHTTPClientTransport(new URL(mcpUrl));
  await client.connect(transport);
  try {
    const tools = await client.listTools();
    const listed = Array.isArray(tools.tools) ? tools.tools : [];
    const names = (items) => JSON.stringify(items.map((tool) => tool.name).sort());
    const renderTools = listed.filter((tool) => typeof tool._meta?.ui?.resourceUri === 'string');
    const appOnly = listed.filter(
      (tool) => JSON.stringify(tool._meta?.ui?.visibility ?? null) === '["app"]',
    );
    const expectedRender = JSON.stringify(VIEWS.map((view) => view.tool).sort());
    if (names(renderTools) !== expectedRender) {
      throw new Error(`tools/list render tools ${names(renderTools)} !== ${expectedRender}`);
    }
    for (const tool of renderTools) {
      if (tool._meta.ui.resourceUri !== APP_RESOURCE_URI) {
        throw new Error(`tool ${tool.name} resourceUri ${tool._meta.ui.resourceUri} !== ${APP_RESOURCE_URI}`);
      }
    }
    if (names(appOnly) !== JSON.stringify([...APP_ONLY_TOOLS].sort())) {
      throw new Error(`tools/list app-only tools ${names(appOnly)} !== ${JSON.stringify(APP_ONLY_TOOLS)}`);
    }
    if (listed.length !== renderTools.length + appOnly.length) {
      throw new Error(`tools/list has tools that are neither render tools nor app-only: ${names(listed)}`);
    }

    const resources = await client.listResources();
    if (!Array.isArray(resources.resources) || resources.resources.length !== 1) {
      throw new Error(
        `resources/list expected 1 shared App resource, got ${resources.resources?.length}`,
      );
    }
    if (resources.resources[0]?.uri !== APP_RESOURCE_URI) {
      throw new Error(
        `resources/list uri must be ${APP_RESOURCE_URI}, got ${resources.resources[0]?.uri}`,
      );
    }
    const reads = [];
    for (const resource of resources.resources) {
      const read = await client.readResource({ uri: resource.uri });
      const content = read.contents?.[0];
      if (!content || typeof content.text !== 'string') {
        throw new Error(`resources/read ${resource.uri} missing text`);
      }
      if (content.mimeType !== RESOURCE_MIME_TYPE) {
        throw new Error(
          `resources/read ${resource.uri} mimeType ${content.mimeType} !== ${RESOURCE_MIME_TYPE}`,
        );
      }
      const csp = content._meta?.ui?.csp;
      if (!csp || typeof csp !== 'object' || Array.isArray(csp)) {
        throw new Error(`resources/read ${resource.uri} _meta.ui.csp must be an object`);
      }
      if (!Array.isArray(csp.connectDomains) || !Array.isArray(csp.resourceDomains)) {
        throw new Error(
          `resources/read ${resource.uri} _meta.ui.csp needs connectDomains and resourceDomains arrays`,
        );
      }
      const hash = createHash('sha256').update(content.text, 'utf8').digest('hex');
      const expected = manifest.resources.find((entry) => entry.uri === resource.uri);
      if (!expected) throw new Error(`manifest missing ${resource.uri}`);
      if (hash !== expected.sha256) {
        throw new Error(
          `resources/read ${resource.uri} sha256 ${hash} !== manifest ${expected.sha256}`,
        );
      }
      reads.push({
        uri: resource.uri,
        mimeType: content.mimeType,
        sha256: hash,
        csp,
      });
    }

    // Every render tool: structuredContent for the App, text for hosts without Apps.
    const toolCalls = [];
    let present = null;
    for (const view of VIEWS) {
      const call = await client.callTool({ name: view.tool, arguments: {} });
      if (!call.structuredContent) throw new Error(`${view.tool} missing structuredContent`);
      const textBlock = (call.content ?? []).find((block) => block.type === 'text');
      if (!textBlock || typeof textBlock.text !== 'string' || textBlock.text.length === 0) {
        throw new Error(`${view.tool} missing text fallback`);
      }
      if (view.tool === 'render_present') present = call.structuredContent;
      toolCalls.push({
        name: view.tool,
        has_structured_content: call.structuredContent !== null && typeof call.structuredContent === 'object',
        text_fallback: textBlock.text,
      });
    }

    // The present view resolves each binding through the app-only show tool
    // (arguments as ui/src/features/views/PresentView.tsx sends them).
    const showCalls = [];
    for (const binding of present?.resolved_bindings ?? []) {
      const shown = await client.callTool({
        name: 'show',
        arguments: {
          workspace_id: binding.source.workspace_id,
          item_id: binding.source.item_id,
          at: { kind: 'revision', revision: binding.source.revision },
          view: 'text',
          selection: binding.source.selection,
          max_bytes: 65536,
          max_images: 0,
        },
      });
      const source = shown.structuredContent?.source;
      if (source?.item_id !== binding.source.item_id || source?.revision !== binding.source.revision) {
        throw new Error(`show did not return the bound source for binding ${binding.name}`);
      }
      showCalls.push({ binding: binding.name, item_id: source.item_id, revision: source.revision });
    }
    if (showCalls.length === 0) throw new Error('render_present has no resolved_bindings to show');

    // The present view fetches each retained dataset through the app-only read_object tool
    // (arguments and loop as PresentView.tsx; the harness serves small blocks on purpose).
    const objectReads = [];
    for (const binding of present.resolved_bindings) {
      if (!binding.materialized) continue;
      const chunks = [];
      let offset = 0;
      let calls = 0;
      let part = null;
      do {
        if (calls >= 1000) throw new Error(`read_object for binding ${binding.name} did not finish in 1000 calls`);
        const read = await client.callTool({
          name: 'read_object',
          arguments: { source: binding.source, object: binding.materialized, offset: String(offset), length: 1048576 },
        });
        calls += 1;
        part = read.structuredContent;
        if (read.isError || !part) {
          throw new Error(`read_object failed for binding ${binding.name}: ${JSON.stringify(read.content)}`);
        }
        const textBlock = (read.content ?? []).find((block) => block.type === 'text');
        if (!textBlock?.text) throw new Error('read_object missing text fallback');
        if (part.sha256 !== binding.materialized || part.offset !== String(offset)) {
          throw new Error(`read_object changed identity or range for binding ${binding.name}: ${JSON.stringify(part)}`);
        }
        const bytes = Buffer.from(part.data_base64, 'base64');
        if (bytes.length === 0 && part.has_more) throw new Error('read_object made no progress');
        chunks.push(bytes);
        offset += bytes.length;
      } while (part.has_more === true);
      const merged = Buffer.concat(chunks);
      const sha256 = createHash('sha256').update(merged).digest('hex');
      if (sha256 !== binding.materialized) {
        throw new Error(`read_object bytes for binding ${binding.name} hash to ${sha256}, not ${binding.materialized}`);
      }
      if (part.total_size !== String(merged.length)) {
        throw new Error(`read_object total_size ${part.total_size} !== ${merged.length} bytes read`);
      }
      if (calls < 2) throw new Error('read_object returned the dataset in one block; the ranged loop was not exercised');
      objectReads.push({
        binding: binding.name,
        object: binding.materialized,
        calls,
        bytes: merged.length,
        sha256,
        media_type: part.media_type,
      });
    }
    if (objectReads.length === 0) throw new Error('render_present has no materialized binding to read');
    // A digest nobody retains must come back as a tool error, never as bytes.
    const unknown = await client.callTool({
      name: 'read_object',
      arguments: { source: present.resolved_bindings[0].source, object: '0'.repeat(64) },
    });
    if (unknown.isError !== true || unknown.structuredContent) {
      throw new Error(`read_object served an unknown digest: ${JSON.stringify(unknown)}`);
    }

    return {
      status: 'passed',
      server: client.getServerVersion() ?? null,
      tools: renderTools.map((tool) => ({
        name: tool.name,
        resourceUri: tool._meta.ui.resourceUri,
      })),
      app_only_tools: appOnly.map((tool) => tool.name),
      resources: reads,
      tool_calls: toolCalls,
      show_calls: showCalls,
      object_reads: objectReads,
      unknown_object_refused: unknown.isError === true,
    };
  } finally {
    await client.close().catch(() => {});
  }
}

async function ensureBasicHost() {
  // Official basic-host is not shipped inside the npm package; fetch the tagged example.
  const marker = join(basicHostDir, 'package.json');
  let source = 'cached';
  try {
    await readFile(marker, 'utf8');
  } catch {
    source = 'fetched';
    await mkdir(basicHostDir, { recursive: true });
    const files = [
      'package.json',
      'index.html',
      'sandbox.html',
      'serve.ts',
      'tsconfig.json',
      'vite.config.ts',
      'src/index.tsx',
      'src/implementation.ts',
      'src/sandbox.ts',
      'src/theme.ts',
      'src/index.module.css',
      'src/global.css',
      'src/host-styles.ts',
      'src/vite-env.d.ts',
    ];
    const base = `https://raw.githubusercontent.com/modelcontextprotocol/ext-apps/${BASIC_HOST_TAG}/examples/basic-host`;
    for (const file of files) {
      const response = await fetch(`${base}/${file}`);
      if (!response.ok) {
        throw new Error(`fetch basic-host ${file}: HTTP ${response.status}`);
      }
      const body = Buffer.from(await response.arrayBuffer());
      const target = join(basicHostDir, file);
      await mkdir(join(target, '..'), { recursive: true });
      await writeFile(target, body);
    }
  }
  // Express sendFile on this Windows/bun combo 404s with an absolute join path;
  // use { root } so sandbox.html is served from dist/.
  const servePath = join(basicHostDir, 'serve.ts');
  let serveSource = await readFile(servePath, 'utf8');
  if (serveSource.includes('res.sendFile(join(DIRECTORY, "sandbox.html"))')) {
    serveSource = serveSource.replace(
      'res.sendFile(join(DIRECTORY, "sandbox.html"));',
      'res.sendFile("sandbox.html", { root: DIRECTORY });',
    );
    await writeFile(servePath, serveSource);
  }
  const install = await run('bun', ['install'], { cwd: basicHostDir });
  if (install.code !== 0) {
    throw new Error(`basic-host bun install exited ${install.code}`);
  }
  // Upstream example relies on monorepo-hoisted cross-env / @types/cors.
  const extras = await run('bun', ['add', '-d', '@types/cors', 'cross-env'], { cwd: basicHostDir });
  if (extras.code !== 0) {
    throw new Error(`basic-host bun add extras exited ${extras.code}`);
  }
  // What is on disk decides which host this was, whether it was fetched now or earlier.
  const manifest = JSON.parse(await readFile(marker, 'utf8'));
  if (`v${manifest.version}` !== BASIC_HOST_TAG) {
    throw new Error(`basic-host in ${basicHostDir} is ${manifest.name}@${manifest.version}, not ${BASIC_HOST_TAG}`);
  }
  return { source, path: basicHostDir, package: { name: manifest.name, version: manifest.version } };
}

async function buildBasicHost() {
  // Avoid concurrently+cross-env PATH issues on Windows: two vite builds with env.
  const indexBuild = await run('bun', ['x', 'vite', 'build'], {
    cwd: basicHostDir,
    env: { ...process.env, INPUT: 'index.html', NODE_ENV: 'development' },
  });
  if (indexBuild.code !== 0) {
    throw new Error(`basic-host vite index build exited ${indexBuild.code}\n${indexBuild.stderr}`);
  }
  const sandboxBuild = await run('bun', ['x', 'vite', 'build'], {
    cwd: basicHostDir,
    env: { ...process.env, INPUT: 'sandbox.html', NODE_ENV: 'development' },
  });
  if (sandboxBuild.code !== 0) {
    throw new Error(`basic-host vite sandbox build exited ${sandboxBuild.code}\n${sandboxBuild.stderr}`);
  }
}

function frameDepth(frame) {
  let depth = 0;
  for (let parent = frame.parentFrame(); parent; parent = parent.parentFrame()) depth += 1;
  return depth;
}

/** The App document: host page (0) -> sandbox proxy on :8081 (1) -> App (2). */
function appFrameOf(page) {
  return (
    page
      .frames()
      .map((frame) => ({ frame, depth: frameDepth(frame) }))
      .filter((item) => item.depth >= 2)
      .sort((a, b) => b.depth - a.depth)[0] ?? null
  );
}

async function readFrame(frame) {
  try {
    const text = await frame.locator('body').innerText({ timeout: 1_000 });
    const alerts = await frame.locator('[role="alert"]').allInnerTexts();
    // Vega's SVG renderer draws the spec's own marks as children of g.role-mark;
    // a table inside closed <details> has no client rects.
    const dom = await frame.evaluate(() => ({
      svgs: document.querySelectorAll('svg').length,
      svg_marks: document.querySelectorAll('svg g[class~="role-mark"] > *').length,
      tables: Array.from(document.querySelectorAll('table'), (table) => ({
        caption: table.caption?.textContent ?? '',
        visible: table.getClientRects().length > 0,
        in_details: table.closest('details') !== null,
        columns: Array.from(table.querySelectorAll('thead th'), (cell) => cell.textContent ?? ''),
        rows: Array.from(table.tBodies[0]?.rows ?? [], (row) =>
          Array.from(row.cells, (cell) => cell.textContent ?? ''),
        ),
      })),
    }));
    return { text, alerts, ...dom };
  } catch {
    return null;
  }
}

/** The dataset fixture as the App must show it: digest of its exact bytes and its rows. */
async function loadDatasetExpectation(dataset) {
  const bytes = await readFile(join(root, dataset.fixture));
  return datasetExpectation({
    binding: dataset.binding,
    digest: createHash('sha256').update(bytes).digest('hex'),
    rows: JSON.parse(bytes.toString('utf8')),
  });
}

async function renderView(browser, AxeBuilder, view, toolLog) {
  const url = basicHostUrl(view.tool);
  const screenshot = join(outDir, `basic-host-${view.tool}.png`);
  const expected = view.dataset ? await loadDatasetExpectation(view.dataset) : null;
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    // Tool calls reported from here on belong to this view's render.
    const logStart = toolLog().length;
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 });

    let located = null;
    let observed = null;
    let verdict = judgeView(view, null);
    let dataset = expected ? judgePresentDataset(expected, null) : null;
    let rendered = false;
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline && !rendered) {
      const hostText = await page.locator('body').innerText();
      if (/Failed to connect to any servers/i.test(hostText)) {
        throw new Error(`basic-host could not reach the harness: ${hostText.slice(0, 200)}`);
      }
      located = appFrameOf(page);
      observed = located ? await readFrame(located.frame) : null;
      verdict = judgeView(view, observed);
      if (expected) {
        dataset = judgePresentDataset(
          expected,
          observed && { ...observed, tool_calls: toolCallsFrom(toolLog().slice(logStart)) },
        );
      }
      rendered = verdict.ok && (dataset === null || dataset.ok);
      if (!rendered) await page.waitForTimeout(500);
    }
    await page.screenshot({ path: screenshot, fullPage: true });

    const base = {
      tool: view.tool,
      view: view.view,
      url,
      screenshot,
      app_frame_depth: located?.depth ?? null,
      feature_text: (observed?.text ?? '').slice(0, 400),
      alerts: verdict.alerts,
      expected_alerts: verdict.expected_alerts,
      ...(dataset ? { present_dataset: dataset.record } : {}),
    };
    if (!verdict.ok) {
      return {
        ...base,
        status: 'failed',
        error: `App frame did not show the ${view.view} view: missing=${JSON.stringify(verdict.missing)} foreign=${JSON.stringify(verdict.foreign)} blocking=${JSON.stringify(verdict.blocking)} alerts=${JSON.stringify(verdict.alerts)}`,
      };
    }

    if (dataset && !dataset.ok) {
      return {
        ...base,
        status: 'failed',
        error: `App frame did not render the retained dataset: ${dataset.problems.join('; ')}`,
      };
    }

    // Every rule runs on the whole page; partitionAxe tolerates the two upstream rules
    // on host chrome only and requires proof that axe reached the App frame.
    const axe = partitionAxe(await new AxeBuilder({ page }).analyze(), located.depth);
    const axeProblems = [];
    if (!axe.app_frame_analysed) axeProblems.push('axe did not analyse the App frame');
    if (axe.app_frame.length) {
      axeProblems.push(`App frame serious/critical: ${JSON.stringify(axe.app_frame)}`);
    }
    if (axe.host_blocking.length) {
      axeProblems.push(
        `host chrome serious/critical outside the tolerated upstream rules: ${JSON.stringify(axe.host_blocking)}`,
      );
    }
    return axeProblems.length
      ? { ...base, axe, status: 'failed', error: axeProblems.join('; ') }
      : { ...base, axe, status: 'passed' };
  } finally {
    await context.close().catch(() => {});
  }
}

async function runBasicHostCheck(mcpUrl, toolLog) {
  // Package does not ship basic-host HTML; use the tagged GitHub example.
  const ensured = await ensureBasicHost();
  await buildBasicHost();
  const host = spawnGroup('bun', ['serve.ts'], {
    cwd: basicHostDir,
    env: {
      ...process.env,
      SERVERS: JSON.stringify([mcpUrl]),
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

    const { chromium } = await import(
      pathToFileURL(join(uiDir, 'node_modules/@playwright/test/index.mjs')).href
    );
    const AxeBuilder = (
      await import(pathToFileURL(join(uiDir, 'node_modules/@axe-core/playwright/dist/index.js')).href)
    ).default;

    const browser = await chromium.launch({ headless: true });
    const views = [];
    try {
      for (const view of VIEWS) {
        try {
          views.push(await renderView(browser, AxeBuilder, view, toolLog));
        } catch (error) {
          views.push({
            tool: view.tool,
            view: view.view,
            status: 'failed',
            error: String(error.message || error),
          });
        }
      }
    } finally {
      await browser.close().catch(() => {});
    }
    return {
      ...basicHostVerdict(views),
      source: ensured.source,
      host_package: ensured.package,
      views,
      axe_tolerated_upstream_host_rules: UPSTREAM_HOST_RULES,
      present_dataset: views.find((item) => item.present_dataset)?.present_dataset ?? {
        status: 'not_observed',
        reason: 'no view reached the point where its dataset rendering is observed',
      },
      note: 'The ext-apps basic-host example is a reference host; this is not a claude.ai or ChatGPT claim.',
    };
  } finally {
    await killProcessTree(host);
  }
}

async function openNgrok(port) {
  const opened_at = new Date().toISOString();
  const proc = spawnGroup('ngrok', ['http', String(port), '--log=stdout']);
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
    proc,
    opened_at,
    public_url: publicUrl ? `${publicUrl}/mcp` : null,
    note: publicUrl
      ? 'Short-lived public URL for manual claude.ai / ChatGPT connector testing; closed before this receipt was written. Not Cloudflare.'
      : `ngrok started but no public https URL appeared on 127.0.0.1:4040; stderr: ${proc.stderr().slice(0, 1000)}`,
  };
}

async function hostRenderFromEvidence() {
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

const record = process.argv.includes('--record');
if (record && PROTOCOL_ONLY) {
  throw new Error('--record refused: a protocol-only run is not the gate receipt');
}
const header = await receiptHeader(root, MCP_APPS_INPUTS);

await mkdir(outDir, { recursive: true });
await mkdir(hostsDir, { recursive: true });

const build = await run('bun', ['--bun', 'run', 'build'], { cwd: uiDir });
if (build.code !== 0) {
  throw new Error(`ui build exited ${build.code}`);
}

const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
const resources = Array.isArray(manifest.resources) ? manifest.resources : [];
if (resources.length !== 1) {
  throw new Error(`manifest must list 1 shared App resource, found ${resources.length}`);
}
if (resources[0]?.uri !== APP_RESOURCE_URI) {
  throw new Error(`manifest uri must be ${APP_RESOURCE_URI}, got ${resources[0]?.uri}`);
}

const mimeMismatches = resources.filter((resource) => resource.mimeType !== RESOURCE_MIME_TYPE);
const mimeCheck = {
  sdk_constant: RESOURCE_MIME_TYPE,
  manifest_mime_types: resources.map((resource) => ({
    name: resource.name,
    mimeType: resource.mimeType,
  })),
  equal: mimeMismatches.length === 0,
};
if (!mimeCheck.equal) {
  throw new Error(
    `manifest mimeType must equal SDK RESOURCE_MIME_TYPE (${RESOURCE_MIME_TYPE}); mismatches: ${JSON.stringify(mimeMismatches)}`,
  );
}

// Build once, then run the binary itself: its stderr and exit code are ours to read,
// and the process group holds the harness, not `cargo run`.
const harnessBin = await buildRelease(root, HARNESS_PACKAGE);
const harnessBytes = await readFile(harnessBin);
const harnessArgs = ['--http', HTTP_BIND];

const checkRun = await run(harnessBin, ['--check'], {
  env: { ...process.env, OKF_MCP_APPS_DIST: distApps },
  stdio: ['ignore', 'pipe', 'pipe'],
});
if (checkRun.code !== 0) {
  throw new Error(`okf-qualify-mcp-apps --check exited ${checkRun.code}\n${checkRun.stderr}`);
}

let check;
try {
  const trimmed = checkRun.stdout.trim();
  const start = trimmed.indexOf('{');
  const end = trimmed.lastIndexOf('}');
  if (start < 0 || end < start) throw new Error('no JSON object in stdout');
  check = JSON.parse(trimmed.slice(start, end + 1));
} catch (error) {
  throw new Error(`could not parse harness check JSON: ${error.message}\n${checkRun.stdout}`);
}

const readable = Object.fromEntries(
  (check.resources ?? []).map((resource) => [resource.name, Boolean(resource.readable)]),
);
if (!readable.app) {
  throw new Error('shared App resource was not reported readable by the harness');
}

const harness = spawnGroup(harnessBin, harnessArgs, {
  env: {
    ...process.env,
    OKF_MCP_APPS_DIST: distApps,
    // Explicit either way: an inherited value must not lift Host protection.
    OKF_MCP_APPS_NGROK: NGROK_ENABLED && !PROTOCOL_ONLY ? '1' : '0',
  },
});

let protocol = { status: 'not_run' };
let basicHost = { status: 'not_run', reason: PROTOCOL_ONLY ? 'skipped: protocol-only' : undefined };
let tunnel = null;
let fatal = null;
let harnessExit = null;
let harnessDiedEarly = false;
let listeningOn = null;

try {
  const listening = await waitForListening(harness, {
    pattern: /okf-qualify-mcp-apps listening on (\S+)/,
    label: HARNESS_PACKAGE,
  });
  listeningOn = listening[1];
  try {
    protocol = await protocolCheck(MCP_URL, manifest);
  } catch (error) {
    protocol = { status: 'failed', error: String(error.message || error) };
  }

  if (!PROTOCOL_ONLY) {
    if (NGROK_ENABLED) {
      try {
        tunnel = await openNgrok(HTTP_PORT);
      } catch (error) {
        tunnel = { proc: null, opened_at: null, public_url: null, note: String(error.message || error) };
      }
    }
    try {
      basicHost = await runBasicHostCheck(MCP_URL, () => harness.stderr());
    } catch (error) {
      basicHost = {
        status: 'failed',
        error: String(error.message || error),
        note: `@modelcontextprotocol/ext-apps npm package does not ship basic-host; attempted the official ${BASIC_HOST_TAG} GitHub example.`,
      };
    }
  }
} catch (error) {
  fatal = error;
  protocol = { status: 'failed', error: String(error.message || error) };
} finally {
  if (tunnel) {
    if (tunnel.proc) await killProcessTree(tunnel.proc);
    tunnel.closed_at = new Date().toISOString();
  }
  harnessDiedEarly = harness.child.exitCode !== null;
  harnessExit = await killProcessTree(harness);
}

const ngrok = ngrokRecord({
  enabled: NGROK_ENABLED && !PROTOCOL_ONLY,
  opened_at: tunnel?.opened_at ?? null,
  closed_at: tunnel?.closed_at ?? null,
  public_url: tunnel?.public_url ?? null,
  local_port: HTTP_PORT,
  note: tunnel?.note ?? (PROTOCOL_ONLY ? 'skipped: protocol-only' : 'OKF_MCP_APPS_NGROK is not 1'),
});

const host_render = PROTOCOL_ONLY
  ? { status: 'not_run', reason: 'skipped: protocol-only' }
  : await hostRenderFromEvidence();

const problems = runProblems({ protocol, basicHost, protocolOnly: PROTOCOL_ONLY });
if (fatal) problems.unshift(`harness: ${String(fatal.message || fatal)}`);

const receipt = {
  ...header,
  component: 'mcp-apps-protocol-qualification',
  gate: 'mcp-apps-protocol-qualification',
  finished_at: new Date().toISOString(),
  result: problems.length === 0 ? 'PASS' : 'FAIL',
  problems,
  harness: HARNESS_PACKAGE,
  protocol_only: PROTOCOL_ONLY,
  transport: transportRecord({ requested: MCP_URL, reported: listeningOn }),
  harness_binary: {
    path: harnessBin,
    bytes: harnessBytes.length,
    sha256: createHash('sha256').update(harnessBytes).digest('hex'),
    args: harnessArgs,
  },
  harness_process: {
    exited_before_teardown: harnessDiedEarly,
    exit: harnessExit,
    stderr_tail: harness.stderr().slice(-2000),
  },
  mime_check: mimeCheck,
  bundle_sizes: Object.fromEntries(
    resources.map((resource) => [resource.name, resource.byteLength]),
  ),
  resources_readable: readable,
  check,
  protocol_check: protocol,
  basic_host: basicHost,
  ngrok,
  host_render,
  static_bundle_smoke: {
    status: 'not_run',
    reason:
      'ui/tests/e2e/mcp-apps-static-bundle-smoke.spec.ts is a separate smoke test; this orchestrator does not run it and it is not a host-render check',
  },
  note: 'host_render (claude.ai / ChatGPT) belongs to the acceptance gate mcp-apps-web-hosts and is never invented here.',
};

await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`MCP Apps qualification receipt: ${receiptPath}\n`);
if (record) {
  process.stdout.write(`MCP Apps receipt recorded: ${await recordReceipt(root, 'mcp-apps', receipt)}\n`);
}

if (problems.length) {
  throw new Error(`MCP Apps qualification FAIL:\n${problems.join('\n')}`);
}
