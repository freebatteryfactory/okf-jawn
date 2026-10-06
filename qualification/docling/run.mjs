/**
 * Orchestrate Docling direct-library qualification. Thin entry point:
 *   lib/assets.mjs    re-hashes every model asset before anything converts;
 *   lib/build.mjs     reads the docling features of this build from cargo;
 *   lib/runner.mjs    runs one process per fixture and samples its peak memory once;
 *   lib/evidence.mjs  re-reads the Markdown, document export and page images each process wrote;
 *   lib/receipt.mjs   judges every criterion and composes the receipt;
 *   scripts/lib/provenance.mjs supplies the receipt header behind its clean-tree guard.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/docling/run.mjs [--record]
 * Needs .artifacts/qualification/docling/assets.json naming the verified model assets; a
 * missing or changed asset stops the run before the build.
 * Writes .artifacts/qualification/docling/receipt.json; --record also copies it to
 * qualification/receipts/docling.json. Exit code: 0 PASS, 2 FAIL, 3 INCOMPLETE (lib/criteria.mjs
 * EXIT_CODES); 1 is a run that stopped without writing a receipt.
 *
 * Single-fixture mode, for iterating without the large fixtures: set OKF_DOCLING_ONLY to one
 * or more names from FIXTURE_RUNS, comma-separated. Only those run; the receipt goes to
 * .artifacts/qualification/docling/only/receipt.json, says so in `scope`, leaves every
 * other fixture's criteria not_judged (so it is INCOMPLETE, never PASS) and cannot be recorded.
 */

import { createHash } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { receiptHeader, recordReceipt } from '../../scripts/lib/provenance.mjs';
import { buildRelease, exec, lockedPackage, lockedPackages } from '../lib/cargo.mjs';
import { verifyAssets } from './lib/assets.mjs';
import { TREE_ARGS, buildFacts } from './lib/build.mjs';
import { exitCodeFor } from './lib/criteria.mjs';
import { loadEvidence } from './lib/evidence.mjs';
import { DOCLING_INPUTS, FIXTURE_RUNS, TIMEOUT_PROBE, buildDoclingReceipt } from './lib/receipt.mjs';
import { runFixtureProcess } from './lib/runner.mjs';

const PACKAGE = 'okf-qualify-docling';
const FIXTURES = 'tests/fixtures/documents';
const MANIFEST = '.artifacts/qualification/docling/assets.json';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const record = process.argv.includes('--record');
const only = (process.env.OKF_DOCLING_ONLY ?? '')
  .split(',')
  .map((name) => name.trim())
  .filter(Boolean);
const unknown = only.filter((name) => !FIXTURE_RUNS.includes(name));
if (unknown.length) throw new Error(`OKF_DOCLING_ONLY names no fixture run: ${unknown.join(', ')}. Known: ${FIXTURE_RUNS.join(', ')}`);
if (only.length && record) throw new Error('A single-fixture run (OKF_DOCLING_ONLY) is not the qualification and cannot be recorded');
const selected = only.length ? FIXTURE_RUNS.filter((name) => only.includes(name)) : FIXTURE_RUNS;

const outDir = join(root, '.artifacts/qualification/docling', ...(only.length ? ['only'] : []));
const partialDir = join(outDir, 'partial');
const fixturesDir = join(root, FIXTURES);

const header = await receiptHeader(root, DOCLING_INPUTS);

let manifestBytes;
try {
  manifestBytes = await readFile(join(root, MANIFEST));
} catch (error) {
  throw new Error(
    `Docling model assets manifest not readable at ${join(root, MANIFEST)}: ${error.message}. Qualification needs the verified model assets; a missing asset is never qualified.`,
  );
}
process.stdout.write('Verifying Docling model assets against the manifest hashes...\n');
const { manifest, verified } = await verifyAssets(manifestBytes);
process.stdout.write(`Verified ${verified.matched} of ${verified.count} assets (${verified.bytes_total} bytes) against the manifest hashes.\n`);

const sources = JSON.parse(await readFile(join(fixturesDir, 'SOURCES.json'), 'utf8'));
const lockText = await readFile(join(root, 'Cargo.lock'), 'utf8');
const docling = lockedPackage(lockText, 'docling');
const converter = {
  crate: 'docling',
  version: docling.version,
  checksum: docling.checksum,
  docling_core_versions: lockedPackages(lockText, 'docling-core').map((entry) => entry.version),
  source: 'Cargo.lock',
};

