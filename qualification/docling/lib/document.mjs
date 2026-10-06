/**
 * Read the converter's own document export and judge the locators and page renders in it.
 * Pure: no I/O and no imports.
 *
 * Input is the JSON `DoclingDocument::export_to_json_value()` returns (docling-core's wire
 * schema: texts/tables/pictures with `prov` entries of page_no + bbox, a `pages` map, groups),
 * which the Rust harness writes unmodified apart from removing the page images it records
 * separately. Nothing here reads the Markdown and nothing is taken from fixture names.
 *
 * SPEC section 7 promises "inclusive pages/lines" selections and "exact resolved citations";
 * section 5 "relevant page renders, source locators". So a paginated fixture (PDF, image)
 * must give every text item, table and picture a page inside the document and a box inside
 * that page, and one render per page. Other formats are recorded as the library reports them, not judged.
 *
 * An item is located by one of two sources. The export: the item's own `prov`. Or, for a PDF
 * item the export gives no `prov` (the library drops a caption's box), the library's own
 * text-layer document: the converter process looks the item's text up there by the rule of
 * src/locate.rs and records the page and box it found; this module holds that box to the same
 * rule as an exported one. An item neither source locates is not located, and nothing here
 * guesses a box.
 *
 * A render must also depict its page: the orchestrator decodes each image (lib/png.mjs) and
 * this module fails one that is a single flat colour, unless the fixture's entry in
 * SOURCES.json declares that page blank, in which case the render must be flat.
 */

const HEADING_LABELS = ['title', 'section_header'];
/** The export rounds coordinates to two decimals (docling-core json.rs:667). */
const ROUNDING = 0.011;
/** `invalid` lists at most this many items; `invalid_total` is the full count. */
const INVALID_SHOWN = 10;
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/**
 * What locating through the text layer does not guarantee; the receipt carries this sentence
 * with the rule. It is the sentence src/locate.rs states as LOCATE_LIMITS (a test holds the
 * two equal): the harness review placed an item wrongly on two documents written for it, and
 * both passed.
 */
export const TEXT_LAYER_LIMITS =
  "The rule does not guarantee that the box is the item's own. An item under the body that starts a page is searched on the page of its earlier sibling, so the same words standing once on that earlier page (a running footer) are taken for it; and when the page sets the item's text differently (a caption continued as 'Figure 1 (continued)') while another line of that page is exactly the item's text, that line is taken. Nothing holds the box found to the box of the parent: for each item the lookup records the distance between the box found and the box of the item whose page was searched (lookup.distance, in page units, 0 when they touch or overlap) as a measurement, and no threshold is applied";

/** What a passing provenance criterion asserts; the receipt carries this sentence. */
export const PROVENANCE_RULE = `every text item, table and picture is located: by its own provenance in the export or, for a PDF item the export gives none, by the one text-layer item on the page of its parent (or of its nearest located earlier sibling) that has exactly its text; each page_no is a page of the document and each bbox has area and lies inside that page; the detail states how many items each source located and how many none did. ${TEXT_LAYER_LIMITS}`;

/** What `text_layer_distances` holds; the receipt carries this sentence beside the numbers. */
export const DISTANCE_STATEMENT =
  'for each item the text layer located, the shortest distance in page units between the box found and the box of the item whose page was searched (its parent, or its nearest located earlier sibling), 0 when they touch or overlap, as src/locate.rs recorded it. A measurement: no threshold is applied and no item passes or fails on it';

/** The sources an item can be located by, in the order the counts are stated. */
export const LOCATION_SOURCES = Object.freeze(['export', 'text_layer', 'none']);

/** What a passing page_renders criterion asserts; the receipt carries this sentence. */
export const PAGE_RENDER_RULE =
  'one image per page of the document, the page count the fixture declares and the library reports, each file re-read with the hash and pixel size the library gave; every image is decoded, and a render that is a single flat colour fails unless SOURCES.json declares that page blank (expect.blank_pages); a page declared blank must render flat';

const list = (value) => (Array.isArray(value) ? value : []);
const layerOf = (item) => item?.content_layer ?? 'body';

function provOf(item) {
  return list(item?.prov).map((entry) => ({ page_no: entry?.page_no ?? null, bbox: entry?.bbox ?? null }));
}

