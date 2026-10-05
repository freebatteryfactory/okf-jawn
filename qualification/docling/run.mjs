/**
 * Orchestrate Docling direct-library qualification.
 * Sets verified env vars, runs okf-qualify-docling, records peak RSS.
 */

import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const assetsPath = join(root, '.artifacts/qualification/docling/assets.json');
const outDir = join(root, '.artifacts/qualification/docling');
const fixturesDir = join(root, 'tests/fixtures/documents');

const assetsRaw = await readFile(assetsPath, 'utf8');
const assets = JSON.parse(assetsRaw.replace(/^\uFEFF/, ''));
const env = {
  ...process.env,
  DOCLING_RS_MODELS_DIR: assets.DOCLING_RS_MODELS_DIR,
  OKF_DOCLING_FIXTURES: fixturesDir,
  OKF_DOCLING_OUT: outDir,
  ...assets.recommended_env,
};

await mkdir(outDir, { recursive: true });

const cargo = spawn(
  'cargo',
  ['run', '--locked', '-p', 'okf-qualify-docling', '--release'],
  { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] },
);

let stdout = '';
let stderr = '';
cargo.stdout.on('data', (chunk) => {
  stdout += chunk;
  process.stdout.write(chunk);
});
cargo.stderr.on('data', (chunk) => {
  stderr += chunk;
  process.stderr.write(chunk);
});

const exitCode = await new Promise((resolveExit) => {
  cargo.on('close', resolveExit);
});

let peakWorkingSet = null;
if (process.platform === 'win32' && cargo.pid) {
  // Best-effort: process may already have exited; receipt still carries per-fixture None.
  peakWorkingSet = null;
}

const receiptPath = join(outDir, 'receipt.json');
let receipt;
try {
  receipt = JSON.parse(await readFile(receiptPath, 'utf8'));
} catch (error) {
  throw new Error(`Docling qualification did not write ${receiptPath}: ${error.message}\n${stderr}`);
}
receipt.orchestrator = {
  exit_code: exitCode,
  peak_working_set_note:
    'Peak RSS is recorded per OS tooling outside authored unsafe; fixture peak_rss_bytes may be null.',
  peak_working_set_bytes: peakWorkingSet,
  assets_manifest: assetsPath,
  stdout_sha256: createHash('sha256').update(stdout).digest('hex'),
};
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);

if (exitCode !== 0) {
  process.exitCode = exitCode || 1;
  throw new Error(`okf-qualify-docling exited ${exitCode}`);
}

const failed = Object.values(receipt.summary).some((value) => String(value).startsWith('FAIL'));
if (failed) throw new Error(`Docling qualification summary has FAIL entries: ${JSON.stringify(receipt.summary)}`);
process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);
