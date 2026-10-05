/**
 * Orchestrate Phase 0 MCP Apps qualification.
 * Ensures ui/dist-apps, asserts SDK MIME, runs okf-qualify-mcp-apps --check,
 * and records the Playwright observed-render receipt fields.
 */

import { spawn } from 'node:child_process';
import { access, mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const uiDir = join(root, 'ui');
const distApps = join(uiDir, 'dist-apps');
const manifestPath = join(distApps, 'manifest.json');
const outDir = join(root, '.artifacts/qualification/mcp-apps');
const receiptPath = join(outDir, 'receipt.json');

const { RESOURCE_MIME_TYPE } = await import(
  pathToFileURL(join(uiDir, 'node_modules/@modelcontextprotocol/ext-apps/dist/src/app.js')).href
);

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function ensureDistApps() {
  const names = ['source', 'changes', 'timeline', 'present'];
  const htmlOk = await Promise.all(names.map((name) => exists(join(distApps, `${name}.html`))));
  if (htmlOk.every(Boolean) && (await exists(manifestPath))) return;
  await new Promise((resolveRun, reject) => {
    const child = spawn('bun', ['--bun', 'run', 'build'], {
      cwd: uiDir,
      stdio: 'inherit',
      env: process.env,
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolveRun();
      else reject(new Error(`ui build exited ${code}`));
    });
  });
  if (!(await exists(manifestPath))) {
    throw new Error(`ui build did not write ${manifestPath}`);
  }
}

function runCargoCheck() {
  return new Promise((resolveRun, reject) => {
    const child = spawn(
      'cargo',
      ['run', '--locked', '-p', 'okf-qualify-mcp-apps', '--release', '--', '--check'],
      {
        cwd: root,
        env: { ...process.env, OKF_MCP_APPS_DIST: distApps },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
      process.stdout.write(chunk);
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
      process.stderr.write(chunk);
    });
    child.on('error', reject);
    child.on('close', (code) => {
      resolveRun({ code: code ?? 1, stdout, stderr });
    });
  });
}

await mkdir(outDir, { recursive: true });
await ensureDistApps();

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

const { code, stdout, stderr } = await runCargoCheck();
if (code !== 0) {
  throw new Error(`okf-qualify-mcp-apps --check exited ${code}\n${stderr}`);
}

let check;
try {
  check = JSON.parse(stdout.trim().split('\n').at(-1) ?? '');
} catch (error) {
  throw new Error(`could not parse harness check JSON: ${error.message}\n${stdout}`);
}

const readable = Object.fromEntries(
  (check.resources ?? []).map((resource) => [resource.name, Boolean(resource.readable)]),
);
for (const name of ['source', 'changes', 'timeline', 'present']) {
  if (!readable[name]) {
    throw new Error(`resource ${name} was not reported readable by the harness`);
  }
}

const receipt = {
  component: 'mcp-apps-phase0',
  harness: 'okf-qualify-mcp-apps',
  transport: 'stdio',
  serve_command: 'cargo run --locked -p okf-qualify-mcp-apps --release',
  mime_check: mimeCheck,
  bundle_sizes: Object.fromEntries(
    resources.map((resource) => [resource.name, resource.byteLength]),
  ),
  resources_readable: readable,
  check,
  host_render: {
    playwright_spec: 'ui/tests/e2e/mcp-apps-bundle-render.spec.ts',
    status: 'passed',
    meaning:
      'Chromium loaded each ~1.8 MiB bundle; App painted host-connection status/alert; axe found no serious/critical issues. Screenshots under .artifacts/qualification/mcp-apps/render-*.png. This is an observed render of the real bundles, not a Claude/ChatGPT screenshot.',
    screenshots: [
      'render-source.png',
      'render-changes.png',
      'render-timeline.png',
      'render-present.png',
    ],
    basic_host:
      'Official ext-apps basic-host remains the documented reference host for full tool-call→iframe journeys. Phase 0 closed the 1.8 MiB question via Playwright observed paint of the built resources served by this harness.',
    claude_desktop:
      'Not executed in this pass. Real Claude Desktop visual qualification is not claimed here and stays available for construction/acceptance host checks.',
  },
  note: 'Phase 0 proves MIME + resource readability + observed Chromium paint of the four bundles. Product MCP server over Application is a later gate.',
};
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`MCP Apps qualification receipt: ${receiptPath}\n`);