// What the converter processes see beyond the caller's environment. Recorded as settings.
const environment = {
  DOCLING_RS_MODELS_DIR: manifest.DOCLING_RS_MODELS_DIR,
  ...manifest.recommended_env,
  OKF_DOCLING_FIXTURES: fixturesDir,
  OKF_DOCLING_CRATE_VERSION: docling.version,
  OKF_DOCLING_HOLD: '1',
};
const env = { ...process.env, ...environment };

await mkdir(outDir, { recursive: true });
await rm(partialDir, { recursive: true, force: true });
await mkdir(partialDir, { recursive: true });

const binPath = await buildRelease(root, PACKAGE);
const tree = await exec('cargo', TREE_ARGS, { cwd: root });
if (tree.code !== 0) throw new Error(`cargo ${TREE_ARGS.join(' ')} exited ${tree.code}\n${tree.stderr}`);
const build = buildFacts({
  harnessToml: await readFile(join(root, 'qualification/docling/Cargo.toml'), 'utf8'),
  workspaceToml: await readFile(join(root, 'Cargo.toml'), 'utf8'),
  treeText: tree.stdout,
  // buildRelease (qualification/lib/cargo.mjs) runs exactly this and does not return its argv.
  command: `cargo build --locked --release -p ${PACKAGE}`,
});

const runs = [];
for (const name of selected) {
  process.stdout.write(`\n=== Docling fixture: ${name} ===\n`);
  const fixtureOut = join(partialDir, name.replaceAll(/[\\/]/g, '__'));
  await mkdir(fixtureOut, { recursive: true });
  const run = await runFixtureProcess({
    command: binPath,
    cwd: root,
    env: { ...env, OKF_DOCLING_ONLY: name, OKF_DOCLING_OUT: fixtureOut },
    onStdout: (text) => process.stdout.write(text),
    onStderr: (text) => process.stderr.write(text),
  });
  let report = null;
  try {
    report = JSON.parse(await readFile(join(fixtureOut, 'receipt.json'), 'utf8'));
  } catch {
    report = null; // lib/receipt.mjs records a process without a receipt as a harness error
  }
  const written = name === TIMEOUT_PROBE ? report?.timeout_case?.document : report?.receipts?.[0]?.document;
  runs.push({
    only: name,
    run: { ...run, stdoutSha256: createHash('sha256').update(run.stdout).digest('hex') },
    report,
    evidence: await loadEvidence(fixtureOut, written),
  });
}

const receipt = buildDoclingReceipt({
  header,
  converter,
  platform: process.platform,
  build,
  assets: { ...verified, manifest: MANIFEST, models_dir: manifest.DOCLING_RS_MODELS_DIR, models_source: manifest.models_source ?? null },
  environment,
  sources,
  runs,
  scope: only.length ? { mode: 'only', fixtures: selected, qualification: false } : { mode: 'all', fixtures: selected, qualification: true },
  paths: { fixtures_dir: FIXTURES, sources: `${FIXTURES}/SOURCES.json`, per_fixture_evidence: join(outDir, 'partial') },
  finishedAt: new Date().toISOString(),
});

const receiptPath = join(outDir, 'receipt.json');
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);
if (record) {
  process.stdout.write(`Docling receipt recorded: ${await recordReceipt(root, 'docling', receipt)}\n`);
}

// The receipt's result is the fold of its criteria (lib/criteria.mjs); this file only reports it.
process.stdout.write(`Docling qualification result: ${receipt.result}
`);
if (receipt.harness_error) process.stderr.write(`Harness error: ${receipt.harness_error}
`);
if (receipt.finding) process.stderr.write(`Finding for the owner: ${receipt.finding}. Fixture must_fail_truncated.pdf was not altered.
`);
for (const item of receipt.failures) {
  process.stderr.write(`${item.result} ${item.fixture ?? 'run'}${item.criterion ? ` ${item.criterion}` : ''}: ${item.detail}
`);
}
process.exitCode = exitCodeFor(receipt.result);
