/**
 * The criteria the Docling qualification judges, and the envelope that carries them.
 * Pure: no I/O.
 *
 * A harness judges criteria; it never types its own result. Every id this harness can emit is
 * derived here from tests/fixtures/documents/SOURCES.json plus the run-level list, without
 * converting anything, so a test can compare the required ids with the tracked criteria.json
 * and a dropped check is a visible change to that file. `envelope` is the only place the
 * receipt's `result` is set, and it sets it with the shared `foldCriteria`.
 *
 * Vocabulary (scripts/lib/receipt-envelope.mjs):
 *   pass            the rule in CRITERION_RULES holds
 *   fail            a statement about the library under test
 *   not_judged      no verdict was reached; `detail` says why. With `harness_error` the cause
 *                   is the environment (a process that did not start or was killed from
 *                   outside, a file that changed on disk, a peak that could not be read),
 *                   which makes the run INCOMPLETE. A converter process that dies on a
 *                   fixture is not the environment: it is a `fail` of that fixture
 *   not_applicable  the library cannot do this for that input by design; `detail` says so
 * `required: false` marks a measurement recorded without a verdict: the locator the library
 * gives a spreadsheet or a presentation, which this harness has no rule for.
 */

import { foldCriteria } from '../../../scripts/lib/receipt-envelope.mjs';

/**
 * Process exit code of run.mjs per receipt result, shared by every harness: 0 PASS, 2 FAIL,
 * 3 INCOMPLETE. 1 is left to a run that wrote no receipt.
 */
export { EXIT_CODES, exitCodeFor } from '../../../scripts/lib/receipt-envelope.mjs';
import { PAGE_RENDER_RULE, PROVENANCE_RULE } from './document.mjs';

/** The verification.json gate this receipt is evidence for. */
export const GATE = 'docling-library-qualification';

export const MUST_FAIL = 'must_fail_truncated.pdf';
export const TIMEOUT_PROBE = 'timeout_probe';

/**
 * What a fixture's declared kind (SOURCES.json `kind`) means for judging. `format` is the
 * name docling's InputFormat::as_str gives that kind (docling format.rs:118).
 * `provenance`: judged (every item needs a page and a box), none (the library gives the
 * format no locator), recorded (the library's locator is recorded and no rule is applied).
 * `text_layer`: the file has a text layer the library reads on its own
 * (docling::pdf_text_layer_pages), so the converter process records, for each item, the
 * source that located it (src/locate.rs), and the placeholders the library printed for
 * glyphs its fonts give no Unicode for (src/glyphs.rs), which are judged against the
 * fixture's declaration (undecoded_glyphs_reported).
 */
export const KINDS = Object.freeze({
  pdf: Object.freeze({ format: 'pdf', page_renders: true, provenance: 'judged', text_layer: true }),
  image: Object.freeze({ format: 'image', page_renders: true, provenance: 'judged' }),
  docx: Object.freeze({ format: 'docx', page_renders: false, provenance: 'none' }),
  xlsx: Object.freeze({ format: 'xlsx', page_renders: false, provenance: 'recorded' }),
  pptx: Object.freeze({ format: 'pptx', page_renders: false, provenance: 'recorded' }),
});

/** Criteria of the run as a whole. */
export const RUN_CRITERIA = Object.freeze(['assets/hashes_match', 'assets/model_inventory']);

