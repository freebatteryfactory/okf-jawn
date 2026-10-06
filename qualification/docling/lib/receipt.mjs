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
 *   fail          the library did something the rule forbids (see criteria.mjs CRITERION_RULES).
 *                 A converter process that dies on a fixture is one of them: a panic, an abort
 *                 or a fault inside the conversion is the library failing on that fixture, so
 *                 it fails the fixture's conversion criterion (`refused` for the must-fail
 *                 fixture, `timeout_reported` for the probe), with the exit code and the last
 *                 lines of stderr in the detail (see `processEnd`).
 *   harness_error the environment stopped a judgement: a converter process that did not start,
 *                 was killed at the harness timeout or from outside, refused the run itself
 *                 (HARNESS_EXIT) or wrote no receipt; evidence files that are not the ones the
 *                 process recorded; a peak that could not be read; assets or models that are
 *                 not the verified ones. The criteria it touches are not_judged and the run is
 *                 INCOMPLETE unless a required criterion failed.
 *
 * `failures` lists everything that keeps the run from PASS: each required criterion that
 * failed or was not judged, and one entry for a fixture that did not run or whose process the
 * environment stopped. Each detail states the total as well as the examples.
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
  splitExpect,
  stopped,
  unjudged,
} from './criteria.mjs';
import { LOCATION_SOURCES, bodyFacts, converterOptions, describeDocument, judgePageRenders, judgeProvenance } from './document.mjs';
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
/**
 * The exit code src/main.rs ends with when the harness binary itself could not make the run
 * (EXIT_HARNESS there; a test holds the two equal): a variable that is not set, a fixture file
 * that is missing, an output directory that cannot be written.
 */
export const HARNESS_EXIT = 64;
/** The signals a process raises against itself when it dies; any other signal was sent to it from outside. */
const CRASH_SIGNALS = Object.freeze(['SIGABRT', 'SIGSEGV', 'SIGBUS', 'SIGILL', 'SIGFPE', 'SIGTRAP']);
/** How many of the last lines of stderr the detail of a crash carries. */
const STDERR_LINES = 5;
/**
 * The criterion of each kind of fixture run that a dying converter process fails, with what the
 * process did instead of what the criterion asks. Every run has exactly one of them.
 */
const CRASH_FAILS = Object.freeze({
  conversion: 'while converting the fixture',
  refused: 'instead of refusing the truncated input',
  timeout_reported: 'instead of reporting the spent budget',
});
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

/** The last lines a process wrote to stderr, each trimmed, joined for one sentence. */
function lastLines(text) {
  const lines = String(text ?? '').split(/\r?\n/).map((line) => line.trim()).filter(Boolean).slice(-STDERR_LINES);
  return lines.length ? lines.join(' | ').slice(-800) : '(it wrote nothing to stderr)';
}

/**
 * How a converter process that gave nothing to judge ended: `{ kind, how }`, or null when it
 * exited 0 with a receipt.
 *
 * kind `crash`: the process died on the fixture. It exited non-zero with a code the harness
 * binary does not use for its own refusals (a Rust panic exits 101; an abort or a fault exits
 * with the platform's code), or it died on a signal a process raises against itself. The
 * binary's own code neither panics nor exits, so this is the library failing on the fixture,
 * and it is judged as a `fail`. A process that ran out of memory dies the same way and is
 * recorded the same way; the detail carries the exit code.
 *
 * kind `harness`: the environment. The process could not be started (the binary is missing),
 * was killed at the harness timeout, was ended by a signal sent from outside, refused the run
 * itself (HARNESS_EXIT), or exited 0 without a readable receipt.
 */
function processEnd(run, base) {
  const harness = (how) => ({ kind: 'harness', how });
  const said = tail(run.stderr) ? `: ${tail(run.stderr)}` : '';
  if (run.spawnError) return harness(`could not be started: ${tail(run.spawnError)}`);
  if (run.timedOut) return harness('was killed at the harness timeout');
  if (run.exitCode === HARNESS_EXIT) return harness(`could not make the run (exit ${HARNESS_EXIT}, the harness binary's own refusal)${said}`);
  if (run.exitCode === null) {
    if (CRASH_SIGNALS.includes(run.signal)) return { kind: 'crash', how: `died on signal ${run.signal}` };
    return harness(`ended on signal ${run.signal}, sent from outside the process${said}`);
  }
  if (run.exitCode !== 0) return { kind: 'crash', how: `exited ${run.exitCode}` };
  if (base === null || typeof base !== 'object') return harness('exited 0 without a readable receipt');
  return null;
}

