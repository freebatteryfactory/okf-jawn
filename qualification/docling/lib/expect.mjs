/**
 * Judge what a converted fixture contains against the `expect` block its entry in
 * tests/fixtures/documents/SOURCES.json declares. Pure: no I/O and no imports.
 *
 * An expectation is written from the fixture (its generator, its XML, an independent text
 * extraction), never from converter output. A fixture with no `expect` block, or with one
 * that checks nothing, fails: "it converted" is not content.
 */

/** How each kind of expectation is compared; the receipt carries these sentences. */
export const MATCH_RULES = Object.freeze({
  markdown_contains:
    'each string must occur in the exported Markdown after every run of whitespace is collapsed to one space; case-sensitive unless the entry sets case_insensitive',
  table_rows:
    'the cells must occur in this order among the trimmed cell texts of one row of one body-layer table; other cells may sit between them',
  counts:
    'tables, pictures and headings are counted on the body layer of the document model (headings are text items labelled title or section_header)',
  sheet_names:
    'the names of the body-layer groups labelled sheet must equal the visible worksheet names, in workbook order',
  pages:
    'the page count of the document model must equal the fixture page (or slide) count; for PDF and image fixtures this is judged with the page renders instead',
  ocr_tokens:
    'the Markdown minus HTML comments is lower-cased and split on every character that is not a letter or digit; each expected token must equal one observed token exactly. No character confusion (O/0, I/1/l, S/5, B/8) is forgiven and no edit distance is allowed',
  no_text:
    'the fixture shows no glyphs, so the Markdown minus HTML comments must hold no letter or digit; any token is text the converter invented',
});

/** Collapse every run of whitespace to one space. */
export const collapse = (text) => String(text ?? '').replace(/\s+/g, ' ').trim();

/** Lower-cased letter/digit tokens of the Markdown, with HTML comments (picture placeholders) removed. */
export function textTokens(markdown) {
  return String(markdown ?? '')
    .replace(/<!--[\s\S]*?-->/g, ' ')
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
}

/** True when `cells` occur in order within `row`. */
export function rowHasCells(row, cells) {
  let next = 0;
  for (const cell of row) {
    if (next < cells.length && collapse(cell) === collapse(cells[next])) next += 1;
  }
  return next === cells.length;
}

function countCheck(kind, comparison, expected, observed) {
  const ok = comparison === 'exact' ? observed === expected : observed >= expected;
  return { kind, comparison, expected, observed, ok };
}

/**
 * @param {object|undefined} expect the fixture's `expect` block
 * @param {{ markdown: string, tables: string[][][], headings: number, pictures: number, sheet_names: string[], page_count: number|null }} observed
 *   body-layer facts of the converted document; `tables` is one string[][] grid per table;
 *   `page_count` is null when the page count is judged elsewhere (PDF and image fixtures)
 */
export function judgeContent(expect, observed) {
  if (expect === null || typeof expect !== 'object' || Array.isArray(expect)) {
    return { status: 'FAIL', reason: 'the fixture declares no expect block in SOURCES.json', checks: [], found: 0, total: 0 };
  }
  const markdown = collapse(observed.markdown);
  const tokens = textTokens(observed.markdown);
  const checks = [];

  for (const entry of expect.markdown_contains ?? []) {
    const text = typeof entry === 'string' ? entry : entry.text;
    const insensitive = typeof entry === 'object' && entry.case_insensitive === true;
    const needle = collapse(text);
    const ok = insensitive ? markdown.toLowerCase().includes(needle.toLowerCase()) : markdown.includes(needle);
    checks.push({ kind: 'markdown_contains', expected: text, case_insensitive: insensitive, ok });
  }
  for (const cells of expect.table_rows ?? []) {
    const ok = observed.tables.some((grid) => grid.some((row) => rowHasCells(row, cells)));
    checks.push({ kind: 'table_row', expected: cells, ok });
  }
  if (Number.isInteger(expect.tables)) checks.push(countCheck('tables', 'exact', expect.tables, observed.tables.length));
  if (Number.isInteger(expect.tables_at_least)) {
    checks.push(countCheck('tables', 'at_least', expect.tables_at_least, observed.tables.length));
  }
  if (Number.isInteger(expect.headings_at_least)) {
    checks.push(countCheck('headings', 'at_least', expect.headings_at_least, observed.headings));
  }
  if (Number.isInteger(expect.pictures_at_least)) {
    checks.push(countCheck('pictures', 'at_least', expect.pictures_at_least, observed.pictures));
  }
  if (Array.isArray(expect.sheet_names)) {
    const names = observed.sheet_names ?? [];
    checks.push({
      kind: 'sheet_names',
      expected: expect.sheet_names,
      observed: names,
      ok: names.length === expect.sheet_names.length && names.every((name, index) => name === expect.sheet_names[index]),
    });
  }
  if (Number.isInteger(expect.pages) && observed.page_count !== null && observed.page_count !== undefined) {
    checks.push(countCheck('pages', 'exact', expect.pages, observed.page_count));
  }
  for (const token of expect.ocr_tokens ?? []) {
    checks.push({ kind: 'ocr_token', expected: token, ok: tokens.includes(String(token).toLowerCase()) });
  }
  if (expect.no_text === true) {
    checks.push({ kind: 'no_text', expected: [], observed: tokens.slice(0, 50), ok: tokens.length === 0 });
  }

  const found = checks.filter((check) => check.ok).length;
  const result = {
    status: checks.length > 0 && found === checks.length ? 'PASS' : 'FAIL',
    found,
    total: checks.length,
    confirmed_by: expect.confirmed_by ?? null,
    observed: {
      markdown_chars: String(observed.markdown ?? '').length,
      text_tokens: tokens.length,
      tables: observed.tables.length,
      headings: observed.headings,
      pictures: observed.pictures,
    },
    checks,
  };
  if (checks.length === 0) result.reason = 'the expect block checks nothing';
  // Text that exists only as pixels is recorded as read, pass or fail, so a FAIL carries its evidence.
  if ((expect.ocr_tokens ?? []).length > 0 || expect.no_text === true) {
    result.ocr_exercised = (expect.ocr_tokens ?? []).length > 0;
    result.observed_text = collapse(String(observed.markdown ?? '').replace(/<!--[\s\S]*?-->/g, ' ')).slice(0, 2000);
  }
  return result;
}