/** What a passing criterion asserts, by run-level id or by fixture aspect; the receipt carries these sentences. */
export const CRITERION_RULES = Object.freeze({
  'assets/hashes_match':
    'every asset the manifest lists was read again in this run and has the length and the SHA-256 the manifest gives',
  'assets/model_inventory':
    'every model file docling::model_inventory() reports in a converter process of this run is one of the re-hashed assets, by path and length',
  conversion:
    'DocumentConverter::convert returned Success with Markdown that is not blank, and the fixture bytes are unchanged afterwards; a converter process that dies on the fixture (a panic, an abort, a fault: any exit the harness binary did not make itself) fails this criterion, with its exit code and the last lines of its stderr',
  evidence:
    'the Markdown, the document export, every page image and, for a PDF, the text-layer document the converter process wrote are on disk with the hashes it recorded',
  format_recognised:
    'the format the library reports for the file is the format of the kind SOURCES.json declares for the fixture; what is judged for a fixture follows the declared kind, never the reported format',
  content:
    'every expectation the fixture declares in SOURCES.json holds for the converted document; match_rules says how each kind of expectation is compared',
  content_across_font_runs:
    'every expectation SOURCES.json marks as crossing a font change (crosses_font_runs) holds for the converted document, by the same comparison as content and with nothing forgiven; kept apart from content because the published docling-pdf 1.93.6 ends a text cell at every font change and joins cells with a space (dp_lines.rs:227-229, assemble.rs:2084), so a phrase set in more than one font is extracted with spaces the page does not show; the docling-pdf this build uses is the one-commit fork that keeps such a phrase in one cell (verification.json gate converter-docling-pdf-font-run-patch)',
  no_invented_text:
    'the converter invents no text for a page without glyphs: the fixture shows none, so the Markdown with HTML comments (the picture placeholders) removed holds no letter and no digit; every other expectation the fixture declares holds as well',
  page_renders: PAGE_RENDER_RULE,
  provenance: PROVENANCE_RULE,
  text_provenance:
    'not applicable by design: the fixture shows no glyphs, so there is no text item whose location could be judged; its pictures and tables are located by provenance and any text the converter produces fails no_invented_text',
  undecoded_glyphs_reported:
    'the placeholders the library prints for glyphs a font gives no Unicode for (`/` and a glyph name; the detector and its rule are src/glyphs.rs, restated from docling-pdf textparse.rs is_gid_name) are found on exactly the pages, and in exactly the numbers, the fixture declares in SOURCES.json (expect.undecoded_glyphs.pages, counted from the file itself); a PDF fixture that declares none must show none. A pass says the text that cannot be decoded is detected and where; it does not say that text was recovered',
  memory_measured:
    'a peak resident set size was read for the converter process; the size is a measurement and no limit is applied',
  refused:
    'the converter itself refuses the truncated input (convert returns Err, or Ok with status Failure) and the fixture bytes are unchanged; a converter process that dies on the input has not refused it and fails this criterion',
  refusal_attributed:
    'another PDF fixture converted in the same run, so the refusal is not a broken PDF pipeline, and what the refusal text says about the build agrees with what cargo resolved',
  timeout_reported:
    'with a 1 ms document budget the converter returns PartialSuccess with a pipeline error that names the timeout; a converter process that dies under the budget has not reported it and fails this criterion',
  budget_had_effect:
    'the budget changed the result in a way a timeout that did nothing could not: the probe reported the spent budget and its document holds fewer pages or fewer items than the conversion of the same bytes without a budget in the same run; elapsed time is recorded and not judged',
});

const aspects = (list) => list.map((entry) => (typeof entry === 'string' ? { aspect: entry, required: true } : entry));

/**
 * The name of the criterion that judges a fixture's `expect` block. A fixture that shows no
 * glyphs (`no_text`) asserts one thing, that no text is invented, and is named for it.
 */
export function contentAspect(source) {
  return source?.expect?.no_text === true ? 'no_invented_text' : 'content';
}

/**
 * The expectations of a fixture that cross a font change: the `markdown_contains` entries
 * marked `crosses_font_runs`. They are judged by content_across_font_runs and not by content.
 */
export function fontRunExpectations(source) {
  return (source?.expect?.markdown_contains ?? []).filter((entry) => entry !== null && typeof entry === 'object' && entry.crosses_font_runs === true);
}

/**
 * The fixture's `expect` block in two parts: `plain` without the expectations that cross a
 * font change, and `font_runs` with only those (null when the fixture marks none). Both are
 * judged by the same function.
 */
export function splitExpect(source) {
  const marked = fontRunExpectations(source);
  if (marked.length === 0) return { plain: source?.expect, font_runs: null };
  return {
    plain: { ...source.expect, markdown_contains: source.expect.markdown_contains.filter((entry) => !marked.includes(entry)) },
    font_runs: { confirmed_by: source.expect.crosses_font_runs_confirmed_by ?? null, markdown_contains: marked },
  };
}

/**
 * The criteria one fixture run emits, in the order they are judged.
 * @param {string} name a key of SOURCES.json `files`, or TIMEOUT_PROBE
 * @param {object} [source] the fixture's entry in SOURCES.json
 * @returns {{ aspect: string, required: boolean }[]}
 */
export function fixtureAspects(name, source) {
  if (name === TIMEOUT_PROBE) return aspects(['timeout_reported', 'evidence', 'budget_had_effect', 'memory_measured']);
  if (source?.role === 'must_fail') return aspects(['refused', 'refusal_attributed', 'memory_measured']);
  const kind = KINDS[source?.kind];
  if (!kind) {
    throw new Error(`SOURCES.json: ${name} must declare kind as one of ${Object.keys(KINDS).join(', ')}; found ${JSON.stringify(source?.kind ?? null)}`);
  }
  const noText = contentAspect(source) === 'no_invented_text';
  return aspects([
    'conversion',
    'evidence',
    'format_recognised',
    contentAspect(source),
    ...(fontRunExpectations(source).length > 0 ? ['content_across_font_runs'] : []),
    'page_renders',
    { aspect: 'provenance', required: kind.provenance !== 'recorded' },
    // Where locations are judged and the fixture has no text to locate, say so as a criterion.
    ...(noText && kind.provenance === 'judged' ? ['text_provenance'] : []),
    ...(kind.text_layer ? ['undecoded_glyphs_reported'] : []),
    'memory_measured',
  ]);
}

