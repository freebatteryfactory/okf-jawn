/**
 * Compose the Docling qualification receipt from per-fixture process results.
 * Pure: no I/O; the judging rules live in expect.mjs, document.mjs, build.mjs and assets.mjs.
 *
 * The Rust harness observes (stage, status, errors, the Markdown and document export, page
 * images, settings). This module judges, per fixture, each criterion the specification
 * needs and keeps a criterion's evidence beside its status:
 *   conversion        the converter's own result (Rust `outcome`)
 *   evidence          (only when it fails) a file the process wrote is unreadable or changed
 *   content           expected strings, rows and counts from SOURCES.json were found
 *   page_renders      one image per page, with pixel size and SHA-256 (PDF and image fixtures)
 *   provenance        every text item has a page and a box inside it (PDF and image fixtures)
 *   memory_measured   a peak was measured; no limit is applied here
 *   refusal           (must-fail only) what kind of refusal, attributable to the input
 * A status is PASS, FAIL, unavailable (counts as FAIL), or one of the non-judging words
 * not_applicable / not_exercised / recorded_not_judged, each with its reason.
 */

import { inventoryCheck } from './assets.mjs';
import { classifyRefusal } from './build.mjs';
import { bodyFacts, converterOptions, describeDocument, judgePageRenders, judgeProvenance } from './document.mjs';
import { MATCH_RULES, judgeContent } from './expect.mjs';

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

/** Formats the library converts through its paginated PDF/image pipeline. */
const PAGINATED_FORMATS = ['pdf', 'image'];
const NON_JUDGING = ['not_applicable', 'not_exercised', 'recorded_not_judged'];
/** Converter options the receipt lifts out of the DocumentConverter Debug text (SPEC section 5: settings). */
const SETTING_NAMES = [
  'generate_page_images',
  'images_scale',
  'ocr_lang',
  'ocr_mode',
  'ocr_engine',
  'ocr_scale',
  'no_ocr',
  'skip_ocr',
  'force_full_page_ocr',
  'no_table_former',
  'compact_tables',
  'skip_empty_cells',
  'heading_hierarchy',
  'page_range',
  'document_timeout',
];

export const MEMORY_STATEMENT =
  'peak_rss_bytes is a measurement. This harness applies no memory limit (limit_applied false), so no fixture passes or fails on its size; a memory limit is a later product gate.';

const passed = (outcome) => String(outcome).startsWith('PASS');
const failing = (status) => status !== 'PASS' && !NON_JUDGING.includes(status);

function memoryOf(run) {
  const measured = typeof run.peakRssBytes === 'number' && run.peakRssBytes > 0;
  return {
    memory: { peak_rss_bytes: measured ? run.peakRssBytes : null, peak_rss_note: run.peakRssNote, limit_bytes: null, limit_applied: false },
    criterion: measured
      ? { status: 'PASS', rule: 'a peak resident set size was read for this converter process' }
      : { status: 'FAIL', reason: run.peakRssNote ?? 'no peak was measured' },
  };
}

/** The first failing criterion names the outcome; every failing one is listed. */
function settle(entry) {
  const failed = Object.entries(entry.criteria)
    .filter(([, criterion]) => failing(criterion.status))
    .map(([name]) => name);
  entry.failed_criteria = failed;
  if (failed.length > 0 && passed(entry.outcome)) entry.outcome = `FAIL_${failed[0]}`;
  return entry;
}

/**
 * One fixture's receipt entry.
 * @param {object} item
 * @param {string} item.only fixture name or TIMEOUT_PROBE
 * @param {object} item.run what lib/runner.mjs reported for the process
 * @param {object|null} item.report the per-process receipt the Rust harness wrote
 * @param {object} [item.source] the fixture's entry in SOURCES.json
 * @param {object} [item.evidence] { markdown, document, pageFiles } re-read from the files the harness wrote
 * @param {object} [item.build] buildFacts() of this run
 */
