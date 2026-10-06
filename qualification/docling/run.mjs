/**
 * Orchestrate Docling direct-library qualification. Thin entry point:
 *   scripts/lib/provenance.mjs supplies the receipt header behind its clean-tree guard;
 *   lib/orchestrate.mjs runs the steps and always writes a receipt for this run;
 *   lib/assets.mjs    re-hashes every model asset before anything converts;
 *   lib/build.mjs     reads the docling features of this build from cargo;
 *   lib/native.mjs    records the ONNX Runtime library the build downloaded and links;
 *   lib/runner.mjs    runs one process per fixture and samples its peak memory once;
 *   lib/evidence.mjs  re-reads the Markdown, document export and page images each process wrote;
 *   lib/receipt.mjs   judges every criterion and composes the receipt;
 *   lib/criteria.mjs  declares the criteria and folds them into the result.
 *
 * Usage (from PowerShell, cargo on PATH): bun qualification/docling/run.mjs
 * Needs .artifacts/qualification/docling/assets.json naming the verified model assets.
 * Writes .artifacts/qualification/docling/receipt.json and records nothing: a finished receipt
 * of any result is recorded by `bun qualification/record.mjs docling`, the one recording path.
 *
 * Exit code (lib/criteria.mjs EXIT_CODES): 0 PASS, 2 FAIL (a required criterion failed),
 * 3 INCOMPLETE (the environment stopped a judgement, or a required criterion was not judged).
 * 1 is a run refused before it started, with no receipt written: an argument, a dirty tree,
 * an unknown fixture name, or a defect in this harness.
 *
 * Single-fixture mode, for iterating without the large fixtures: set OKF_DOCLING_ONLY to one
 * or more names from FIXTURE_RUNS, comma-separated. Only those run; the receipt goes to
 * .artifacts/qualification/docling/only/receipt.json, says so in `scope`, leaves every
 * other fixture's criteria not_judged (so it is INCOMPLETE, never PASS); record.mjs does not
 * read that path.
 */

import { mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { receiptHeader } from '../../scripts/lib/provenance.mjs';
import { buildRelease, exec } from '../lib/cargo.mjs';
import { sha256File, verifyAssets } from './lib/assets.mjs';
import { exitCodeFor } from './lib/criteria.mjs';
import { loadEvidence } from './lib/evidence.mjs';
import { qualify } from './lib/orchestrate.mjs';
import { DOCLING_INPUTS, FIXTURE_RUNS } from './lib/receipt.mjs';
import { runFixtureProcess } from './lib/runner.mjs';

if (process.argv.length > 2) throw new Error(`run.mjs takes no argument (got ${process.argv.slice(2).join(' ')}). To record a finished receipt: bun qualification/record.mjs docling`);

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const only = (process.env.OKF_DOCLING_ONLY ?? '')
  .split(',')
  .map((name) => name.trim())
  .filter(Boolean);
const unknown = only.filter((name) => !FIXTURE_RUNS.includes(name));
if (unknown.length) throw new Error(`OKF_DOCLING_ONLY names no fixture run: ${unknown.join(', ')}. Known: ${FIXTURE_RUNS.join(', ')}`);
const selected = only.length ? FIXTURE_RUNS.filter((name) => only.includes(name)) : FIXTURE_RUNS;

const header = await receiptHeader(root, DOCLING_INPUTS);

const { receipt, receiptPath } = await qualify({
  root,
  header,
  outDir: join(root, '.artifacts/qualification/docling', ...(only.length ? ['only'] : [])),
  selected,
  scope: only.length ? { mode: 'only', fixtures: selected, qualification: false } : { mode: 'all', fixtures: selected, qualification: true },
  env: process.env,
  platform: process.platform,
  io: {
    readFile,
    writeFile,
    mkdir,
    rm,
    readdir,
    stat,
    sha256File,
    verifyAssets,
    buildRelease,
    exec,
    runFixtureProcess,
    loadEvidence,
    log: (text) => process.stdout.write(text),
    logError: (text) => process.stderr.write(text),
    now: () => new Date().toISOString(),
  },
});

process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);

// The receipt's result is the fold of its criteria (lib/criteria.mjs); this file only reports it.
process.stdout.write(`Docling qualification result: ${receipt.result}\n`);
if (receipt.harness_error) process.stderr.write(`Harness error: ${receipt.harness_error}\n`);
if (receipt.finding) process.stderr.write(`Finding for the owner: ${receipt.finding}. Fixture must_fail_truncated.pdf was not altered.\n`);
for (const item of receipt.failures) {
  process.stderr.write(`${item.result} ${item.fixture ?? 'run'}${item.criterion ? ` ${item.criterion}` : ''}: ${item.detail}\n`);
}
process.exitCode = exitCodeFor(receipt.result);