/**
 * Every criterion this harness can emit, in receipt order: the run, each fixture of
 * SOURCES.json, the timeout probe.
 * @param {{ files: Record<string, object> }} sources SOURCES.json
 * @returns {{ id: string, required: boolean }[]}
 */
export function expectedCriteria(sources) {
  const list = RUN_CRITERIA.map((id) => ({ id, required: true }));
  for (const name of [...Object.keys(sources?.files ?? {}), TIMEOUT_PROBE]) {
    for (const { aspect, required } of fixtureAspects(name, sources?.files?.[name])) list.push({ id: `${name}/${aspect}`, required });
  }
  return list;
}

/** The ids criteria.json must list: every criterion this harness emits as required, sorted. */
export function requiredIds(sources) {
  return expectedCriteria(sources)
    .filter((criterion) => criterion.required)
    .map((criterion) => criterion.id)
    .sort();
}

export const pass = (evidence = {}) => ({ result: 'pass', ...evidence });
export const fail = (detail, evidence = {}) => ({ result: 'fail', detail, ...evidence });
export const notJudged = (detail, evidence = {}) => ({ result: 'not_judged', detail, ...evidence });
export const notApplicable = (detail, evidence = {}) => ({ result: 'not_applicable', detail, ...evidence });
/** Not judged because the environment stopped it: the sentence is also the run's harness error. */
export const stopped = (detail, evidence = {}) => ({ result: 'not_judged', detail, harness_error: detail, ...evidence });

/** True for a result that is neither a pass nor a fail. */
export const unjudged = (result) => result === 'not_judged' || result === 'not_applicable';

/** The flat criterion the envelope lists for a judgement. */
function criterionOf(id, required, judgement) {
  const criterion = { id, required, result: judgement.result };
  if (typeof judgement.detail === 'string' && judgement.detail.length > 0) criterion.detail = judgement.detail;
  return criterion;
}

/** The harness errors a set of judgements carries, each once, in order. */
function harnessErrors(judgements) {
  return [...new Set(judgements.map((judgement) => judgement?.harness_error).filter((text) => typeof text === 'string' && text.length > 0))];
}

/**
 * The label of one fixture, folded from its own criteria by the shared function.
 * @param {string} name fixture run name
 * @param {{ aspect: string, required: boolean }[]} list fixtureAspects of the run
 * @param {Record<string, object>} judged aspect -> judgement
 */
export function fixtureLabel(name, list, judged) {
  const criteria = list.map(({ aspect, required }) => criterionOf(`${name}/${aspect}`, required, judged[aspect]));
  return foldCriteria(criteria, harnessErrors(Object.values(judged))[0] ?? null);
}

/**
 * The top-level envelope of the receipt.
 * @param {object} input
 * @param {{ id: string, required: boolean }[]} input.expected expectedCriteria of the run
 * @param {Map<string, object>} input.judgements id -> judgement; an expected id without one is not_judged
 * @param {string|null} [input.harnessError] what stopped the run before it judged anything
 */
export function envelope({ expected, judgements, harnessError = null }) {
  const known = new Set(expected.map((criterion) => criterion.id));
  const stray = [...judgements.keys()].filter((id) => !known.has(id));
  if (stray.length) throw new Error(`judgement for a criterion the harness does not declare: ${stray.join(', ')}`);
  const criteria = expected.map(({ id, required }) => criterionOf(id, required, judgements.get(id) ?? notJudged('no judgement was made for this criterion')));
  const errors = harnessError ? [harnessError] : harnessErrors([...judgements.values()]);
  // One sentence: the first error, and how many more there are. Each is the detail of the criteria it stopped.
  const more = errors.length > 1 ? ` (and ${errors.length - 1} more harness error(s); each is the detail of the criteria it stopped)` : '';
  const harness_error = errors.length ? `${errors[0]}${more}` : null;
  const open = criteria.filter((criterion) => unjudged(criterion.result));
  return {
    gate: GATE,
    result: foldCriteria(criteria, harness_error),
    harness_error,
    not_judged: open.map((criterion) => criterion.id),
    not_judged_reasons: Object.fromEntries(open.map((criterion) => [criterion.id, `${criterion.result}: ${criterion.detail ?? 'no reason recorded'}`])),
    criteria,
  };
}