/** The parts of a docling JSON export this harness judges, in a flat shape. */
export function describeDocument(json) {
  const pages = Object.values(json?.pages ?? {})
    .map((page) => ({ page_no: page?.page_no, width: page?.size?.width, height: page?.size?.height }))
    .sort((a, b) => a.page_no - b.page_no);
  return {
    pages,
    texts: list(json?.texts).map((item) => ({
      ref: item.self_ref,
      label: item.label,
      layer: layerOf(item),
      text: String(item.text ?? ''),
      prov: provOf(item),
    })),
    tables: list(json?.tables).map((item) => ({
      ref: item.self_ref,
      label: item.label,
      layer: layerOf(item),
      prov: provOf(item),
      rows: list(item.data?.grid).map((row) => list(row).map((cell) => String(cell?.text ?? ''))),
      cells: list(item.data?.table_cells).map((cell) => ({ text: String(cell?.text ?? ''), bbox: cell?.bbox ?? null })),
    })),
    pictures: list(json?.pictures).map((item) => ({
      ref: item.self_ref,
      label: item.label,
      layer: layerOf(item),
      prov: provOf(item),
      captions: list(item.captions).length,
      has_image: typeof item.image?.uri === 'string' && item.image.uri.length > 0,
    })),
    groups: list(json?.groups).map((group) => ({ label: group.label, name: group.name ?? null, layer: layerOf(group) })),
  };
}

/** Body-layer structure for the content expectations. */
export function bodyFacts(doc) {
  const body = (items) => items.filter((item) => item.layer === 'body');
  return {
    tables: body(doc.tables).map((table) => table.rows),
    headings: body(doc.texts).filter((item) => HEADING_LABELS.includes(item.label)).length,
    pictures: body(doc.pictures).length,
    pictures_with_image: body(doc.pictures).filter((item) => item.has_image).length,
    pictures_with_caption: body(doc.pictures).filter((item) => item.captions > 0).length,
    sheet_names: body(doc.groups).filter((group) => group.label === 'sheet').map((group) => group.name),
    items_on_other_layers:
      doc.texts.length + doc.tables.length + doc.pictures.length -
      body(doc.texts).length - body(doc.tables).length - body(doc.pictures).length,
  };
}

/** Why `bbox` does not locate a region of `page`, or null when it does. */
export function bboxProblem(bbox, page) {
  if (bbox === null || typeof bbox !== 'object') return 'no bbox';
  const { l, t, r, b } = bbox;
  if (![l, t, r, b].every((value) => typeof value === 'number' && Number.isFinite(value))) return 'bbox is not numeric';
  if (bbox.coord_origin !== 'BOTTOMLEFT' && bbox.coord_origin !== 'TOPLEFT') {
    return `unknown coord_origin ${JSON.stringify(bbox.coord_origin ?? null)}`;
  }
  const [low, high] = bbox.coord_origin === 'BOTTOMLEFT' ? [b, t] : [t, b];
  if (l < -ROUNDING || r > page.width + ROUNDING || low < -ROUNDING || high > page.height + ROUNDING) {
    return `bbox outside the ${page.width} x ${page.height} page`;
  }
  if (r - l <= 0 || high - low <= 0) return 'bbox has no area';
  return null;
}

/** Why an item is not located on a page of the document, or null when every prov entry is good. */
function itemProblem(item, pages) {
  if (item.prov.length === 0) return 'no provenance';
  for (const entry of item.prov) {
    const page = pages.find((candidate) => candidate.page_no === entry.page_no);
    if (!page) return `page_no ${JSON.stringify(entry.page_no)} is not a page of the document (1..=${pages.length})`;
    const problem = bboxProblem(entry.bbox, page);
    if (problem) return problem;
  }
  return null;
}

/**
 * Where one item is and which source says so: `{ located_by, page_no, bbox, problem, lookup }`.
 * The export decides for an item that has provenance, sound or not. For one that has none,
 * `lookups` may hold what the converter process found in the text layer; that page and box
 * are held to the same rule as exported ones.
 */
