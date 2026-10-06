/**
 * Compose the Docling qualification receipt from per-fixture process results.
 * Pure: no I/O; the judging rules live in expect.mjs, document.mjs, build.mjs and assets.mjs,
 * the list of criteria and the envelope in criteria.mjs.
 *
 * The Rust harness observes (stage, status, errors, the Markdown and document export, page
 * images, settings). This module judges every criterion criteria.mjs declares for a fixture
 * and keeps a criterion's evidence beside its result, under `receipts[].criteria.<aspect>`.
 * The same judgements, flattened to { id, required, result, detail }, are the receipt's
 * top-level `criteria`, and `result` is the shared fold of that list: nothing else in this
 * module or in run.mjs decides PASS or FAIL. A fixture's `outcome`, `summary` and
 * `criteria_summary` are derived from the same criteria, so the receipt holds one verdict.
 *
 * What kind of failure is what:
 *   fail          the library did something the rule forbids (see criteria.mjs CRITERION_RULES)
 *   harness_error the environment stopped a judgement: a converter process that did not start,
 *                 exited non-zero, was killed or wrote no receipt; evidence files that are not
 *                 the ones the process recorded; a peak that could not be read; assets or
 *                 models that are not the verified ones. The criteria it touches are
 *                 not_judged and the run is INCOMPLETE unless a required criterion failed.
 *
 * `failures` lists everything that keeps the run from PASS: each required criterion that
 * failed or was not judged, and one entry for a fixture that did not run or whose process
 * failed. Each detail states the total as well as the examples.
 */

import { assetsMatched, inventoryCheck } from './assets.mjs';
import { classifyRefusal } from './build.mjs';
import {
  CRITERION_RULES,
  KINDS,
  MUST_FAIL,
  RUN_CRITERIA,
  TIMEOUT_PROBE,
  contentAspect,
  envelope,
  expectedCriteria,
  fail,
  fixtureAspects,
  fixtureLabel,
  notApplicable,
  notJudged,
  pass,
  stopped,
  unjudged,
} from './criteria.mjs';
import { bodyFacts, converterOptions, describeDocument, judgePageRenders, judgeProvenance } from './document.mjs';
import { MATCH_RULES, judgeContent } from './expect.mjs';

export { MUST_FAIL, TIMEOUT_PROBE };

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

/**
 * Paths whose later change makes a recorded receipt stale: the harness and its tracked
 * criteria.json, the code that makes the header and decides the result, the fixtures, and
 * what the build reads (manifests, lockfile, toolchain, cargo configuration).
 */
export const DOCLING_INPUTS = [
  'qualification/docling',
  'qualification/docling/criteria.json',
  'qualification/lib',
  'scripts/lib/provenance.mjs',
  'scripts/lib/receipt-envelope.mjs',
  'tests/fixtures/documents',
  'Cargo.toml',
  'Cargo.lock',
  'rust-toolchain.toml',
  '.cargo/config.toml',
];

/** A failure detail names at most this many examples after the total. */
const EXAMPLES_SHOWN = 10;
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

/**
 * `; first N: a; b` after a total, or nothing when the list is empty. `total` is how many
 * there are when `items` is itself a capped list; "all" is said only when every one is shown.
 */
function examples(items, total = items.length) {
  if (items.length === 0) return '';
  const shown = items.slice(0, EXAMPLES_SHOWN);
  return `; ${total > shown.length ? `first ${shown.length}` : 'all'}: ${shown.join('; ')}`;
}

/** One line of what a process wrote to stderr, for a sentence. */
const tail = (text) => String(text ?? '').trim().replace(/\s+/g, ' ').slice(-300);

/** The evidence a judge function returned, without the status word the criterion's result replaces. */
function evidenceOf(judged) {
  const rest = { ...judged };
  delete rest.status;
  delete rest.reason;
  return rest;
}

function judgeMemory(only, run) {
  const measured = typeof run.peakRssBytes === 'number' && run.peakRssBytes > 0;
  return {
    memory: { peak_rss_bytes: measured ? run.peakRssBytes : null, peak_rss_note: run.peakRssNote, limit_bytes: null, limit_applied: false },
    judgement: measured
      ? pass({ peak_rss_bytes: run.peakRssBytes })
      : stopped(`the peak memory of the converter process for ${only} was not measured: ${run.peakRssNote ?? 'no peak was read'}`),
  };
}

