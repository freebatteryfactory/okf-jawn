/**
 * Orchestrate Docling direct-library qualification.
 *
 * Build the release binary first, then spawn one process per fixture.
 * Peak memory is sampled from that converter process (PeakWorkingSet64 /
 * VmHWM), never from `cargo run`. requireCleanTree runs before any receipt.
 */

import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { requireCleanTree } from '../../scripts/lib/provenance.mjs';

const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const assetsPath = join(root, '.artifacts/qualification/docling/assets.json');
const outDir = join(root, '.artifacts/qualification/docling');
const fixturesDir = join(root, 'tests/fixtures/documents');
const partialDir = join(outDir, 'partial');
const binName =
  process.platform === 'win32' ? 'okf-qualify-docling.exe' : 'okf-qualify-docling';
const binPath = join(root, 'target', 'release', binName);

/** Keep in sync with qualification/docling/src/main.rs fixture_catalog + must_fail + timeout. */
const FIXTURE_RUNS = [
  'sample_with_image.docx',
  'sample_sheet.xlsx',
  'born_digital_text.pdf',
  'scanned_image_only.pdf',
  'table_heavy.pdf',
  'sample_image.png',
  'corpus/word_sample.docx',
  'corpus/xlsx_01.xlsx',
  'corpus/powerpoint_sample.pptx',
  'corpus/redp5110_sampled.pdf',
  'must_fail_truncated.pdf',
  'timeout_probe',
];

function run(cmd, args, options = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(cmd, args, {
      cwd: options.cwd ?? root,
      env: options.env ?? process.env,
      stdio: options.stdio ?? 'inherit',
      windowsHide: true,
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
      resolveRun({ code: code ?? 1, stdout, stderr });
    });
  });
}

function samplePeakBytes(pid) {
  return new Promise((resolveSample) => {
    if (process.platform === 'win32') {
      const ps = spawn(
        'powershell',
        [
          '-NoProfile',
          '-Command',
          `(Get-Process -Id ${pid} -ErrorAction SilentlyContinue).PeakWorkingSet64`,
        ],
        { stdio: ['ignore', 'pipe', 'ignore'], windowsHide: true },
      );
      let buf = '';
      ps.stdout.on('data', (chunk) => {
        buf += chunk;
      });
      ps.on('close', () => {
        const n = Number(String(buf).trim());
        resolveSample(Number.isFinite(n) && n > 0 ? n : null);
      });
      ps.on('error', () => resolveSample(null));
      return;
    }
    // Linux: VmHWM is peak resident set in kB.
    const sh = spawn(
      'bash',
      ['-lc', `awk '/VmHWM:/ {print $2*1024}' /proc/${pid}/status 2>/dev/null`],
      { stdio: ['ignore', 'pipe', 'ignore'] },
    );
    let buf = '';
    sh.stdout.on('data', (chunk) => {
      buf += chunk;
    });
    sh.on('close', () => {
      const n = Number(String(buf).trim());
      resolveSample(Number.isFinite(n) && n > 0 ? n : null);
    });
    sh.on('error', () => resolveSample(null));
  });
}

async function runFixture(only, env) {
  const fixtureOut = join(partialDir, only.replaceAll(/[\\/]/g, '__'));
  await mkdir(fixtureOut, { recursive: true });
  const child = spawn(binPath, [], {
    cwd: root,
    env: {
      ...env,
      OKF_DOCLING_ONLY: only,
      OKF_DOCLING_OUT: fixtureOut,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  });

  let stdout = '';
  let stderr = '';
  let peakWorkingSet = null;
  const sampleNote =
    process.platform === 'win32'
      ? 'PeakWorkingSet64 via Get-Process on the converter process'
      : 'VmHWM via /proc/<pid>/status on the converter process';

  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    process.stdout.write(chunk);
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
    process.stderr.write(chunk);
  });

  const sampler = setInterval(() => {
    if (!child.pid) return;
    samplePeakBytes(child.pid).then((n) => {
      if (n != null) {
        peakWorkingSet = peakWorkingSet == null ? n : Math.max(peakWorkingSet, n);
      }
    });
  }, 50);

  // Sample as soon as we have a pid; fast fixtures can finish before the first interval.
  if (child.pid) {
    peakWorkingSet = await samplePeakBytes(child.pid);
  }

  const exitCode = await new Promise((resolveExit) => {
    child.on('close', resolveExit);
  });
  clearInterval(sampler);

  // Final sample in case the process exited between intervals.
  if (child.pid && peakWorkingSet == null) {
    // Process is gone; peak may still have been captured. Leave as-is.
  }

  const receiptPath = join(fixtureOut, 'receipt.json');
  let report;
  try {
    report = JSON.parse(await readFile(receiptPath, 'utf8'));
  } catch (error) {
    throw new Error(
      `Docling fixture ${only} did not write ${receiptPath}: ${error.message}\n${stderr}`,
    );
  }

  if (exitCode !== 0) {
    throw new Error(`okf-qualify-docling (${only}) exited ${exitCode}\n${stderr}`);
  }
  if (peakWorkingSet == null) {
    throw new Error(
      `Docling fixture ${only}: peak RSS not measured (${sampleNote}); gate incomplete`,
    );
  }

  return {
    only,
    report,
    peakWorkingSet,
    sampleNote,
    stdout_sha256: createHash('sha256').update(stdout).digest('hex'),
  };
}

