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

/** What a passing provenance criterion asserts; the receipt carries this sentence. */
export const PROVENANCE_RULE =
  'every text item, table and picture has provenance, each page_no is a page of the document and each bbox has area and lies inside that page';

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

function coverage(items, pages) {
  const judged = items.map((item) => ({ item, problem: itemProblem(item, pages) }));
  const invalid = judged.filter((entry) => entry.problem !== null);
  return {
    total: items.length,
    with_provenance: items.filter((item) => item.prov.length > 0).length,
    located: items.length - invalid.length,
    invalid_total: invalid.length,
    invalid: invalid.slice(0, INVALID_SHOWN).map(({ item, problem }) => ({ ref: item.ref, label: item.label, problem })),
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
 * @param {{ paginated: boolean }} options paginated: the fixture is a PDF or an image
 */
export function judgeProvenance(doc, { paginated }) {
  const texts = coverage(doc.texts, doc.pages);
  const tables = coverage(doc.tables, doc.pages);
  const pictures = coverage(doc.pictures, doc.pages);
  const onPage = (items, pageNo) => items.filter((item) => item.prov.some((entry) => entry.page_no === pageNo)).length;
  const pageProvenance = doc.pages.map((page) => ({
    page_no: page.page_no,
    text_items: onPage(doc.texts, page.page_no),
    tables: onPage(doc.tables, page.page_no),
    pictures: onPage(doc.pictures, page.page_no),
  }));
  const items = doc.texts.length + doc.tables.length + doc.pictures.length;
  const located = texts.located + tables.located + pictures.located;
  const base = {
    page_count: doc.pages.length,
    items: { total: items, located },
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