export function fixtureEntry({ only, run, report, source, evidence, build }) {
  const { memory, criterion: memory_measured } = memoryOf(run);
  const base = only === TIMEOUT_PROBE ? report?.timeout_case : report?.receipts?.[0];
  if (run.exitCode !== 0 || base === null || typeof base !== 'object') {
    return {
      only,
      fixture: only,
      outcome: 'FAIL_harness',
      conversion_outcome: null,
      stage: null,
      status: null,
      finding: null,
      harness_exit_code: run.exitCode,
      harness_signal: run.signal,
      harness_timed_out: run.timedOut,
      harness_error: String(run.spawnError ?? run.stderr ?? '').slice(-2000),
      memory,
      criteria: { conversion: { status: 'FAIL', reason: 'the converter process wrote no receipt' }, memory_measured },
      failed_criteria: ['conversion'],
    };
  }

  const conversion = {
    status: passed(base.outcome) ? 'PASS' : 'FAIL',
    rule: base.conversion_rule ?? null,
    outcome: base.outcome,
  };
  const entry = {
    ...base,
    only,
    role: source?.role ?? (only === TIMEOUT_PROBE ? TIMEOUT_PROBE : null),
    conversion_outcome: base.outcome,
    memory,
    criteria: { conversion },
  };

  if (only === MUST_FAIL && conversion.status === 'PASS') {
    const errorText = (base.errors ?? []).map((item) => item.error_message).join(' | ');
    const kind = classifyRefusal(errorText);
    entry.refusal = {
      stage: base.stage,
      error_text: errorText,
      refusal_kind: kind,
      classified_from: 'the converter error text of this run',
      build_features: build?.build_features ?? null,
      pdfium_compiled_in: build?.pdfium_compiled_in ?? null,
    };
    if (kind === 'primary_parser_failed_no_fallback_in_build') {
      entry.refusal.meaning =
        'docling-pdf\'s pure-Rust parser could not read the file and this build has no pdfium fallback; the library did not report why the file is unreadable';
      if (build?.pdfium_compiled_in !== false) {
        entry.criteria.refusal = {
          status: 'FAIL',
          reason: 'the error says pdfium is not compiled in, but cargo did not resolve this build without pdfium',
        };
      }
    }
  } else if (only !== TIMEOUT_PROBE && conversion.status === 'PASS') {
    const doc = describeDocument(evidence?.document ?? null);
    const facts = bodyFacts(doc);
    const paginated = PAGINATED_FORMATS.includes(base.input_format);
    const images = (base.document?.page_images ?? []).map((image) => ({ ...image, file: evidence?.pageFiles?.[image.file] ?? null }));
    const renders = judgePageRenders({
      applicable: paginated,
      expectedPages: source?.expect?.pages ?? null,
      libraryPageCount: base.library_page_count ?? null,
      pages: doc.pages,
      images,
    });
    const provenance = judgeProvenance(doc, { paginated });
    if (evidence?.problems?.length) entry.criteria.evidence = { status: 'FAIL', problems: evidence.problems };
    entry.criteria.content = judgeContent(source?.expect, {
      markdown: evidence?.markdown ?? '',
      ...facts,
      page_count: paginated ? null : doc.pages.length,
    });
    entry.criteria.page_renders = renders;
    entry.criteria.provenance = provenance;
    entry.structure = {
      tables: facts.tables.length,
      headings: facts.headings,
      pictures: facts.pictures,
      pictures_with_image: facts.pictures_with_image,
      pictures_with_caption: facts.pictures_with_caption,
      sheet_names: facts.sheet_names,
      items_on_other_layers: facts.items_on_other_layers,
    };
    entry.page_image_count = images.length;
    entry.page_provenance = provenance.page_provenance.map((page) => ({
      ...page,
      has_image: images.some((image) => image.page_no === page.page_no),
    }));
  }

  entry.criteria.memory_measured = memory_measured;
  return settle(entry);
}

/**
 * The finished receipt. `result` is PASS only when every fixture's every criterion passes
 * and the models the library resolves are the verified ones.
 */