/** Why the converter process gave nothing to judge, or null when it exited 0 with a receipt. */
function processProblem(run, base) {
  if (run.spawnError) return `could not be started: ${tail(run.spawnError)}`;
  if (run.timedOut) return 'was killed at the harness timeout';
  if (run.exitCode !== 0) {
    const how = run.exitCode === null ? `ended on signal ${run.signal}` : `exited ${run.exitCode}`;
    return `${how}${tail(run.stderr) ? `: ${tail(run.stderr)}` : ''}`;
  }
  if (base === null || typeof base !== 'object') return 'exited 0 without a readable receipt';
  return null;
}

/** Why a criterion that reads the converted files cannot be judged, or null when it can. */
function unreadable(base, criteria) {
  if (criteria.conversion.result !== 'pass') return notJudged(`the conversion did not succeed (${base.outcome}), so there is no converted document to judge`);
  if (criteria.evidence.result !== 'pass') return notJudged('the files the converter process wrote are not the ones it recorded (see evidence), so nothing is judged from them');
  return null;
}

function contentJudgement(expect, observed) {
  const judged = judgeContent(expect, observed);
  if (judged.status === 'PASS') return pass(evidenceOf(judged));
  const missed = judged.checks.filter((check) => !check.ok);
  const named = missed.map((check) =>
    check.kind === 'no_text'
      ? `no_text: ${judged.observed.text_tokens} letter or digit token(s) outside the picture placeholders (${check.observed.slice(0, EXAMPLES_SHOWN).join(' ')})`
      : `${check.kind} ${JSON.stringify(check.expected)}${check.observed === undefined ? '' : ` (observed ${JSON.stringify(check.observed)})`}`,
  );
  const detail = judged.reason ?? `${missed.length} of ${judged.total} expectations are not met${examples(named)}`;
  return fail(detail, evidenceOf(judged));
}

function pageRenderJudgement(input) {
  const judged = judgePageRenders({ applicable: true, ...input });
  if (judged.status === 'PASS') return pass(evidenceOf(judged));
  if (judged.status === 'unavailable') return fail(judged.reason, evidenceOf(judged));
  const over = `${judged.page_image_count} page image(s) for ${judged.page_count} page(s)`;
  if (judged.status === 'not_judged') {
    return notJudged(`the pixels of ${judged.undecoded.length} of ${over} could not be read${examples(judged.undecoded)}`, evidenceOf(judged));
  }
  return fail(`${judged.problems.length} problem(s) over ${over}${examples(judged.problems)}`, evidenceOf(judged));
}

/** "13 of 225 items are not located (13 of 202 text items, ...); first 10: ..." */
function unlocated(judged) {
  const kinds = [['text items', judged.text_items], ['tables', judged.tables], ['pictures', judged.pictures]];
  const lost = judged.items.total - judged.items.located;
  const counts = kinds.map(([name, coverage]) => `${coverage.invalid_total} of ${coverage.total} ${name}`).join(', ');
  const named = kinds.flatMap(([, coverage]) => coverage.invalid.map((item) => `${item.ref} ${item.label}: ${item.problem}`));
  return `${lost} of ${judged.items.total} items are not located (${counts})${examples(named, lost)}`;
}

function provenanceJudgement(kind, doc, blocked) {
  if (kind.provenance === 'judged') {
    if (blocked) return blocked;
    const judged = judgeProvenance(doc, { paginated: true });
    if (judged.status === 'PASS') return pass(evidenceOf(judged));
    if (judged.status === 'FAIL') return fail(unlocated(judged), evidenceOf(judged));
    return notJudged(judged.reason, evidenceOf(judged));
  }
  // Not a PDF or an image: what the library gives is recorded as observed, when there is a document to read.
  const observed = blocked ? {} : evidenceOf(judgeProvenance(doc, { paginated: false }));
  const located = observed.items ? `${observed.items.located} of ${observed.items.total} items located, locator ${observed.locator}` : 'no document was read';
  if (kind.provenance === 'none') {
    if ((observed.items?.located ?? 0) > 0) {
      return notJudged(`the library located ${observed.items.located} of ${observed.items.total} DOCX items; this harness has no rule for a DOCX locator`, observed);
    }
    return notApplicable('the library gives DOCX items no page or box', observed);
  }
  return notJudged(`not a PDF or image fixture: the locator the library gives is recorded as observed and no rule is applied (${located})`, observed);
}

