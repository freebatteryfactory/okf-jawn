/**
 * Orchestrate Phase 0 MCP Apps / web-hosts qualification.
 *
 * Steps: rebuild dist-apps, harness --check, Streamable HTTP protocol check,
 * optional official basic-host Playwright/axe, optional ngrok URL recording,
 * honest host_render from `.artifacts/.../hosts/` evidence only.
 */

import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { pathToFileURL } from 'node:url';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const uiDir = join(root, 'ui');
const distApps = join(uiDir, 'dist-apps');
const manifestPath = join(distApps, 'manifest.json');
const outDir = join(root, '.artifacts/qualification/mcp-apps');
const receiptPath = join(outDir, 'receipt.json');
const hostsDir = join(outDir, 'hosts');
const basicHostDir = join(outDir, 'basic-host');
const HTTP_PORT = Number(process.env.OKF_MCP_APPS_PORT ?? '18765');
const HTTP_BIND = `127.0.0.1:${HTTP_PORT}`;
const MCP_URL = `http://127.0.0.1:${HTTP_PORT}/mcp`;
const NGROK_ENABLED =
  process.env.OKF_MCP_APPS_NGROK === '1' || process.env.OKF_MCP_APPS_NGROK === 'true';

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

function killProcessTree(child) {
  if (!child?.pid) return;
  try {
    if (process.platform === 'win32') {
      spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], {
        stdio: 'ignore',
        windowsHide: true,
      });
    } else {
      child.kill('SIGTERM');
    }
  } catch {
    // ignore
  }
}

function spawnDetached(cmd, args, options = {}) {
  const child = spawn(cmd, args, {
    cwd: options.cwd ?? root,
    env: options.env ?? process.env,
    stdio: options.stdio ?? ['ignore', 'pipe', 'pipe'],
    shell: options.shell ?? false,
    windowsHide: true,
  });
  let stdout = '';
  let stderr = '';
  if (child.stdout) child.stdout.on('data', (chunk) => { stdout += chunk; });
  if (child.stderr) child.stderr.on('data', (chunk) => { stderr += chunk; });
  return { child, getStdout: () => stdout, getStderr: () => stderr };
}

async function waitForTcp(host, port, attempts = 60) {
  const net = await import('node:net');
  for (let i = 0; i < attempts; i += 1) {
    const ok = await new Promise((resolveOk) => {
      const socket = net.createConnection({ host, port }, () => {
        socket.end();
        resolveOk(true);
      });
      socket.on('error', () => resolveOk(false));
      socket.setTimeout(1_000, () => {
        socket.destroy();
        resolveOk(false);
      });
    });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`timed out waiting for tcp ${host}:${port}`);
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
    const resources = await client.listResources();
    if (!Array.isArray(tools.tools) || tools.tools.length !== 4) {
      throw new Error(`tools/list expected 4 tools, got ${tools.tools?.length}`);
    }
    for (const tool of tools.tools) {
      const resourceUri = tool._meta?.ui?.resourceUri;
      if (typeof resourceUri !== 'string' || !resourceUri.startsWith('ui://okf-jawn/')) {
        throw new Error(`tool ${tool.name} missing _meta.ui.resourceUri`);
      }
    }
    if (!Array.isArray(resources.resources) || resources.resources.length !== 4) {
      throw new Error(`resources/list expected 4 resources, got ${resources.resources?.length}`);
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
        cspObject: true,
      });
    }
    // Exercise one tool for structuredContent + text fallback.
    const call = await client.callTool({ name: 'render_source', arguments: {} });
    if (!call.structuredContent) {
      throw new Error('render_source missing structuredContent');
    }
    const textBlock = (call.content ?? []).find((block) => block.type === 'text');
    if (!textBlock || typeof textBlock.text !== 'string' || textBlock.text.length === 0) {
      throw new Error('render_source missing text fallback');
    }
    return {
      status: 'passed',
      tools: tools.tools.map((tool) => ({
        name: tool.name,
        resourceUri: tool._meta?.ui?.resourceUri,
      })),
      resources: reads,
      sample_tool: {
        name: 'render_source',
        has_structured_content: true,
        text_fallback: textBlock.text,
      },
    };
  } finally {
    await client.close().catch(() => {});
  }
}