/** True when the converter process exited 0 with a receipt, so there is something to judge. */
const processFinished = (run, base) => processEnd(run, base) === null;

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
  // The observed side of each expectation that is not met: a count, the tokens read, or, for a
  // string that is not in the Markdown, the closest line of it (expect.mjs closestLine).
  const observedBeside = (check) => {
    if (check.kind === 'markdown_contains') {
      return check.closest
        ? ` (closest line of the output: ${JSON.stringify(check.closest.line)}, which holds ${check.closest.shared} of its ${check.closest.of} words)`
        : ' (no line of the output holds half of its words)';
    }
    return check.observed === undefined ? '' : ` (observed ${JSON.stringify(check.observed)})`;
  };
  const named = missed.map((check) =>
    check.kind === 'no_text'
      ? `no_text: ${judged.observed.text_tokens} letter or digit token(s) outside the picture placeholders (${check.observed.slice(0, EXAMPLES_SHOWN).join(' ')})`
      : `${check.kind} ${JSON.stringify(check.expected)}${observedBeside(check)}`,
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

/**
 * The three counts, always: "225 items: 212 located by the export (189 text items, 6 tables,
 * 17 pictures), 13 through the text layer (13 text items, 0 tables, 0 pictures), 0 not
 * located (0 text items, 0 tables, 0 pictures)", then the items that are not located.
 */
function locationCounts(judged) {
  const kinds = [['text items', judged.text_items], ['tables', judged.tables], ['pictures', judged.pictures]];
  const words = { export: 'located by the export', text_layer: 'through the text layer', none: 'not located' };
  const counts = LOCATION_SOURCES.map(
    (source) => `${judged.items.located_by[source]} ${words[source]} (${kinds.map(([name, coverage]) => `${coverage.located_by[source]} ${name}`).join(', ')})`,
  );
  const named = kinds.flatMap(([, coverage]) => coverage.invalid.map((item) => `${item.ref} ${item.label}: ${item.problem}`));
  return `${judged.items.total} items: ${counts.join(', ')}${examples(named, judged.items.located_by.none)}`;
}

/**
 * What the converter process recorded about the items of a PDF (src/locate.rs), by ref, or
 * the sentence that says why it cannot be used: the record is missing, or it is not the
 * record of the document the process wrote.
 */
function recordedLookups(only, base, doc) {
  const recorded = base.locations?.items;
  if (!Array.isArray(recorded)) return { problem: `the converter process for ${only} recorded no item locations for a PDF` };
  const refs = [...doc.texts, ...doc.tables, ...doc.pictures].map((item) => item.ref);
  const lookups = new Map(recorded.map((entry) => [entry.item, entry]));
  if (lookups.size !== recorded.length || recorded.length !== refs.length || !refs.every((ref) => lookups.has(ref))) {
    return { problem: `the item locations the converter process for ${only} recorded (${recorded.length}) are not those of the ${refs.length} items of the document it wrote` };
  }
  return { lookups };
}

function provenanceJudgement(only, kind, base, doc, blocked) {
  if (kind.provenance === 'judged') {
    if (blocked) return blocked;
    // Only a PDF has a text layer to look an unlocated item up in; an image is judged by its export alone.
    const { lookups = null, problem = null } = kind.text_layer ? recordedLookups(only, base, doc) : {};
    if (problem) return stopped(problem);
    const judged = judgeProvenance(doc, { paginated: true, lookups });
    if (judged.status === 'PASS') return pass({ detail: locationCounts(judged), ...evidenceOf(judged) });
    if (judged.status === 'FAIL') return fail(locationCounts(judged), evidenceOf(judged));
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

/** Page numbers as numbers, ascending. */
const pageNumbers = (pages) => Object.keys(pages).map(Number).sort((a, b) => a - b);

/** "page 3 (268), page 5 (3)" */
const pageCounts = (pages) => pageNumbers(pages).map((page) => `page ${page} (${pages[page]})`).join(', ');

/**
 * Do the placeholders the converter process found (src/glyphs.rs) stand on the pages, and in
 * the numbers, the fixture declares? `declared` is `expect.undecoded_glyphs` of SOURCES.json,
 * absent for a fixture whose fonts all map to Unicode: such a fixture must show none.
 */
function glyphJudgement(only, base, declared, blocked) {
  if (blocked) return blocked;
  const recorded = base.undecoded_glyphs;
  const counts = recorded?.pages;
  const whole = (value) => Number.isInteger(value) && value >= 0;
  if (counts === null || typeof counts !== 'object' || Array.isArray(counts) || !Object.values(counts).every(whole) || !whole(recorded.unlocated_tokens)) {
    return stopped(`the converter process for ${only} recorded no count of glyph-name placeholders for a PDF`);
  }
  const positive = (pages) => Object.fromEntries(pageNumbers(pages).filter((page) => pages[page] > 0).map((page) => [page, pages[page]]));
  const expected = positive(declared?.pages ?? {});
  const reported = positive(counts);
  const differing = [...new Set([...pageNumbers(expected), ...pageNumbers(reported)])]
    .sort((a, b) => a - b)
    .filter((page) => (expected[page] ?? 0) !== (reported[page] ?? 0))
    .map((page) => `page ${page}: ${reported[page] ?? 0} reported, ${expected[page] ?? 0} declared`);
  if (recorded.unlocated_tokens > 0) differing.push(`${recorded.unlocated_tokens} reported in items that are on no page`);
  const total = (pages) => Object.values(pages).reduce((sum, count) => sum + count, 0);
  const evidence = {
    declared_pages: expected,
    reported_pages: reported,
    declared_total: total(expected),
    reported_total: total(reported) + recorded.unlocated_tokens,
    unlocated_tokens: recorded.unlocated_tokens,
    confirmed_by: declared?.confirmed_by ?? null,
  };
  if (differing.length > 0) {
    return fail(`the glyph-name placeholders reported differ from the ones SOURCES.json declares in ${differing.length} place(s)${examples(differing)}`, evidence);
  }
  const detail = evidence.declared_total === 0
    ? 'no glyph-name placeholder is reported and SOURCES.json declares none: every glyph of the text layer has Unicode'
    : `${evidence.reported_total} glyph-name placeholder(s) reported, as SOURCES.json declares: ${pageCounts(reported)}; the text of those glyphs is not in the file and is not recovered`;
  return pass({ detail, ...evidence });
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

  const end = processEnd(run, base);
  if (end) {
    const sentence = `the converter process for ${only} ${end.how}`;
    const crashed = end.kind === 'crash';
    const observed = {
      only,
      fixture: only,
      role,
      conversion_outcome: null,
      stage: null,
      status: null,
      finding: null,
      process_failed: !crashed,
      process_crashed: crashed,
      harness_error: crashed ? null : sentence,
      harness_exit_code: run.exitCode,
      harness_signal: run.signal,
      harness_timed_out: run.timedOut,
      harness_stderr: String(run.spawnError ?? run.stderr ?? '').slice(-2000),
      memory,
    };
    if (!crashed) return finish({ ...observed, criteria: Object.fromEntries(list.map(({ aspect }) => [aspect, stopped(sentence)])) });
    // The library died on the fixture: the criterion that says the converter handled it fails,
    // and nothing else of the fixture can be judged. No harness error: the environment did its part.
    const handled = list.map(({ aspect }) => aspect).find((aspect) => Object.hasOwn(CRASH_FAILS, aspect));
    const judged = Object.fromEntries(list.map(({ aspect }) => [aspect, notJudged(`${sentence}, so there is nothing of the fixture to judge`)]));
    judged[handled] = fail(`${sentence} ${CRASH_FAILS[handled]}; the last lines of its stderr: ${lastLines(run.stderr)}`, { exit_code: run.exitCode, signal: run.signal });
    if (memoryJudgement.result === 'pass') judged.memory_measured = memoryJudgement;
    return finish({ ...observed, criteria: judged });
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
    // One comparison, two criteria: the expectations that cross a font change are judged apart.
    const expect = splitExpect(source);
    const observed = { markdown: evidence?.markdown ?? '', ...facts, page_count: kind.page_renders ? null : doc.pages.length };
    criteria[contentAspect(source)] = blocked ?? contentJudgement(expect.plain, observed);
    if (expect.font_runs) criteria.content_across_font_runs = blocked ?? contentJudgement(expect.font_runs, observed);
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
    criteria.provenance = provenanceJudgement(only, kind, base, doc, blocked);
    // The items are listed once, judged, under criteria.provenance.located_items.
    if (base.locations) entry.locations = { ...base.locations, items: `${base.locations.items?.length ?? 0} recorded; judged under criteria.provenance.located_items` };
    if (list.some(({ aspect }) => aspect === 'text_provenance')) {
      criteria.text_provenance = notApplicable(
        'the fixture shows no glyphs, so the converter is to produce no text item and there is none whose location could be judged; its pictures and tables are located by provenance',
      );
    }
    if (kind.text_layer) criteria.undecoded_glyphs_reported = glyphJudgement(only, base, source.expect?.undecoded_glyphs, blocked);
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
    .filter((item) => processFinished(item.run, item.report?.receipts?.[0]) && item.report.receipts[0].outcome === 'PASS')
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
  const converted = processFinished(match.run, base) && base.outcome === 'PASS' && Array.isArray(match.evidence?.problems) && match.evidence.problems.length === 0;
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