/** Whether the files a converter process wrote are the ones it recorded. */
function evidenceJudgement(only, base, evidence) {
  if (base.document === null || typeof base.document !== 'object') {
    return notJudged('the converter returned no document, so the process wrote no file to read');
  }
  if (!Array.isArray(evidence?.problems)) return stopped(`the files the converter process for ${only} wrote were not re-read`);
  if (evidence.problems.length > 0) {
    return stopped(`${evidence.problems.length} file(s) the converter process for ${only} wrote are not the ones it recorded${examples(evidence.problems)}`, {
      problems: evidence.problems,
    });
  }
  return pass();
}

/** Pages and located-or-not items of a converted document: what a spent budget can reduce. */
function extent(document) {
  const doc = describeDocument(document);
  return { pages: doc.pages.length, items: doc.texts.length + doc.tables.length + doc.pictures.length };
}

/**
 * Did the 1 ms budget change the result? Compared with the conversion of the same bytes
 * without a budget in the same run. Elapsed time is not judged: the library gives no bound
 * to hold it to.
 * @param {object} criteria the probe's criteria judged so far
 * @param {object|null} full { fixture, converted, document } of the run that converted the same bytes
 */
function budgetJudgement(criteria, base, evidence, full) {
  if (criteria.timeout_reported.result !== 'pass') return notJudged('the converter did not report a spent budget, so there is no effect to compare');
  if (base.document === null || typeof base.document !== 'object') {
    return notJudged('the library returned no document for the probe, so nothing distinguishes it from the full conversion');
  }
  if (criteria.evidence.result !== 'pass') return notJudged('the files the probe process wrote are not the ones it recorded (see evidence), so nothing is judged from them');
  if (!full) return notJudged('no fixture of this run converted the same bytes without a budget, so there is nothing to compare the probe with');
  if (!full.converted) return notJudged(`the full conversion of ${full.fixture} did not succeed in this run, so there is nothing to compare the probe with`);
  const probe = extent(evidence.document);
  const whole = extent(full.document);
  const compared = { probe, full: { fixture: full.fixture, ...whole } };
  if (probe.pages < whole.pages || probe.items < whole.items) {
    return pass({
      detail: `the probe returned ${probe.pages} of ${whole.pages} page(s) and ${probe.items} of ${whole.items} item(s) of the full conversion of ${full.fixture}`,
      ...compared,
    });
  }
  return fail(
    `the probe reported a spent budget and still returned ${probe.pages} page(s) and ${probe.items} item(s), no fewer than the full conversion of ${full.fixture} (${whole.pages} page(s), ${whole.items} item(s)): the budget had no visible effect`,
    compared,
  );
}

/** The must-fail refusal as observed, and whether it can be attributed to the truncated input. */
function refusalOf(base, build, otherPdfsConverted) {
  const errorText = (base.errors ?? []).map((item) => item.error_message).join(' | ');
  const kind = classifyRefusal(errorText);
  const refusal = {
    stage: base.stage,
    error_text: errorText,
    refusal_kind: kind,
    classified_from: 'the converter error text of this run',
    build_features: build?.build_features ?? null,
    pdfium_compiled_in: build?.pdfium_compiled_in ?? null,
    other_pdfs_converted: otherPdfsConverted,
  };
  let judgement = pass();
  if (kind === 'primary_parser_failed_no_fallback_in_build') {
    refusal.meaning =
      'docling-pdf\'s pure-Rust parser could not read the file and this build has no pdfium fallback; the library did not report why the file is unreadable';
    if (build?.pdfium_compiled_in !== false) {
      judgement = fail('the error says pdfium is not compiled in, but cargo did not resolve this build without pdfium');
    }
  }
  if (judgement.result === 'pass' && otherPdfsConverted.length === 0) {
    judgement = notJudged('no other PDF fixture converted successfully in this run, so the refusal cannot be attributed to the truncated input');
  }
  return { refusal, judgement };
}