export function buildDoclingReceipt({
  header,
  converter,
  platform,
  build,
  assets,
  environment,
  sources,
  runs,
  scope,
  paths,
  finishedAt,
}) {
  const entries = runs.map((item) => ({
    only: item.only,
    entry: fixtureEntry({ ...item, source: sources?.files?.[item.only], build }),
  }));

  // A PDF refusal proves nothing when the PDF pipeline itself is not working:
  // docling loads its models before it reads the file, so every PDF then errors.
  const pdfConverted = entries
    .filter(({ only, entry }) => only.endsWith('.pdf') && only !== MUST_FAIL && passed(entry.conversion_outcome))
    .map(({ only }) => only);
  for (const { only, entry } of entries) {
    if (only !== MUST_FAIL || !entry.refusal || !passed(entry.conversion_outcome)) continue;
    entry.refusal.other_pdfs_converted = pdfConverted;
    if (pdfConverted.length === 0) {
      entry.criteria.refusal = {
        status: 'FAIL',
        reason: 'no other PDF fixture converted successfully in this run, so the refusal cannot be attributed to the truncated input',
      };
      entry.refusal_note = entry.criteria.refusal.reason;
      entry.outcome = 'FAIL_refusal_unproven';
      entry.failed_criteria = [...new Set([...entry.failed_criteria, 'refusal'])];
    } else if (!entry.criteria.refusal) {
      entry.criteria.refusal = { status: 'PASS', rule: 'refused by the converter while another PDF converted in the same run' };
    }
  }

  const summary = {};
  const criteria_summary = {};
  const failures = [];
  for (const only of FIXTURE_RUNS) {
    const found = entries.find((item) => item.only === only)?.entry;
    summary[only] = found ? found.outcome : 'FAIL_not_run';
    for (const [name, criterion] of Object.entries(found?.criteria ?? {})) {
      (criteria_summary[name] ??= {})[only] = criterion.status;
      if (failing(criterion.status)) {
        failures.push({
          fixture: only,
          criterion: name,
          status: criterion.status,
          detail: criterion.reason ?? criterion.problems ?? criterion.checks?.filter((check) => !check.ok) ?? criterion.text_items?.invalid ?? null,
        });
      }
    }
  }

  const firstReport = runs.find((item) => Array.isArray(item.report?.model_inventory))?.report;
  const model_inventory = inventoryCheck(firstReport?.model_inventory ?? null, assets?.files ?? []);
  if (failing(model_inventory.status)) {
    failures.push({ fixture: null, criterion: 'model_inventory', status: model_inventory.status, detail: model_inventory.reason ?? model_inventory.unverified });
  }

  // Group the processes by the options their converter held. artifacts_dir is a fresh
  // temporary directory per process, so the whole Debug text is never equal twice.
  const held = entries
    .filter(({ entry }) => typeof entry.settings?.converter_debug === 'string')
    .map(({ only, entry }) => ({ only, options: converterOptions(entry.settings.converter_debug, SETTING_NAMES) }));
  const optionSets = [...new Set(held.map((item) => JSON.stringify(item.options)))];
  const mustFail = entries.find((item) => item.only === MUST_FAIL)?.entry;
  const ok = Object.values(summary).every(passed) && !failing(model_inventory.status);

  return {
    ...header,
    component: 'docling-library-qualification',
    finished_at: finishedAt,
    result: ok ? 'PASS' : 'FAIL',
    finding: mustFail?.finding ?? null,
    scope,
    converter,
    build,
    platform,
    assets_verified: assets ?? null,
    model_inventory,
    settings: {
      source: 'Debug text of the docling::DocumentConverter each converter process built, plus the environment run.mjs gave it; values are raw Rust literals, and None means the harness left the library default',
      converters: optionSets.map((key) => ({
        options: JSON.parse(key),
        used_by: held.filter((item) => JSON.stringify(item.options) === key).map((item) => item.only),
      })),
      environment: environment ?? null,
    },
    memory: {
      limit_bytes: null,
      limit_applied: false,
      statement: MEMORY_STATEMENT,
      measured_by: [...new Set(runs.map((item) => item.run.peakRssNote).filter(Boolean))],
      peak_rss_bytes: Object.fromEntries(entries.map(({ only, entry }) => [only, entry.memory.peak_rss_bytes])),
    },
    match_rules: MATCH_RULES,
    paths,
    summary,
    criteria_summary,
    failures,
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
