/**
 * Orchestrate Docling direct-library qualification. Thin entry point:
 *   lib/runner.mjs  runs one process per fixture and samples its peak memory once;
 *   lib/receipt.mjs composes and judges the receipt;
 *   scripts/lib/provenance.mjs supplies the header behind requireCleanTree.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/docling/run.mjs [--record]
 * Needs .artifacts/qualification/docling/assets.json naming the verified model assets.
 * Writes .artifacts/qualification/docling/receipt.json; --record also copies it to
 * qualification/receipts/docling.json. Exits non-zero unless the receipt result is PASS.
 */

import { createHash } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
import { buildRelease, lockedPackage, lockedPackages } from '../lib/cargo.mjs';
import { DOCLING_INPUTS, FIXTURE_RUNS, buildDoclingReceipt } from './lib/receipt.mjs';
import { runFixtureProcess } from './lib/runner.mjs';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const outDir = join(root, '.artifacts/qualification/docling');
const assetsPath = join(outDir, 'assets.json');
const partialDir = join(outDir, 'partial');
const fixturesDir = join(root, 'tests/fixtures/documents');
const record = process.argv.includes('--record');

const header = await receiptHeader(root, DOCLING_INPUTS);

let assets;
try {
  assets = JSON.parse((await readFile(assetsPath, 'utf8')).replace(/^\uFEFF/, ''));
} catch (error) {
  throw new Error(
    `Docling model assets manifest not readable at ${assetsPath}: ${error.message}. Qualification needs the verified model assets; a missing asset is never qualified.`,
  );
}

const lockText = await readFile(join(root, 'Cargo.lock'), 'utf8');
const docling = lockedPackage(lockText, 'docling');
const converter = {
  crate: 'docling',
  version: docling.version,
  checksum: docling.checksum,
  docling_core_versions: lockedPackages(lockText, 'docling-core').map((entry) => entry.version),
  source: 'Cargo.lock',
};

const env = {
  ...process.env,
  DOCLING_RS_MODELS_DIR: assets.DOCLING_RS_MODELS_DIR,
  OKF_DOCLING_FIXTURES: fixturesDir,
  ...assets.recommended_env,
  OKF_DOCLING_CRATE_VERSION: docling.version,
  OKF_DOCLING_HOLD: '1',
};

await mkdir(outDir, { recursive: true });
await rm(partialDir, { recursive: true, force: true });
await mkdir(partialDir, { recursive: true });

const binPath = await buildRelease(root, 'okf-qualify-docling');

const runs = [];
for (const only of FIXTURE_RUNS) {
  process.stdout.write(`\n=== Docling fixture: ${only} ===\n`);
  const fixtureOut = join(partialDir, only.replaceAll(/[\\/]/g, '__'));
  await mkdir(fixtureOut, { recursive: true });
  const run = await runFixtureProcess({
    command: binPath,
    cwd: root,
    env: { ...env, OKF_DOCLING_ONLY: only, OKF_DOCLING_OUT: fixtureOut },
    onStdout: (text) => process.stdout.write(text),
    onStderr: (text) => process.stderr.write(text),
  });
  let report = null;
  try {
    report = JSON.parse(await readFile(join(fixtureOut, 'receipt.json'), 'utf8'));
  } catch {
    report = null; // judged as FAIL_harness by lib/receipt.mjs
  }
  runs.push({
    only,
    run: { ...run, stdoutSha256: createHash('sha256').update(run.stdout).digest('hex') },
    report,
  });
}

const receipt = buildDoclingReceipt({
  header,
  converter,
  platform: process.platform,
  modelsDir: assets.DOCLING_RS_MODELS_DIR,
  runs,
  finishedAt: new Date().toISOString(),
});

const receiptPath = join(outDir, 'receipt.json');
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);
if (record) {
  process.stdout.write(`Docling receipt recorded: ${await recordReceipt(root, 'docling', receipt)}\n`);
}

if (receipt.result !== 'PASS') {
  const finding = receipt.finding
    ? ` Finding for the owner: ${receipt.finding}. Fixture must_fail_truncated.pdf was not altered.`
    : '';
  throw new Error(
    `Docling qualification FAIL.${finding} ${JSON.stringify({ summary: receipt.summary, memory: receipt.memory_summary })}`,
  );
}
