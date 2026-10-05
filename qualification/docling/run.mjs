/**
 * Orchestrate Docling direct-library qualification.
 * Sets verified env vars, runs okf-qualify-docling, samples peak working set while running.
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
let peakWorkingSet = null;
let sampleNote = 'not measured';

cargo.stdout.on('data', (chunk) => {
  stdout += chunk;
  process.stdout.write(chunk);
});
cargo.stderr.on('data', (chunk) => {
  stderr += chunk;
  process.stderr.write(chunk);
});

if (process.platform === 'win32') {
  const sampler = setInterval(() => {
    if (!cargo.pid) return;
    try {
      // PowerShell Get-Process PeakWorkingSet64 while the child is alive.
      const ps = spawn(
        'powershell',
        [
          '-NoProfile',
          '-Command',
          `(Get-Process -Id ${cargo.pid} -ErrorAction SilentlyContinue).PeakWorkingSet64`,
        ],
        { stdio: ['ignore', 'pipe', 'ignore'] },
      );
      let buf = '';
      ps.stdout.on('data', (chunk) => {
        buf += chunk;
      });
      ps.on('close', () => {
        const n = Number(String(buf).trim());
        if (Number.isFinite(n) && n > 0) {
          peakWorkingSet = peakWorkingSet == null ? n : Math.max(peakWorkingSet, n);
          sampleNote = 'PeakWorkingSet64 sampled via Get-Process while cargo run was alive';
        }
      });
    } catch {
      // leave as not measured
    }
  }, 500);
  cargo.on('close', () => clearInterval(sampler));
}

const exitCode = await new Promise((resolveExit) => {
  cargo.on('close', resolveExit);
});

const receiptPath = join(outDir, 'receipt.json');
let receipt;
try {
  receipt = JSON.parse(await readFile(receiptPath, 'utf8'));
} catch (error) {
  throw new Error(`Docling qualification did not write ${receiptPath}: ${error.message}\n${stderr}`);
}

const commit = (
  await new Promise((resolveSha, reject) => {
    const child = spawn('git', ['rev-parse', 'HEAD'], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '';
    child.stdout.on('data', (c) => {
      out += c;
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) resolveSha(out.trim());
      else reject(new Error('git rev-parse failed'));
    });
  })
).trim();

receipt.orchestrator = {
  exit_code: exitCode,
  commit_sha: commit,
  peak_working_set_note: sampleNote,
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
if (failed) {
  throw new Error(`Docling qualification summary has FAIL entries: ${JSON.stringify(receipt.summary)}`);
}
process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);