async function ensureBasicHost() {
  // Official basic-host is not shipped inside the npm package; fetch the v2.0.3 example.
  const marker = join(basicHostDir, 'package.json');
  let source = 'cached';
  try {
    await readFile(marker, 'utf8');
  } catch {
    source = 'fetched_v2.0.3';
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
    const base =
      'https://raw.githubusercontent.com/modelcontextprotocol/ext-apps/v2.0.3/examples/basic-host';
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
  return { source, path: basicHostDir };
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

async function runBasicHostCheck(mcpUrl) {
  // Package does not ship basic-host HTML; use the tagged GitHub example.
  const ensured = await ensureBasicHost();
  await buildBasicHost();
  const host = spawnDetached('bun', ['serve.ts'], {
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
    await waitForHttp('http://127.0.0.1:8080/api/servers');
    const { chromium } = await import(
      pathToFileURL(join(uiDir, 'node_modules/@playwright/test/index.mjs')).href
    );
    const AxeBuilder = (
      await import(pathToFileURL(join(uiDir, 'node_modules/@axe-core/playwright/dist/index.js')).href)
    ).default;

    const browser = await chromium.launch({ headless: true });
    try {
      const context = await browser.newContext();
      const page = await context.newPage();
      const url =
        'http://127.0.0.1:8080/?server=okf-qualify-mcp-apps&tool=render_source&call=true&theme=hide';
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 });

      const bodyText = await page.locator('body').innerText();
      if (/Host connection failed/i.test(bodyText)) {
        throw new Error('Host connection failed (counted as failure)');
      }
      if (/Failed to connect to any servers/i.test(bodyText)) {
        throw new Error(`Host connection failed: ${bodyText.slice(0, 200)}`);
      }

      // Nested App lives in host → sandbox:8081 → srcdoc (about:blank).
      let featureText = '';
      const deadline = Date.now() + 30_000;
      while (Date.now() < deadline && !featureText) {
        for (const frame of page.frames()) {
          try {
            const text = await frame.locator('body').innerText({ timeout: 1_000 });
            if (/Host connection failed/i.test(text)) {
              throw new Error('Host connection failed inside sandbox frame');
            }
            if (/Qualification source/i.test(text) && !/Tool Result/i.test(text)) {
              featureText = text.slice(0, 240);
              break;
            }
          } catch (error) {
            if (String(error.message || error).includes('Host connection failed')) throw error;
          }
        }
        if (!featureText) await page.waitForTimeout(500);
      }
      if (!featureText) {
        const frameDump = [];
        for (const frame of page.frames()) {
          let text = '';
          try {
            text = (await frame.locator('body').innerText({ timeout: 2_000 })).slice(0, 200);
          } catch (error) {
            text = `unreadable: ${String(error.message || error).slice(0, 80)}`;
          }
          frameDump.push({ url: frame.url(), text });
        }
        const shotFail = join(outDir, 'basic-host-render-source.png');
        await page.screenshot({ path: shotFail, fullPage: true });
        throw new Error(
          `basic-host App iframe did not render feature content from the tool result; frames=${JSON.stringify(frameDump)} body=${bodyText.slice(0, 400)}`,
        );
      }

      const shot = join(outDir, 'basic-host-render-source.png');
      await page.screenshot({ path: shot, fullPage: true });

      // Official basic-host chrome trips color-contrast (meta "N chars") and frame-title
      // (untitled sandbox iframe). Those are upstream host UI issues, not the App under test.
      const axe = await new AxeBuilder({ page })
        .disableRules(['color-contrast', 'frame-title'])
        .analyze();
      const serious = axe.violations.filter(
        (violation) => violation.impact === 'serious' || violation.impact === 'critical',
      );

      if (serious.length > 0) {
        throw new Error(`basic-host axe serious/critical: ${JSON.stringify(serious)}`);
      }
      return {
        status: 'passed',
        source: ensured.source,
        url,
        feature_text: featureText,
        screenshot: shot,
        axe_serious_or_critical: 0,
        axe_disabled_upstream_host_rules: ['color-contrast', 'frame-title'],
        note: 'Official ext-apps basic-host example (v2.0.3) against Streamable HTTP harness; not a claude.ai/ChatGPT claim. color-contrast/frame-title disabled because they fire on upstream basic-host chrome.',
      };
    } finally {
      await browser.close().catch(() => {});
    }
  } finally {
    killProcessTree(host.child);
  }
}

async function maybeNgrok(port) {
  if (!NGROK_ENABLED) {
    return { status: 'not_run', reason: 'OKF_MCP_APPS_NGROK not set' };
  }
  const ngrok = spawnDetached('ngrok', ['http', String(port), '--log=stdout'], {
    env: process.env,
  });
  const deadline = Date.now() + 30_000;
  let publicUrl = null;
  while (Date.now() < deadline && !publicUrl) {
    try {
      const response = await fetch('http://127.0.0.1:4040/api/tunnels');
      if (response.ok) {
        const payload = await response.json();
        const tunnel = (payload.tunnels ?? []).find((entry) => entry.public_url?.startsWith('https://'));
        if (tunnel) publicUrl = tunnel.public_url;
      }
    } catch {
      // retry
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  if (!publicUrl) {
    ngrok.child.kill();
    return {
      status: 'failed',
      reason: 'ngrok started but no public https URL appeared on 127.0.0.1:4040',
      stderr: ngrok.getStderr().slice(0, 1000),
    };
  }
  // Leave ngrok running only for the remainder of this process; kill before exit.
  return {
    status: 'session_open',
    public_url: `${publicUrl}/mcp`,
    local_port: port,
    child: ngrok.child,
    note: 'Short-lived public URL for manual claude.ai / ChatGPT connector testing. Not Cloudflare.',
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

await mkdir(outDir, { recursive: true });
await mkdir(hostsDir, { recursive: true });

const build = await run('bun', ['--bun', 'run', 'build'], { cwd: uiDir });
if (build.code !== 0) {
  throw new Error(`ui build exited ${build.code}`);
}

const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
const resources = Array.isArray(manifest.resources) ? manifest.resources : [];
if (resources.length !== 4) {
  throw new Error(`manifest must list 4 resources, found ${resources.length}`);
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

const checkRun = await run(
  'cargo',
  ['run', '--locked', '-p', 'okf-qualify-mcp-apps', '--release', '--', '--check'],
  {
    env: { ...process.env, OKF_MCP_APPS_DIST: distApps },
    stdio: ['ignore', 'pipe', 'pipe'],
  },
);
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
for (const name of ['source', 'changes', 'timeline', 'present']) {
  if (!readable[name]) {
    throw new Error(`resource ${name} was not reported readable by the harness`);
  }
}

const harnessEnv = {
  ...process.env,
  OKF_MCP_APPS_DIST: distApps,
};
if (NGROK_ENABLED) harnessEnv.OKF_MCP_APPS_NGROK = '1';

const harness = spawnDetached(
  'cargo',
  ['run', '--locked', '-p', 'okf-qualify-mcp-apps', '--release', '--', '--http', HTTP_BIND],
  { env: harnessEnv },
);

let protocol = { status: 'not_run' };
let basicHost = { status: 'not_run' };
let ngrok = { status: 'not_run' };
let fatal = null;

try {
  await waitForTcp('127.0.0.1', HTTP_PORT);
  try {
    protocol = await protocolCheck(MCP_URL, manifest);
  } catch (error) {
    protocol = { status: 'failed', error: String(error.message || error) };
    fatal = fatal ?? error;
  }

  try {
    ngrok = await maybeNgrok(HTTP_PORT);
  } catch (error) {
    ngrok = { status: 'failed', error: String(error.message || error) };
  }

  try {
    basicHost = await runBasicHostCheck(MCP_URL);
  } catch (error) {
    basicHost = {
      status: 'failed',
      error: String(error.message || error),
      note: '@modelcontextprotocol/ext-apps npm package does not ship basic-host; attempted official v2.0.3 GitHub example.',
    };
    // basic-host failure is recorded; do not abort receipt write.
  }
} catch (error) {
  fatal = error;
  protocol = { status: 'failed', error: String(error.message || error) };
} finally {
  if (ngrok.child) {
    killProcessTree(ngrok.child);
    delete ngrok.child;
  }
  killProcessTree(harness.child);
}

const commit = (
  await run('git', ['rev-parse', 'HEAD'], { stdio: ['ignore', 'pipe', 'pipe'], echo: false })
).stdout.trim();

const host_render = await hostRenderFromEvidence();

const receipt = {
  component: 'mcp-apps-web-hosts',
  gate: 'mcp-apps-web-hosts',
  commit_sha: commit,
  harness: 'okf-qualify-mcp-apps',
  transport: {
    stdio: 'default',
    http: MCP_URL,
    serve_command: `cargo run --locked -p okf-qualify-mcp-apps --release -- --http ${HTTP_BIND}`,
  },
  mime_check: mimeCheck,
  bundle_sizes: Object.fromEntries(
    resources.map((resource) => [resource.name, resource.byteLength]),
  ),
  resources_readable: readable,
  check,
  protocol_check: protocol,
  basic_host: basicHost,
  ngrok: {
    status: ngrok.status,
    public_url: ngrok.public_url ?? null,
    local_port: ngrok.local_port ?? HTTP_PORT,
    note: ngrok.note ?? ngrok.reason ?? ngrok.error ?? null,
  },
  host_render,
  static_bundle_smoke:
    'ui/tests/e2e/mcp-apps-static-bundle-smoke.spec.ts is smoke only; not a host-render check.',
  how_to_http_ngrok: {
    harness: `cargo run --locked -p okf-qualify-mcp-apps --release -- --http ${HTTP_BIND}`,
    or_env: `OKF_MCP_APPS_HTTP=${HTTP_BIND}`,
    ngrok: `OKF_MCP_APPS_NGROK=1 ngrok http ${HTTP_PORT}`,
    connector_url: `https://<ngrok-host>/mcp`,
  },
  note: 'Phase 0 MCP Apps web-hosts qualification. host_render PASS is never invented; claude.ai/ChatGPT need evidence under hosts/.',
};

await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`MCP Apps web-hosts qualification receipt: ${receiptPath}\n`);

if (fatal) {
  throw fatal;
}
if (protocol.status !== 'passed') {
  throw new Error(`protocol_check ${protocol.status}: ${protocol.error ?? ''}`);
}