/**
 * One fixture's receipt entry.
 * @param {object} item
 * @param {string} item.only fixture name or TIMEOUT_PROBE
 * @param {object} item.run what lib/runner.mjs reported for the process
 * @param {object|null} item.report the per-process receipt the Rust harness wrote
 * @param {object} [item.source] the fixture's entry in SOURCES.json
 * @param {object} [item.evidence] { markdown, document, pageFiles, problems } re-read from the files the harness wrote
 * @param {object} [item.build] buildFacts() of this run
 * @param {string[]} [item.otherPdfsConverted] the other PDF fixtures that converted in this run
 * @param {object|null} [item.full] for the timeout probe: the run that converted the same bytes without a budget
 */
export function fixtureEntry({ only, run, report, source, evidence, build, otherPdfsConverted = [], full = null }) {
  const list = fixtureAspects(only, source);
  const { memory, judgement: memoryJudgement } = judgeMemory(only, run);
  const base = only === TIMEOUT_PROBE ? report?.timeout_case : report?.receipts?.[0];
  const role = source?.role ?? (only === TIMEOUT_PROBE ? TIMEOUT_PROBE : null);
  const finish = (entry) => {
    entry.outcome = fixtureLabel(only, list, entry.criteria);
    entry.failed_criteria = list.filter(({ aspect }) => entry.criteria[aspect].result === 'fail').map(({ aspect }) => aspect);
    entry.not_judged = list.filter(({ aspect }) => unjudged(entry.criteria[aspect].result)).map(({ aspect }) => aspect);
    return entry;
  };

  const stop = processProblem(run, base);
  if (stop) {
    const sentence = `the converter process for ${only} ${stop}`;
    return finish({
      only,
      fixture: only,
      role,
      conversion_outcome: null,
      stage: null,
      status: null,
      finding: null,
      process_failed: true,
      harness_error: sentence,
      harness_exit_code: run.exitCode,
      harness_signal: run.signal,
      harness_timed_out: run.timedOut,
      harness_stderr: String(run.spawnError ?? run.stderr ?? '').slice(-2000),
      memory,
      criteria: Object.fromEntries(list.map(({ aspect }) => [aspect, stopped(sentence)])),
    });
  }

  const entry = { ...base, only, role, conversion_outcome: base.outcome, memory, criteria: {} };
  const criteria = entry.criteria;
  const observedOutcome = { rule: base.conversion_rule ?? null, observed: base.outcome };
  const errors = (base.errors ?? []).map((item) => item.error_message).join(' | ');

  if (only === TIMEOUT_PROBE) {
    criteria.timeout_reported =
      base.outcome === 'PASS'
        ? pass(observedOutcome)
        : fail(`the converter did not report the spent budget as documented: ${base.outcome}, status ${base.status ?? 'none'}${errors ? `, errors: ${errors}` : ', no error item'}`, observedOutcome);
    criteria.evidence = evidenceJudgement(only, base, evidence);
    criteria.budget_had_effect = budgetJudgement(criteria, base, evidence, full);
  } else if (source?.role === 'must_fail') {
    criteria.refused =
      base.outcome === 'PASS_explicit_failure'
        ? pass(observedOutcome)
        : fail(`the converter did not refuse the truncated input: ${base.outcome}${base.finding ? ` (${base.finding})` : ''}`, observedOutcome);
    if (criteria.refused.result === 'pass') {
      const { refusal, judgement } = refusalOf(base, build, otherPdfsConverted);
      entry.refusal = refusal;
      criteria.refusal_attributed = judgement;
    } else {
      criteria.refusal_attributed = notJudged('the converter did not refuse the input, so there is no refusal to attribute');
    }
  } else {
    const kind = KINDS[source.kind];
    criteria.conversion =
      base.outcome === 'PASS'
        ? pass(observedOutcome)
        : fail(`the converter did not convert the fixture: ${base.outcome}${errors ? `: ${errors}` : ''}`, observedOutcome);
    criteria.evidence = evidenceJudgement(only, base, evidence);
    criteria.format_recognised =
      base.input_format === kind.format
        ? pass({ declared_kind: source.kind, reported_format: base.input_format })
        : fail(`the library reports format ${JSON.stringify(base.input_format ?? null)} for a fixture SOURCES.json declares ${source.kind} (format "${kind.format}")`, {
            declared_kind: source.kind,
            reported_format: base.input_format ?? null,
          });

    const blocked = unreadable(base, criteria);
    const doc = describeDocument(blocked ? null : evidence.document);
    const facts = bodyFacts(doc);
    const images = (base.document?.page_images ?? []).map((image) => ({ ...image, file: evidence?.pageFiles?.[image.file] ?? null }));
    criteria[contentAspect(source)] =
      blocked ??
      contentJudgement(source.expect, { markdown: evidence.markdown ?? '', ...facts, page_count: kind.page_renders ? null : doc.pages.length });
    if (!kind.page_renders) {
      criteria.page_renders = notApplicable('the library keeps page images for the PDF/image pipeline only (docling converter.rs:756 generate_page_images)', {
        page_image_count: images.length,
      });
    } else {
      criteria.page_renders =
        blocked ??
        pageRenderJudgement({
          expectedPages: source.expect?.pages ?? null,
          libraryPageCount: base.library_page_count ?? null,
          pages: doc.pages,
          images,
          blankPages: source.expect?.blank_pages ?? [],
        });
    }
    criteria.provenance = provenanceJudgement(kind, doc, blocked);
    if (list.some(({ aspect }) => aspect === 'text_provenance')) {
      criteria.text_provenance = notApplicable(
        'the fixture shows no glyphs, so the converter is to produce no text item and there is none whose location could be judged; its pictures and tables are located by provenance',
      );
    }
    if (!blocked) {
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
      entry.page_provenance = (criteria.provenance.page_provenance ?? []).map((page) => ({
        ...page,
        has_image: images.some((image) => image.page_no === page.page_no),
      }));
    }
  }

  criteria.memory_measured = memoryJudgement;
  return finish(entry);
}