function locate(item, pages, lookups) {
  const nowhere = (problem, lookup) => ({ located_by: 'none', page_no: null, bbox: null, problem, ...(lookup ? { lookup } : {}) });
  if (item.prov.length > 0) {
    const problem = itemProblem(item, pages);
    if (problem !== null) return nowhere(problem);
    return { located_by: 'export', page_no: item.prov[0].page_no, bbox: item.prov[0].bbox, problem: null };
  }
  const found = lookups?.get(item.ref);
  if (found?.located_by !== 'text_layer') {
    const why = found?.lookup?.reason;
    return nowhere(why ? `no provenance; text layer: ${why}` : 'no provenance', found?.lookup);
  }
  const page = pages.find((candidate) => candidate.page_no === found.page_no);
  const problem = page ? bboxProblem(found.bbox, page) : `page_no ${JSON.stringify(found.page_no ?? null)} is not a page of the document (1..=${pages.length})`;
  if (problem !== null) return nowhere(`no provenance; text layer: ${problem}`, found.lookup);
  return { located_by: 'text_layer', page_no: found.page_no, bbox: found.bbox, problem: null, lookup: found.lookup };
}

function coverage(kind, items, pages, lookups) {
  const judged = items.map((item) => ({ item, where: locate(item, pages, lookups) }));
  const invalid = judged.filter((entry) => entry.where.located_by === 'none');
  return {
    total: items.length,
    with_provenance: items.filter((item) => item.prov.length > 0).length,
    located: items.length - invalid.length,
    located_by: Object.fromEntries(LOCATION_SOURCES.map((source) => [source, judged.filter((entry) => entry.where.located_by === source).length])),
    invalid_total: invalid.length,
    invalid: invalid.slice(0, INVALID_SHOWN).map(({ item, where }) => ({ ref: item.ref, label: item.label, problem: where.problem })),
    // Every item with the source that located it, its page and its box.
    items: judged.map(({ item, where }) => ({ ref: item.ref, kind, label: item.label, ...where })),
  };
}

/**
 * The distances the lookup recorded for the items the text layer located, with their range.
 * `unmeasured` counts the items whose reference has no box on the page (the lookup recorded null).
 */
function textLayerDistances(items) {
  const located = items.filter((item) => item.located_by === 'text_layer');
  const measured = (item) => typeof item.lookup?.distance === 'number' && Number.isFinite(item.lookup.distance);
  const values = located.filter(measured).map((item) => item.lookup.distance);
  return {
    statement: DISTANCE_STATEMENT,
    threshold_applied: false,
    measured: values.length,
    unmeasured: located.length - values.length,
    min: values.length ? Math.min(...values) : null,
    max: values.length ? Math.max(...values) : null,
    items: located.map((item) => ({
      ref: item.ref,
      basis: item.lookup?.basis ?? null,
      reference: item.lookup?.reference ?? null,
      distance: measured(item) ? item.lookup.distance : null,
    })),
  };
}

const sampleOf = (item) =>
  item
    ? {
        ref: item.ref,
        label: item.label,
        text: item.text.slice(0, 80),
        page_no: item.prov[0]?.page_no ?? null,
        bbox: item.prov[0]?.bbox ?? null,
      }
    : null;

function tableCellSample(doc) {
  for (const table of doc.tables) {
    const cell = table.cells.find((candidate) => candidate.bbox && candidate.text.trim().length > 0);
    if (cell) {
      return { ref: table.ref, label: 'table_cell', text: cell.text.slice(0, 80), page_no: table.prov[0]?.page_no ?? null, bbox: cell.bbox };
    }
  }
  return null;
}

/**
 * @param {ReturnType<typeof describeDocument>} doc
 * @param {{ paginated: boolean, lookups?: Map<string, object>|null }} options
 *   paginated: the fixture is a PDF or an image. lookups: for a PDF, the entry the converter
 *   process recorded for each item (src/locate.rs), by the item's ref; only the entries of
 *   items the export left without provenance are read
 */
