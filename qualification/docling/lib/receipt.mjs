/**
 * Compose the Docling qualification receipt from per-fixture process results.
 * Pure: no I/O and no imports, so the rules are testable without cargo or model assets.
 *
 * The Rust harness judges each conversion (stage, outcome, finding). This module adds
 * what only the orchestrator knows: whether the peak was measured, whether the process
 * behaved, and whether a must-fail refusal is attributable to the truncated input.
 */

export const MUST_FAIL = 'must_fail_truncated.pdf';
export const TIMEOUT_PROBE = 'timeout_probe';

/** Execution order. Kept equal to fixture_catalog() + must_fail in src/main.rs and to SOURCES.json by tests on both sides. */
export const FIXTURE_RUNS = [
  'sample_with_image.docx',
  'sample_sheet.xlsx',
  'born_digital_text.pdf',
  'scanned_image_only.pdf',
  'scanned_text.pdf',
  'table_heavy.pdf',
  'sample_image.png',
  'text_image.png',
  'corpus/word_sample.docx',
  'corpus/xlsx_01.xlsx',
  'corpus/powerpoint_sample.pptx',
  'corpus/redp5110_sampled.pdf',
  MUST_FAIL,
  TIMEOUT_PROBE,
];

export const DOCLING_INPUTS = [
  'qualification/docling',
  'qualification/lib',
  'tests/fixtures/documents',
  'Cargo.toml',
  'Cargo.lock',
];

const passed = (outcome) => String(outcome).startsWith('PASS');

/** One fixture's receipt entry: the harness receipt plus memory, or a harness FAIL. */
export function fixtureEntry({ only, run, report }) {
  const measured = typeof run.peakRssBytes === 'number' && run.peakRssBytes > 0;
  const memory = {
    peak_rss_bytes: measured ? run.peakRssBytes : null,
    peak_rss_note: run.peakRssNote,
    memory: measured ? 'PASS' : 'FAIL_not_measured',
  };
  const base = only === TIMEOUT_PROBE ? report?.timeout_case : report?.receipts?.[0];
  if (run.exitCode !== 0 || base === null || typeof base !== 'object') {
    return {
      fixture: only,
      outcome: 'FAIL_harness',
      stage: null,
      status: null,
      finding: null,
      harness_exit_code: run.exitCode,
      harness_signal: run.signal,
      harness_timed_out: run.timedOut,
      harness_error: String(run.spawnError ?? run.stderr ?? '').slice(-2000),
      ...memory,
    };
  }
  return { ...base, ...memory };
}

/** The finished receipt. `result` is PASS only when every outcome and every memory entry passes. */
export function buildDoclingReceipt({ header, converter, platform, modelsDir, runs, finishedAt }) {
  const entries = runs.map((item) => ({ only: item.only, entry: fixtureEntry(item) }));

  // A PDF refusal proves nothing when the PDF pipeline itself is not working:
  // docling loads its models before it reads the file, so every PDF then errors.
  const pdfConverted = entries.some(
    ({ only, entry }) => only.endsWith('.pdf') && only !== MUST_FAIL && passed(entry.outcome),
  );
  for (const { only, entry } of entries) {
    if (only === MUST_FAIL && passed(entry.outcome) && !pdfConverted) {
      entry.outcome = 'FAIL_refusal_unproven';
      entry.refusal_note =
        'no other PDF fixture converted successfully in this run, so the refusal cannot be attributed to the truncated input';
    }
  }

  const summary = {};
  const memory_summary = {};
  for (const only of FIXTURE_RUNS) {
    const found = entries.find((item) => item.only === only);
    summary[only] = found ? found.entry.outcome : 'FAIL_not_run';
    memory_summary[only] = found ? found.entry.memory : 'FAIL_not_measured';
  }
  const mustFail = entries.find((item) => item.only === MUST_FAIL)?.entry;
  const ok = Object.values(summary).every(passed) && Object.values(memory_summary).every((value) => value === 'PASS');

  return {
    ...header,
    component: 'docling-library-qualification',
    finished_at: finishedAt,
    result: ok ? 'PASS' : 'FAIL',
    finding: mustFail?.finding ?? null,
    converter,
    build: 'cargo build --locked --release -p okf-qualify-docling',
    platform,
    peak_memory_source:
      platform === 'win32'
        ? 'PeakWorkingSet64 of each converter process, read once before it exits'
        : 'VmHWM of each converter process, read once before it exits',
    fixtures_dir: 'tests/fixtures/documents',
    models_dir: modelsDir,
    assets_manifest: '.artifacts/qualification/docling/assets.json',
    summary,
    memory_summary,
    receipts: entries.filter((item) => item.only !== TIMEOUT_PROBE).map((item) => item.entry),
    timeout_case: entries.find((item) => item.only === TIMEOUT_PROBE)?.entry ?? null,
    per_fixture: runs.map((item) => ({
      only: item.only,
      exit_code: item.run.exitCode,
      signal: item.run.signal,
      timed_out: item.run.timedOut,
      done: item.run.done !== null,
      peak_rss_bytes: item.run.peakRssBytes,
      peak_rss_note: item.run.peakRssNote,
      stdout_sha256: item.run.stdoutSha256,
    })),
  };
}