/** The PDF fixtures, other than the must-fail one, whose process ran and whose conversion the library reported as a success. */
function convertedPdfs(runs, sources) {
  return runs
    .filter((item) => item.only !== TIMEOUT_PROBE && sources?.files?.[item.only]?.kind === 'pdf' && sources.files[item.only].role !== 'must_fail')
    .filter((item) => processProblem(item.run, item.report?.receipts?.[0]) === null && item.report.receipts[0].outcome === 'PASS')
    .map((item) => item.only);
}

/**
 * The run that converted the timeout probe's bytes without a budget: the fixture whose
 * process recorded the same input hash. Null when no run of this receipt did.
 */
function fullConversionOf(probe, runs) {
  const hash = probe?.report?.timeout_case?.sha256_before;
  const match = typeof hash === 'string' ? runs.find((item) => item.only !== TIMEOUT_PROBE && item.report?.receipts?.[0]?.sha256_before === hash) : null;
  if (!match) return null;
  const base = match.report.receipts[0];
  const converted = processProblem(match.run, base) === null && base.outcome === 'PASS' && Array.isArray(match.evidence?.problems) && match.evidence.problems.length === 0;
  return { fixture: match.only, converted, document: converted ? match.evidence.document : null };
}

function assetsJudgement(assets) {
  if (!assets || !Array.isArray(assets.files)) return notJudged('no asset was hashed in this run');
  const counted = assetsMatched(assets.files);
  if (counted.count > 0 && counted.matched === counted.count) return pass(counted);
  return stopped(
    counted.count === 0
      ? 'the asset manifest lists no file that was hashed'
      : `${counted.count - counted.matched} of ${counted.count} model assets do not have the length and hash the manifest gives${examples(counted.unmatched)}`,
    counted,
  );
}

function inventoryJudgement(checked) {
  if (checked.status === 'PASS') return pass();
  // No inventory at all means no process got far enough to report one; those processes carry the harness error.
  if (checked.reason) return notJudged(checked.reason);
  return stopped(
    `${checked.unverified.length} of ${checked.entries.filter((entry) => entry.stage !== 'pdfium').length} models the library resolves are not verified files${examples(checked.unverified)}`,
  );
}