export function judgeProvenance(doc, { paginated, lookups = null }) {
  const all = {
    texts: coverage('text', doc.texts, doc.pages, lookups),
    tables: coverage('table', doc.tables, doc.pages, lookups),
    pictures: coverage('picture', doc.pictures, doc.pages, lookups),
  };
  // The list of every item is kept once, in document order, beside the per-kind counts.
  const itemList = Object.values(all).flatMap((kind) => kind.items);
  const [texts, tables, pictures] = Object.values(all).map(({ items: _listed, ...counts }) => counts);
  // A page holds the items whose provenance names it and the items the text layer located on it.
  const onPage = (kind, exported, pageNo) =>
    exported.filter((item) => item.prov.some((entry) => entry.page_no === pageNo)).length +
    itemList.filter((item) => item.kind === kind && item.located_by === 'text_layer' && item.page_no === pageNo).length;
  const pageProvenance = doc.pages.map((page) => ({
    page_no: page.page_no,
    text_items: onPage('text', doc.texts, page.page_no),
    tables: onPage('table', doc.tables, page.page_no),
    pictures: onPage('picture', doc.pictures, page.page_no),
  }));
  const items = doc.texts.length + doc.tables.length + doc.pictures.length;
  const located = texts.located + tables.located + pictures.located;
  const base = {
    page_count: doc.pages.length,
    items: {
      total: items,
      located,
      located_by: Object.fromEntries(LOCATION_SOURCES.map((source) => [source, texts.located_by[source] + tables.located_by[source] + pictures.located_by[source]])),
    },
    text_items: texts,
    tables,
    pictures,
    sample: {
      first: sampleOf(doc.texts[0]),
      last: sampleOf(doc.texts.at(-1)),
      heading: sampleOf(doc.texts.find((item) => HEADING_LABELS.includes(item.label))),
      table_cell: tableCellSample(doc),
    },
    page_provenance: pageProvenance,
    // Paginated fixtures only: each item with the source that located it, its page and its box.
    ...(paginated ? { located_items: itemList } : {}),
    // A PDF only: how far each item the text layer located lies from the item whose page was searched.
    ...(paginated && lookups ? { text_layer_distances: textLayerDistances(itemList) } : {}),
  };
  if (!paginated) {
    return {
      status: 'recorded_not_judged',
      reason: 'not a PDF or image fixture: the locator the library gives is recorded as observed',
      // Counted from items that are located by the same rule as above, not from items that merely carry a prov entry.
      locator: located === 0 ? 'none' : located === items ? 'page_and_bbox_on_every_item' : 'page_and_bbox_on_some_items',
      groups: [...new Set(doc.groups.map((group) => `${group.label}:${group.name ?? ''}`))].slice(0, 50),
      ...base,
    };
  }
  if (items === 0) {
    return { status: 'not_exercised', reason: 'the converted document holds no text item, table or picture', ...base };
  }
  return { status: located === items ? 'PASS' : 'FAIL', rule: PROVENANCE_RULE, ...base };
}

/** Width and height from a PNG's IHDR, or null when the bytes do not start as a PNG. */
export function pngSize(bytes) {
  if (!bytes || bytes.length < 24) return null;
  if (!PNG_SIGNATURE.every((value, index) => bytes[index] === value)) return null;
  if (String.fromCharCode(bytes[12], bytes[13], bytes[14], bytes[15]) !== 'IHDR') return null;
  const u32 = (at) => ((bytes[at] << 24) | (bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3]) >>> 0;
  return { width: u32(16), height: u32(20) };
}

/**
 * @param {object} input
 * @param {boolean} input.applicable the fixture goes through the PDF/image pipeline
 * @param {number|null} input.expectedPages page count declared for the fixture
 * @param {{ value: number|null, error: string|null }|null} input.libraryPageCount docling::pdf_page_count
 * @param {{ page_no: number, width: number, height: number }[]} input.pages pages of the document
 * @param {object[]} input.images what the harness recorded per page image, with `file` = the
 *   written file re-read by the orchestrator ({ sha256, bytes, png, ink }) or null when unreadable;
 *   `ink` is what lib/png.mjs pngInk read from the pixels
 * @param {number[]} [input.blankPages] pages the fixture declares blank in SOURCES.json
 */