const commitSha = await requireCleanTree(root);

const assetsRaw = await readFile(assetsPath, 'utf8');
const assets = JSON.parse(assetsRaw.replace(/^\uFEFF/, ''));
const env = {
  ...process.env,
  DOCLING_RS_MODELS_DIR: assets.DOCLING_RS_MODELS_DIR,
  OKF_DOCLING_FIXTURES: fixturesDir,
  ...assets.recommended_env,
};

await mkdir(outDir, { recursive: true });
await rm(partialDir, { recursive: true, force: true });
await mkdir(partialDir, { recursive: true });

const build = await run(
  'cargo',
  ['build', '--locked', '-p', 'okf-qualify-docling', '--release'],
  { stdio: 'inherit' },
);
if (build.code !== 0) {
  throw new Error(`cargo build -p okf-qualify-docling --release exited ${build.code}`);
}

const runs = [];
for (const only of FIXTURE_RUNS) {
  process.stdout.write(`\n=== Docling fixture: ${only} ===\n`);
  runs.push(await runFixture(only, env));
}

const receipts = [];
const summary = {};
let timeout_case = null;
let finding = null;

for (const runResult of runs) {
  const { report, peakWorkingSet, sampleNote, only } = runResult;
  if (only === 'timeout_probe') {
    const tc = report.timeout_case;
    if (!tc || typeof tc !== 'object') {
      throw new Error('timeout_probe run missing timeout_case');
    }
    tc.peak_rss_bytes = peakWorkingSet;
    tc.peak_rss_note = sampleNote;
    timeout_case = tc;
    summary.timeout_probe = tc.outcome;
    continue;
  }
  const fixture = report.receipts?.[0];
  if (!fixture) {
    throw new Error(`fixture run ${only} missing receipts[0]`);
  }
  fixture.peak_rss_bytes = peakWorkingSet;
  fixture.peak_rss_note = sampleNote;
  receipts.push(fixture);
  const summaryKey =
    only === 'must_fail_truncated.pdf' ? 'must_fail_truncated.pdf' : only;
  summary[summaryKey] = fixture.outcome;

  if (
    summaryKey === 'must_fail_truncated.pdf' &&
    (fixture.status === 'Success' || fixture.status === 'PartialSuccess')
  ) {
    finding = 'converter accepts truncated PDF';
    summary[summaryKey] = 'FAIL_expected_failure';
    fixture.outcome = 'FAIL_expected_failure';
    fixture.finding = finding;
  }
}

const receipt = {
  converter_crate: runs[0]?.report?.converter_crate ?? null,
  fixtures_dir: fixturesDir,
  models_dir: assets.DOCLING_RS_MODELS_DIR,
  receipts,
  summary,
  timeout_case,
  orchestrator: {
    commit_sha: commitSha,
    git_sha: commitSha,
    inputs: [
      'qualification/docling',
      'tests/fixtures/documents',
      'Cargo.toml',
      'Cargo.lock',
    ],
    build: 'cargo build --locked -p okf-qualify-docling --release',
    peak_memory_source:
      process.platform === 'win32'
        ? 'PeakWorkingSet64 of each converter process'
        : 'VmHWM of each converter process',
    assets_manifest: assetsPath,
    finding,
    per_fixture: runs.map((item) => ({
      only: item.only,
      peak_rss_bytes: item.peakWorkingSet,
      peak_rss_note: item.sampleNote,
      stdout_sha256: item.stdout_sha256,
    })),
  },
};

const receiptPath = join(outDir, 'receipt.json');
await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
process.stdout.write(`Docling qualification receipt: ${receiptPath}\n`);

if (finding) {
  process.exitCode = 1;
  throw new Error(
    `Docling must_fail FAIL with finding: ${finding}. Fixture must_fail_truncated.pdf was not altered.`,
  );
}

const failed = Object.values(summary).some((value) => String(value).startsWith('FAIL'));
if (failed) {
  process.exitCode = 1;
  throw new Error(`Docling qualification summary has FAIL entries: ${JSON.stringify(summary)}`);
}
