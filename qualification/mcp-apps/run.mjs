/**
 * Orchestrate Phase 0 MCP Apps qualification (MIME + harness --check).
 * Host render evidence is recorded only by a later Playwright / web-host step.
 */

import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const uiDir = join(root, 'ui');
const distApps = join(uiDir, 'dist-apps');
const manifestPath = join(distApps, 'manifest.json');
const outDir = join(root, '.artifacts/qualification/mcp-apps');
const receiptPath = join(outDir, 'receipt.json');

const require = createRequire(join(uiDir, 'package.json'));
const { RESOURCE_MIME_TYPE } = require('@modelcontextprotocol/ext-apps');

function run(cmd, args, options = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(cmd, args, {
      cwd: options.cwd ?? root,
      env: options.env ?? process.env,
      stdio: options.stdio ?? 'inherit',
    });
    let stdout = '';
    let stderr = '';
    if (child.stdout) {
      child.stdout.on('data', (chunk) => {
        stdout += chunk;
        process.stdout.write(chunk);
      });
    }
    if (child.stderr) {
      child.stderr.on('data', (chunk) => {
        stderr += chunk;
        process.stderr.write(chunk);
      });
    }
    child.on('error', reject);
    child.on('close', (code) => {
      resolveRun({ code: code ?? 1, stdout, stderr });
    });
  });
}

await mkdir(outDir, { recursive: true });

// Always rebuild so qualification never reads a stale dist-apps tree.
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

const { code, stdout, stderr } = await run(
  'cargo',
  ['run', '--locked', '-p', 'okf-qualify-mcp-apps', '--release', '--', '--check'],
  {
    env: { ...process.env, OKF_MCP_APPS_DIST: distApps },
    stdio: ['ignore', 'pipe', 'pipe'],
  },
);
if (code !== 0) {
  throw new Error(`okf-qualify-mcp-apps --check exited ${code}\n${stderr}`);
}

let check;
try {
  // Harness prints one compact JSON object on stdout.
  const trimmed = stdout.trim();
  const start = trimmed.indexOf('{');
  const end = trimmed.lastIndexOf('}');
  if (start < 0 || end < start) {
    throw new Error('no JSON object in stdout');
  }
  check = JSON.parse(trimmed.slice(start, end + 1));
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

const commit = (
  await run('git', ['rev-parse', 'HEAD'], { stdio: ['ignore', 'pipe', 'pipe'] })
).stdout.trim();

const receipt = {
  component: 'mcp-apps-phase0',
  commit_sha: commit,
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
    status: 'not_run',
    meaning:
      'Host render is recorded only after basic-host Playwright/axe or a real web host (claude.ai / ChatGPT) observes a feature component. This receipt does not claim a host render.',
  },
  note: 'Phase 0 MIME + resource readability from a fresh ui build. Product MCP server over Application is a later gate.',
};
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`MCP Apps qualification receipt: ${receiptPath}\n`);