export function judgePageRenders({ applicable, expectedPages, libraryPageCount, pages, images, blankPages = [] }) {
  if (!applicable) {
    return {
      status: 'not_applicable',
      reason: 'the library keeps page images for the PDF/image pipeline only (docling converter.rs:756 generate_page_images)',
      page_image_count: images.length,
    };
  }
  const base = {
    page_count: pages.length,
    page_image_count: images.length,
    expected_pages: expectedPages ?? null,
    library_page_count: libraryPageCount ?? null,
    blank_pages: blankPages,
  };
  if (images.length === 0) {
    return {
      status: 'unavailable',
      reason: `DocumentConverter::generate_page_images(true) was applied and the returned document.page_images is empty for ${pages.length} page(s)`,
      ...base,
    };
  }
  const problems = [];
  const undecoded = [];
  if (pages.length === 0) problems.push('the document has no pages map');
  if (Number.isInteger(expectedPages) && pages.length !== expectedPages) {
    problems.push(`the document has ${pages.length} page(s); the fixture has ${expectedPages}`);
  }
  if (Number.isInteger(libraryPageCount?.value) && libraryPageCount.value !== pages.length) {
    problems.push(`docling::pdf_page_count says ${libraryPageCount.value}; the document has ${pages.length}`);
  }
  if (images.length !== pages.length) problems.push(`${images.length} page image(s) for ${pages.length} page(s)`);
  for (const blank of blankPages) {
    if (!pages.some((page) => page.page_no === blank)) problems.push(`blank page ${blank} is declared in SOURCES.json and is not a page of the document`);
  }
  const renders = images.map((image) => {
    const page = pages.find((candidate) => candidate.page_no === image.page_no);
    if (!page) problems.push(`page image ${image.page_no} belongs to no page of the document`);
    if (!(image.width > 0 && image.height > 0)) problems.push(`page ${image.page_no}: image has no pixels`);
    if (!image.file) problems.push(`page ${image.page_no}: the written image could not be re-read`);
    else {
      if (image.file.sha256 !== image.sha256) problems.push(`page ${image.page_no}: file hash differs from the hash the harness recorded`);
      if (image.mimetype === 'image/png') {
        if (!image.file.png) problems.push(`page ${image.page_no}: bytes are not a PNG`);
        else if (image.file.png.width !== image.width || image.file.png.height !== image.height) {
          problems.push(
            `page ${image.page_no}: PNG is ${image.file.png.width}x${image.file.png.height}, the library reports ${image.width}x${image.height}`,
          );
        }
      }
    }
    const declaredBlank = blankPages.includes(image.page_no);
    const ink = image.mimetype === 'image/png' ? (image.file?.ink ?? null) : null;
    if (image.file) {
      if (image.mimetype !== 'image/png') undecoded.push(`page ${image.page_no}: ${image.mimetype} pixels are not read by this harness`);
      else if (!ink?.decoded) undecoded.push(`page ${image.page_no}: the pixels could not be read (${ink?.reason ?? 'the file was not decoded'})`);
      else if (ink.uniform && !declaredBlank) {
        problems.push(`page ${image.page_no}: the render is one flat colour (sample bytes ${ink.first_pixel.join(' ')}) and SOURCES.json does not declare this page blank`);
      } else if (!ink.uniform && declaredBlank) {
        problems.push(`page ${image.page_no}: declared blank in SOURCES.json but ${ink.differing_pixels} of ${ink.pixels} pixel(s) differ from the first`);
      }
    }
    return {
      page_no: image.page_no,
      width_px: image.width,
      height_px: image.height,
      mimetype: image.mimetype,
      dpi: image.dpi,
      bytes: image.bytes,
      sha256: image.sha256,
      px_per_page_unit: page && page.width > 0 ? Math.round((image.width / page.width) * 1e4) / 1e4 : null,
      declared_blank: declaredBlank,
      uniform: ink?.decoded ? ink.uniform : null,
      differing_pixels: ink?.decoded ? ink.differing_pixels : null,
    };
  });
  for (const page of pages) {
    if (!images.some((image) => image.page_no === page.page_no)) problems.push(`page ${page.page_no} has no image`);
  }
  // A problem that was found stands; pixels that could not be read leave the rest unjudged, never passed.
  const status = problems.length ? 'FAIL' : undecoded.length ? 'not_judged' : 'PASS';
  return { status, rule: PAGE_RENDER_RULE, ...base, problems, undecoded, renders };
}

/**
 * Named options out of the `Debug` text of a `docling::DocumentConverter`, as raw Rust
 * literals ("None", "true", "Some(\"en\")"). A name the text does not hold maps to null.
 */
export function converterOptions(debug, names) {
  const text = String(debug ?? '');
  return Object.fromEntries(
    names.map((name) => {
      const match = new RegExp(`\\b${name}: (Some\\((?:"[^"]*"|[^()]*)\\)|"[^"]*"|[A-Za-z0-9_.]+)`).exec(text);
      return [name, match ? match[1] : null];
    }),
  );
}