/**
 * The finished receipt. `result` comes from criteria.mjs `envelope`, which folds the
 * criteria with scripts/lib/receipt-envelope.mjs `foldCriteria`; nothing here sets it.
 * @param {object} input
 * @param {string|null} [input.harnessError] what stopped the run before any fixture was converted
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
  harnessError = null,
}) {
  const converted = convertedPdfs(runs, sources);
  const entries = runs.map((item) => ({
    only: item.only,
    entry: fixtureEntry({
      ...item,
      source: sources?.files?.[item.only],
      build,
      otherPdfsConverted: converted.filter((name) => name !== item.only),
      full: item.only === TIMEOUT_PROBE ? fullConversionOf(item, runs) : null,
    }),
  }));
  const entryOf = (only) => entries.find((item) => item.only === only)?.entry ?? null;

  const firstReport = runs.find((item) => Array.isArray(item.report?.model_inventory))?.report;
  const inventory = inventoryCheck(firstReport?.model_inventory ?? null, assets?.files ?? []);

  const notRun = harnessError ? `the fixture did not run: ${harnessError}` : 'the fixture did not run';
  const judgements = new Map();
  judgements.set('assets/hashes_match', harnessError && !assets ? notJudged(`not reached: ${harnessError}`) : assetsJudgement(assets));
  judgements.set(
    'assets/model_inventory',
    runs.length === 0 ? notJudged(harnessError ? `not reached: ${harnessError}` : 'no converter process ran') : inventoryJudgement(inventory),
  );
  // Every fixture run SOURCES.json declares, in execution order. Without readable declarations only the probe is known.
  const names = FIXTURE_RUNS.filter((only) => only === TIMEOUT_PROBE || sources?.files?.[only]);
  const labels = {};
  for (const only of names) {
    const list = fixtureAspects(only, sources?.files?.[only]);
    const judged = entryOf(only)?.criteria ?? Object.fromEntries(list.map(({ aspect }) => [aspect, notJudged(notRun)]));
    for (const { aspect } of list) judgements.set(`${only}/${aspect}`, judged[aspect]);
    labels[only] = entryOf(only)?.outcome ?? fixtureLabel(only, list, judged);
  }
  const top = envelope({ expected: expectedCriteria(sources), judgements, harnessError });

  const criteria_summary = {};
  const failures = [];
  for (const id of RUN_CRITERIA) {
    const criterion = top.criteria.find((item) => item.id === id);
    if (criterion.result !== 'pass') failures.push({ fixture: null, criterion: id, result: criterion.result, detail: criterion.detail ?? null });
  }
  for (const only of names) {
    const found = entryOf(only);
    for (const { aspect, required } of fixtureAspects(only, sources?.files?.[only])) {
      const judged = judgements.get(`${only}/${aspect}`);
      (criteria_summary[aspect] ??= {})[only] = judged.result;
      if (found && !found.process_failed && required && (judged.result === 'fail' || judged.result === 'not_judged')) {
        failures.push({ fixture: only, criterion: aspect, result: judged.result, detail: judged.detail ?? null });
      }
    }
    if (!found) failures.push({ fixture: only, criterion: null, result: 'not_judged', detail: notRun });
    else if (found.process_failed) failures.push({ fixture: only, criterion: null, result: 'not_judged', detail: found.harness_error });
  }

  // Group the processes by the options their converter held. artifacts_dir is a fresh
  // temporary directory per process, so the whole Debug text is never equal twice.
  const held = entries
    .filter(({ entry }) => typeof entry.settings?.converter_debug === 'string')
    .map(({ only, entry }) => ({ only, options: converterOptions(entry.settings.converter_debug, SETTING_NAMES) }));
  const optionSets = [...new Set(held.map((item) => JSON.stringify(item.options)))];
  const inventoryRecord = evidenceOf(inventory);

  return {
    ...header,
    ...top,
    component: 'docling-library-qualification',
    finished_at: finishedAt,
    finding: entryOf(MUST_FAIL)?.finding ?? null,
    scope,
    converter: converter ?? null,
    build: build ?? null,
    platform,
    assets_verified: assets ?? null,
    model_inventory: { result: judgements.get('assets/model_inventory').result, ...inventoryRecord },
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
    criterion_rules: CRITERION_RULES,
    match_rules: MATCH_RULES,
    paths,
    summary: labels,
    criteria_summary,
    failures,
    receipts: entries.filter((item) => item.only !== TIMEOUT_PROBE).map((item) => item.entry),
    timeout_case: entryOf(TIMEOUT_PROBE),
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
