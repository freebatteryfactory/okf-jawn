# Stage 1b: contract design for the eight shared shapes

Status: approved; sections 1 to 8, 10 to 13 implemented on `cure/stage-1b-contract` (contract,
xtask, SPEC, generated outputs), section 9 is the core-cli package that follows. Temporary
working document, removed when Stage 1 closes like the
Stage 1 design beside it; SPEC.md, README.md and AGENTS.md stay the canonical prose. Base:
`eda0253` on `main`. Branch: `cure/stage-1b-contract`.

This document fixes the shared wire types and port shapes for eight subjects, so that storage,
ingest, MCP and the UI mean the same thing by the same object. It is the input of two gates:
`contract-expresses-extraction-sources-views` (integration owner, sections 1 to 8 and 10) and
`core-ports-carry-extraction-and-views` (core-cli, section 9). It is not a lane plan.

Marks. `[verified: path:line]` cites the source that shows a library capability; library paths
are relative to the crate root of `docling-1.93.5/src/...` (crates.io, as Cargo.lock pins it),
to `docling.rs@e500c23/crates/...` (the fork commit Cargo.lock pins for docling-core, docling-onnx
and docling-pdf 1.93.6; every file cited from it was compared with the crates.io 1.93.6 copy and is
identical), to `okf-core-0.2.7/src/...`, or to this repository. `[inferred]` marks what was not
read in source.

Fix round 1 (after the independent review, CHANGES_REQUIRED) changed every section; the
owner questions of round 0 are answered by the orchestrator's rulings and carried in sections 6,
8, 13 and 14. The final design review's eight decisions are applied where they bind: one
`Timestamp` spelling with no leap second (section 7), the narrowed header rule (9.3), the import
refusal of a live or purged workspace's archive (8), resumption before `base_revision` (6), the
converse of `Job.workspace_id` (8), the rewritten core caption test (9.1), `generated/converter/`
as a generated directory (12) and datasets through the revision map (6).

## 0. Conventions this design applies everywhere

- Every new struct derives `Debug, Clone, Serialize, Deserialize, JsonSchema` and carries
  `#[serde(deny_unknown_fields)]`. Every new plain enum derives `Debug, Clone, Copy, Serialize,
  Deserialize, JsonSchema, PartialEq, Eq` with `#[serde(rename_all = "snake_case")]`. Every
  enum with data is internally tagged with `rename_all = "snake_case"` and
  `deny_unknown_fields`; the tag is `kind` unless a section names another. Code blocks below show
  only attributes that differ from these.
- An optional field is `Option<T>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
  (marked `// opt` below); an optional list is `Vec<T>` with
  `#[serde(default, skip_serializing_if = "Vec::is_empty")]` (marked `// opt-list`). Both keep the
  one-wire-shape rule of `crates/contract/tests/schema.rs`. No struct-level `#[serde(default)]`
  and no defaulted `bool` appear on the wire: both split the read and write schemas.
- A `u64` byte count is a decimal `String`, as `DownloadArtifact::size` already is.
- Requests take `At`; responses and stored records carry resolved `Revision`s.
- Every type below is produced by one lane and read by another; the producer and consumer are
  named where it is not obvious.

New contract files: `crates/contract/src/extraction.rs` ("Conversion outcome, converter
identity, settings and supplied text of a source") and `crates/contract/src/purge.rs`
("Archive is reversible; purge is explicit, destructive and recorded"). Both are added to
`lib.rs`. The operation table grows from 69 to 77 operations (section 10); the model tool set (12)
and the app tool set (`read_object`) do not change.

## 1. Conversion outcome and converter identity

SPEC §5: "Unsupported content is visible as unsupported, not silently empty or successfully
converted." "Uploaded bytes are preserved while conversion is pending or after it fails, and the
item shows that state accurately. 'Bytes retained, extraction unsupported' is a fallback state,
not completion of supported-format ingestion. No fake success, empty digest or text-only
substitute may stand in for the converter." SPEC §4: "A digest records a conversion of
particular bytes with a particular converter and settings." Owner, 2026-10-07: "a partly
converted file stays readable, names its unconverted pages, is listed as unprocessed, and a
retry converts only those pages."

### Library facts

- Docling reports `ConversionStatus::{Success, PartialSuccess, Failure}` and, for a document it
  produced, `ErrorItem { component_type, module_name, error_message }`; any `ErrorItem` makes the
  result `PartialSuccess` [verified: docling-1.93.5/src/result.rs:10-27, 50-56].
- Errors that produce no document: `Io`, `UnknownFormat`, `UnsupportedFormat`, `Parse`,
  `Streaming`, `Browser`, `Panic`, `Timeout` (streaming only), `WithSource`
  [verified: docling-1.93.5/src/error.rs:9-48].
- A document budget stops between pages and keeps the finished pages as a `PartialSuccess`; a
  page in flight finishes first [verified: docling-1.93.5/src/converter.rs:259-277].
- A truncated PDF is refused: `convert` returns `Err` or `Ok` with `Failure`
  [verified: qualification/docling/lib/criteria.mjs:87-88].
- The model set is reported by `docling::model_inventory()` as `ModelEntry { stage, path, found,
  bytes }`, with no hash [verified: docling.rs@e500c23/crates/docling-pdf/src/lib.rs:372-383]; the qualification
  hashes the files itself. The converter's crate sources come from Cargo.lock, as the receipt's
  `converter.packages` records them (docling 1.93.5 from crates.io; docling-core, docling-onnx and
  docling-pdf 1.93.6 from the fork) [verified: qualification/receipts/docling.json:602-634].
- A missing OCR model degrades to no OCR with a warning, not a failure
  [verified: docling-1.93.5/src/converter.rs:678-682]. So a missing asset never shows up as a
  document outcome; it must be refused by the worker (section 9.1) and by readiness.

### Types (`crates/contract/src/extraction.rs`)

```rust
/// The converter's verdict on one source, as a listing and a filter use it.
pub enum ExtractionStatus {
    /// Accepted; no conversion has finished.
    Pending,
    /// Every page, or the whole non-paginated document, was converted.
    Completed,
    /// A document was produced but part of the source is missing from it.
    Partial,
    /// The converter produced no document; the bytes are retained.
    Failed,
    /// No converter exists for this format; the bytes are retained.
    Unsupported,
}

/// What the converter did; supplied text and corrections never change it.
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionOutcome {
    /// Accepted; the card was committed before conversion and shows no text.
    Pending,
    /// Docling's `Success`.
    Completed,
    /// Docling's `PartialSuccess`, or a windowed conversion with windows not converted.
    Partial {
        /// Pages converted and not converted; absent for a format docling converts whole.
        coverage: Option<PageCoverage>, // opt
        /// Why it is partial, as the converter recorded it.
        issues: Vec<ConverterIssue>,
    },
    /// No document was produced.
    Failed {
        /// Typed cause; never the converter's raw text alone.
        reason: FailureReason,
        /// What the converter recorded, when it recorded anything.
        issues: Vec<ConverterIssue>,
    },
    /// No converter handles this format (docling `UnknownFormat` or `UnsupportedFormat`).
    Unsupported,
}

/// Page by page coverage of a source converted in page windows (a PDF; an image is one page).
/// Formats docling converts whole (Office, HTML) have no coverage.
///
/// The three lists are ascending, disjoint, and together cover exactly `1..=page_count`.
pub struct PageCoverage {
    /// Pages in the original.
    #[schemars(range(min = 1))]
    pub page_count: u32,
    /// Pages converted with no known loss.
    pub converted: Vec<crate::common::PageRange>,
    /// Pages converted whose text is partly undecodable (`ExtractionWarning::UndecodableGlyphs`).
    pub partly_extracted: Vec<crate::common::PageRange>,
    /// Pages with no extraction; a retry converts exactly these.
    pub not_converted: Vec<crate::common::PageRange>,
}

/// Why a conversion produced no document.
pub enum FailureReason {           // tag "kind"
    /// The converter refused the bytes as unreadable: truncated, corrupt, or not what the
    /// name says (docling `Parse`, `WithSource`, or `Failure`).
    Damaged,
    /// The worker reached its memory cap before any page converted.
    MemoryLimit {
        /// The cap, in bytes, as a decimal string.
        limit_bytes: String,
    },
    /// The worker's hard time bound ended it before any page converted.
    TimeLimit {
        /// The bound, in whole seconds.
        limit_seconds: u32,
    },
    /// The converter process ended abnormally (docling `Panic`, an abort, a signal).
    ConverterCrashed,
    /// Any other converter error (docling `Io`, `Streaming`, `Browser`).
    ConverterError,
}

/// Docling's `ErrorItem`, with its own field names.
pub struct ConverterIssue {
    /// `document_backend`, `model`, `doc_assembler` or `user_input`.
    pub component_type: String,
    /// Stage that recorded it, such as `pipeline`.
    pub module_name: String,
    /// The converter's message, with every worker filesystem path reduced to its file name.
    pub error_message: String,
}

/// The converter that produced a digest; part of the digest's identity (SPEC §4).
pub struct ConverterIdentity {
    /// `docling`.
    pub name: String,
    /// The docling crate version, such as `1.93.5`.
    pub version: String,
    /// The converter crates as Cargo.lock resolves them; a patched fork is visible here.
    /// Generated from Cargo.lock by xtask (section 9.1), so gen-check holds it to the lock.
    pub packages: Vec<ConverterPackage>,
    /// Settings applied.
    pub settings: ConversionSettings,
    /// Model files the pipeline loads, hashed once when the worker starts; empty for formats
    /// that load none.
    pub models: Vec<ModelIdentity>,
    /// Pages per conversion window; absent when the document was converted whole.
    pub page_window: Option<u32>, // opt
}

/// One converter crate, as Cargo.lock records it.
pub struct ConverterPackage {
    /// Crate name, such as `docling-pdf`.
    pub name: String,
    /// Crate version.
    pub version: String,
    /// Cargo.lock `source`, such as `git+https://github.com/Heyoub/docling.rs?rev=...`.
    pub source: String,
}

/// One model file of `docling::model_inventory()`, hashed by the worker at startup.
pub struct ModelIdentity {
    /// Pipeline stage, such as `layout` or `ocr.rec`.
    pub stage: String,
    /// File name only; never a filesystem path.
    pub file: String,
    /// Byte count as a decimal string.
    pub size: String,
    /// SHA-256 of the file as loaded.
    pub sha256: crate::identity::Digest,
}

/// Findings that leave a document usable but lessen what it says.
pub enum ExtractionWarning {       // tag "kind"
    /// Glyphs the PDF's fonts give no Unicode for; detected, not recovered, never indexed as words.
    UndecodableGlyphs {
        /// Pages holding placeholders, ascending.
        pages: Vec<PageGlyphs>,
        /// Placeholders in items that have no page.
        unlocated_glyphs: u32,
    },
    /// Items located through the page text layer instead of by the converter.
    InferredLocations {
        /// How many.
        items: u32,
    },
    /// Items with no location in the original.
    UnlocatedItems {
        /// How many.
        items: u32,
    },
    /// A spreadsheet's cells were read as the values the file stores; formulas and number
    /// formats were not extracted (SPEC §5 formula/value/format distinction).
    CellValuesOnly {
        /// Sheets read, in workbook order.
        sheets: Vec<String>,
    },
}

/// Undecodable glyphs on one page.
pub struct PageGlyphs {
    /// One-based page.
    #[schemars(range(min = 1))]
    pub page_no: u32,
    /// Placeholders the library printed on that page.
    pub glyphs: u32,
}

/// Whose text a source card shows and search indexes.
pub enum TextOrigin {
    /// The converter's extraction.
    Converter,
    /// Text an agent proposed and a person accepted (section 5).
    SuppliedByAgent,
    /// No text: pending, failed or unsupported, with nothing supplied.
    None,
}

/// What a listing shows about one source.
pub struct ExtractionSummary {
    /// The converter's verdict.
    pub status: ExtractionStatus,
    /// Whose text is shown.
    pub text_origin: TextOrigin,
    /// A human correction is shown over that text.
    pub corrected: bool,
}

/// Everything recorded about turning one source occurrence's bytes into text.
pub struct Extraction {
    /// What the converter did.
    pub outcome: ConversionOutcome,
    /// Present exactly when a converter ran (not for `pending` or `unsupported`).
    pub converter: Option<ConverterIdentity>, // opt
    /// The retained conversion record (section 9.1); present exactly when a document was
    /// produced (`completed`, `partial`).
    pub digest: Option<crate::identity::Digest>, // opt
    /// Pages in the original, for a paginated format.
    pub page_count: Option<u32>, // opt
    /// Whose text is shown.
    pub text_origin: TextOrigin,
    /// A human correction is shown over that text.
    pub corrected: bool,
    /// The latest accepted agent-supplied text, shown or kept in history (section 5).
    pub supplied: Option<SuppliedText>, // opt
    /// Findings of the conversion.
    pub warnings: Vec<ExtractionWarning>,
}
```

Functions in the same file: `ConversionOutcome::status(&self) -> ExtractionStatus`;
`ExtractionStatus::is_unprocessed(self) -> bool` (true for `partial`, `failed`, `unsupported`);
`PageCoverage::check(&self) -> Result<(), ApiError>` (the ordering, disjointness and cover
rule above, `invalid_input` naming the field); `Extraction::summary(&self) -> ExtractionSummary`.

Docling to outcome, applied by ingest:

| Docling result | Outcome |
| --- | --- |
| `Ok`, `Success` | `completed` |
| `Ok`, `PartialSuccess` | `partial`, issues from `errors`, coverage from the pages the document holds |
| `Ok`, `Failure`; `Err(Parse)`; `Err(WithSource)` | `failed`, `damaged` |
| `Err(Panic)`; process killed by a signal or abort | `failed`, `converter_crashed` |
| child process over its memory cap | `failed`, `memory_limit` if no window converted, else `partial` |
| supervisor time bound | `failed`, `time_limit` if no window converted, else `partial` |
| `Err(Io)`, `Err(Streaming)`, `Err(Browser)` | `failed`, `converter_error` |
| `Err(UnknownFormat)`, `Err(UnsupportedFormat)`, no extractor for the media type | `unsupported` |
| undecodable glyphs found on a page of a `Success` | `partial`; that page in `partly_extracted` |
| any XLSX result with a document | the outcome above, plus warning `cell_values_only` |
| model asset or native library missing | not an outcome: the worker errs, the job fails retryable, the card stays `pending` |

### Operations whose shapes change

- `ItemSummary` gains `extraction: Option<ExtractionSummary>` (opt), present exactly when `kind`
  is `source`. It reaches `list_items`, `get_graph` and, through `ItemDocument`, `get_item` and
  `create_item`.
- `SourceAppearance` gains `extraction: Extraction` (required; it exists only for a source). It
  reaches `get_item` and `create_item` through `ItemDocument::source`, and `get_sources`. The
  converter identity is therefore on `SourceAppearance` and `ItemDocument`, not on
  `ItemSummary`, which carries only the summary.
- `ReadItemResponse` gains `extraction: Option<ExtractionSummary>` (opt), present for a source,
  so an agent reading through `show` is told when the text is partial or supplied.
- `AttentionKind::Extraction` is replaced by `Unprocessed` (a source whose status is unprocessed;
  action `redigest_item`) and `ExtractionWarnings` (a completed source with warnings; action
  `read_item`). A warning and a failure are different observations.
- No operation is added.

### Contract tests

- `semantic::conversion_outcomes_are_tagged_by_status_and_closed`: each variant round-trips; an
  unknown status or field is refused.
- `semantic::page_coverage_covers_every_page_once`: `check` accepts a partition of
  `1..=page_count` and refuses overlap, a gap, a page beyond the count and descending ranges.
- `semantic::unprocessed_means_partial_failed_or_unsupported`: `is_unprocessed` over all five.
- `semantic::a_failure_reason_is_typed`: the `FailureReason` schema is a closed `kind` union with
  no free message field.
- `schema::every_request_and_response_has_one_wire_shape` gains `Extraction` and
  `ConverterIdentity` in its explicit list.

### Deliberately left out

Per-page OCR confidence: docling has a per-page `ocr_score` in its `ConfidenceReport`
[verified: docling.rs@e500c23/crates/docling-core/src/confidence.rs:1-21] and no per-item OCR flag in the export
[inferred: no `from_ocr` field appears in docling-pdf 1.93.6]; nothing consumes it yet. Sheets the
XLSX backend skips past `DOCLING_RS_SHEET_MAX_CELLS` are reported only on stderr
[verified: docling-1.93.5/src/backend/xlsx.rs:66-72]; no typed warning is designed until ingest can
observe it other than by parsing stderr.

Formula versus value (SPEC §5 "Formula/value/format distinctions and extraction limitations must
be visible where they matter"): the XLSX backend reads cell values and has no formula or number
format handling [verified: docling-1.93.5/src/backend/xlsx.rs, no occurrence of `formula` or a
number-format reader]. Ingest therefore adds `cell_values_only` to every XLSX extraction, and a
dataset read from cells carries the warning (section 4), which is where the distinction matters.
Extracting the formulas themselves is not designed: the converter does not give them.

## 2. Source locators and location provenance

SPEC §5: "Every source location records its provenance: direct from the document
representation, inferred by matching text or layout, or unresolved. The interface never presents
an inferred location with the certainty of a direct one." SPEC §3: "Source references retain
workspace, stable item identity, resolved relative path, revision, digest where relevant, and
selection." SPEC §5: "Retain structured Markdown, images/captions, relevant page renders, source
locators and converter/version/settings."

### Library facts, by format

| Format | What docling gives | Locator here |
| --- | --- | --- |
| PDF | `prov` per item: `page_no`, `bbox {l,t,r,b,coord_origin}`, `charspan`; captions lose their box [verified: qualification/docling/src/locate.rs:3-9, 228-242] | `region` (in PDF points [inferred]), `direct`; or `inferred` by the text-layer rule; or `unresolved` with the rule's reason |
| Image | one page; every item needs a page and a box, and the fixture's picture is located by the export [verified: qualification/docling/lib/criteria.mjs:52; qualification/receipts/docling.json:84-87] | `region` (in pixels [inferred]), `direct` |
| PPTX | `page_no` is the slide; the box is in EMU and the page size is the slide size in EMU; top-left origin [verified: docling.rs@e500c23/crates/docling-core/src/tree.rs:117-133]; 26 of 28 items located in the corpus deck [verified: qualification/receipts/docling.json:45] | `region` (in EMU), `direct`; an item with no `prov` is `unresolved`, `not_located_by_converter` |
| XLSX | every sheet is a page in workbook order; the box is the item's cell-index box, top-left origin, half-open on the right and bottom [verified: docling-1.93.5/src/backend/xlsx.rs:137-139, 268-271, 309-311, 370-381] | `cells`: sheet name from the page number's position in workbook order, rows and columns converted to one-based inclusive, `direct` |
| DOCX, HTML | `prov: []` [verified: docling.rs@e500c23/crates/docling-core/src/tree.rs:163-165] | `unresolved`, `format_has_no_locator` |
| OCR text (scanned PDF, image) | recognition runs per layout region, so recognised text arrives as ordinary items with the region's box [verified: docling.rs@e500c23/crates/docling-pdf/src/ocr.rs:1-6] | the same `region` at item granularity; no word or line boxes exist in the export |

### Types (`crates/contract/src/source.rs`, with `Selection` in `read.rs`)

```rust
/// Docling's `coord_origin`: which corner `t` and `b` are measured from.
pub enum CoordOrigin {
    /// Docling `TOPLEFT`.
    TopLeft,
    /// Docling `BOTTOMLEFT`.
    BottomLeft,
}

/// Docling's bounding box, copied as the converter gave it (rounded to two decimals).
pub struct BoundingBox {
    /// Left edge.
    pub l: f64,
    /// Top edge.
    pub t: f64,
    /// Right edge.
    pub r: f64,
    /// Bottom edge.
    pub b: f64,
    /// Corner `t` and `b` are measured from.
    pub coord_origin: CoordOrigin,
}

/// Extent of the page the box lies on, in the box's own unit (PDF points, image pixels, PPTX
/// EMU). A reader draws the box over a page image by the ratio of the two, so it needs no unit.
pub struct PageSize {
    /// Page width.
    pub width: f64,
    /// Page height.
    pub height: f64,
}

/// A box on one page, slide or image of the original.
pub struct PageRegion {
    /// One-based page or slide number.
    #[schemars(range(min = 1))]
    pub page_no: u32,
    /// The box.
    pub bbox: BoundingBox,
    /// The page's extent, so the box can be drawn over a page image on its own.
    pub page_size: PageSize,
}

/// Where in the original bytes something is.
pub enum SourceLocator {           // tag "kind"
    /// A whole page or slide.
    Page {
        /// One-based page or slide number.
        #[schemars(range(min = 1))]
        page_no: u32,
    },
    /// A box on a page, slide or image.
    Region {
        /// The box.
        region: PageRegion,
    },
    /// Spreadsheet cells.
    Cells {
        /// Sheet and one-based inclusive cell range.
        range: crate::common::CellRange,
    },
}

/// A location and how it was obtained; an inferred location is a different variant from a
/// direct one, so no consumer can show it as direct by accident.
#[serde(tag = "provenance", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceLocation {
    /// From the converter's own provenance.
    Direct {
        /// Where.
        locator: SourceLocator,
    },
    /// Found by matching text against the page text layer (ingest-locates-unlocated-items).
    Inferred {
        /// Where.
        locator: SourceLocator,
    },
    /// Not known; never given a guessed box.
    Unresolved {
        /// Why.
        reason: UnresolvedReason,
    },
}

/// Why a location is not known; one variant per reason the producing rule states.
pub enum UnresolvedReason {        // tag "kind"
    /// The format has no page geometry (DOCX, HTML, Markdown).
    FormatHasNoLocator,
    /// The converter gave this item no location and no rule locates it (PPTX, XLSX).
    NotLocatedByConverter,
    /// The page text layer could not be read.
    NoTextLayer,
    /// A table or picture: there is no text to look up.
    NoTextToMatch,
    /// Neither the parent nor an earlier sibling is located.
    NoPageToSearch,
    /// No text-layer item on the searched pages has exactly this text.
    NoMatch,
    /// Several text-layer items have exactly this text.
    AmbiguousMatch {
        /// How many.
        occurrences: u32,
    },
    /// The one match has no area or lies outside the page.
    MatchNotALocation,
    /// The cited text is a human correction, which has no place in the original.
    CorrectedText,
    /// The cited text was supplied by an agent; its page claims are not locations.
    SuppliedText,
}
```

The unresolved reasons are the reasons `locate.rs` gives, typed
[verified: qualification/docling/src/locate.rs:344-397].

Changes to existing types:

- `Selection` (`read.rs`) gains `Region { region: crate::source::PageRegion }`. It loses `Eq`
  (a box holds `f64`) and keeps `PartialEq`; nothing derives `Eq` over it today. A read with a
  region selects the extracted items whose location lies inside the box; `pages` view returns
  that page's image.
- `CellRange` (`common.rs`) gains `#[schemars(range(min = 1))]` on its four bounds; it is the
  XLSX locator and is one-based inclusive.
- `SourceReference` gains `locations: Vec<SourceLocation>` (opt-list): where the cited selection
  of the digest lies in the original, one entry per located item it covers, in reading order.
  The rule, in one place: the server computes it from (item, revision, digest, selection). In a
  response it is always filled. In a request, an omitted list means "fill it" and the server
  fills it; a present list must equal what the server computes, or the request is
  `invalid_input` on `.../locations`. An agent therefore cannot supply a location. A saved View
  file does not persist locations: the server writes each binding's `SourceReference` without
  them (they are derived, and an item purge must not leave them in Git) and fills them when it
  returns the View. Because every citation type holds a `SourceReference`, locations reach
  `ReadItemResponse`, `SearchHit`, `MediaReference`, `ViewBinding`, `Review`, `Receipt`,
  `BlameResponse` and `AddCommentRequest`.

No operation is added.

### Contract tests

- `semantic::location_provenance_is_carried_by_the_tag`: each of the three variants serializes
  with exactly `"provenance":"direct"`, `"inferred"` or `"unresolved"`, and each of those three
  documents deserializes to that variant and no other; it fails if a variant is renamed,
  aliased, or made to share a tag.
- `semantic::an_unresolved_location_has_no_locator`: a `locator` field beside
  `"provenance":"unresolved"` is refused.
- `semantic::a_region_keeps_docling_coordinates`: a PDF bottom-left and a PPTX top-left region
  round-trip unchanged.
- `semantic::cell_ranges_are_one_based`: the `CellRange` schema states minimum 1 on every bound.
- `semantic::citation_locations_are_omitted_when_empty_without_changing_semantics`: omitted,
  `[]` and absent all read as no locations; an empty list is not written.

### Deliberately left out

A distance bound on inferred locations: the owner's decision is open until
`ingest-locates-unlocated-items` is built, and the measurement stays in the ingest lane. Word or
line boxes for OCR text: the export does not carry them. Character spans (`charspan`): nothing
consumes them.

## 3. Asset roles and import settings

SPEC §5: "Retain structured Markdown, images/captions, relevant page renders, source locators and
converter/version/settings." "Heavy conversion runs in bounded workers with time, memory and
cancellation controls."

### Library facts

- Docling produces two kinds of image: page images, kept per page when `generate_page_images` is
  on, in `DoclingDocument::page_images` keyed by page number
  [verified: docling-1.93.5/src/converter.rs:750-759; docling.rs@e500c23/crates/docling-core/src/document.rs:58];
  and a picture item's own image [verified: docling.rs@e500c23/crates/docling-core/src/tree.rs:87-105]. Page images
  exist for the PDF and image pipeline only [verified: qualification/receipts/docling.json:35].
  Nothing else is an image asset, so there are two roles.
- `PictureImage` carries `mimetype`, `width`, `height`, `data`, `dpi`
  [verified: docling.rs@e500c23/crates/docling-core/src/document.rs:655-669]; page renders are 144 dpi unless
  `images_scale` is set (dpi = 72 x scale) [verified: docling-1.93.5/src/converter.rs:738-748].
- Settings that change the result: `ocr_lang` [verified: converter.rs:289], `skip_ocr`
  [verified: converter.rs:668-685], `force_full_page_ocr` [verified: converter.rs:687-697],
  `no_table_former` [verified: converter.rs:622-632], `generate_page_images`, `images_scale`.

### Types

In `crates/contract/src/extraction.rs` (moved from core `conversion.rs`, so the wire and the job
carry one definition):

```rust
/// When optical character recognition runs.
pub enum OcrPolicy {
    /// Recognise only regions with no embedded text layer (docling default).
    Auto,
    /// Never run recognition; layout and tables are still detected (docling `skip_ocr`).
    Skip,
    /// Recognise every page from its render (docling `force_full_page_ocr`).
    ForceFullPage,
}

/// Converter settings; part of the identity of the digest they produce.
#[derive(PartialEq, Eq)]
pub struct ConversionSettings {
    /// When recognition runs.
    pub ocr: OcrPolicy,
    /// Docling `ocr_lang`, such as `en` or `ch`; absent uses docling's default (`en`).
    pub ocr_language: Option<String>, // opt
    /// Reconstruct table structure with the model (docling `no_table_former` off).
    pub table_structure: bool,
    /// Keep a render of every page (docling `generate_page_images`).
    pub page_images: bool,
    /// Render resolution; docling `images_scale` = this / 72. At most 144, docling's own render
    /// resolution: a higher scale only upsamples that render and adds no detail.
    #[schemars(range(min = 36, max = 144))]
    pub page_image_dpi: u16,
}
```

The cap is 144 because docling renders a page once at 2.0 px/pt (144 dpi) and resamples to any
other `images_scale`; above 2.0 it upsamples and does not re-render
[verified: docling-1.93.5/src/converter.rs:739-744]. A larger value would cost memory and
storage and claim detail that is not there. `ConversionSettings::check` refuses a value outside
36..=144 as `invalid_input` on `/settings/page_image_dpi`.

`impl Default for ConversionSettings` (Rust only, not a serde default): `ocr: Auto`,
`ocr_language: None`, `table_structure: true`, `page_images: true`, `page_image_dpi: 144`.
`page_images` changes from `false` to `true`: page renders are what a `region` citation is drawn
on (section 2) and what SPEC §5 asks to retain; page windows (section 9.1) bound the memory.

In `crates/contract/src/read.rs`:

```rust
/// What a retained image is.
pub enum AssetRole {
    /// A render of one whole page (docling page image).
    PageImage,
    /// A picture item's own image (docling `PictureItem` image).
    Picture,
}
```

`MediaReference` gains `role: AssetRole`. Its `caption: String` and `caption_origin: String`
become one field, `caption: Option<String>` (opt): the document's own caption or alternative
text, absent when the document gives none. The origin word is dropped because only one origin
has a producer (ingest copies the docling caption item); `process`, `human` and `agent` captions
would need caption generation or a caption-editing operation, and neither is selected. Core's
`CaptionOrigin` and `AssetCaption::origin` go with it. Its `source` locates the image: a page
image's location is `direct` `page`; a picture's is its region.

### Operations whose shapes change

- `StartImportRequest` gains `settings: Option<ConversionSettings>` (opt); absent means the
  deployment's defaults, resolved into the job at acceptance so a retry converts with the same
  settings.
- `RedigestRequest.settings` changes from `BTreeMap<String, Value>` to `ConversionSettings`, and
  the request gains `unconverted_only: bool`: when true the job converts exactly the
  `not_converted` pages of the current extraction and merges them into a new digest; the settings
  must then equal the current digest's settings, or the request is `invalid_input` on
  `/settings`. This is the owner's "a retry converts only those pages".
- `JobSpec::Import` gains `settings` (section 9.2). No operation is added.

### Contract tests

- `schema::every_request_and_response_has_one_wire_shape` gains `ConversionSettings`.
- `semantic::asset_roles_are_page_image_and_picture`: the closed set.
- `semantic::import_settings_are_optional_and_redigest_settings_typed`: a start-import request
  without `settings` parses; a redigest request with an unknown settings key is refused.

### Deliberately left out

`ocr_engine` (Tesseract), `ocr_mode`, `ocr_scale`, picture classification and enrichment: none is
selected for the product. The converter worker must clear the `DOCLING_*` environment variables
that change output, so the recorded settings are the effective ones; that is ingest work.

## 4. A View's materialized dataset and per-chart status

SPEC §10: "Bindings retain document, resolved revision, digest, selection, units and
transformations. A materialized dataset may be retained in the blob store. Every chart has an
accessible data-table representation. A chart that fails to render shows its own alert; the
View's text and other charts still render." "Source text and numeric datasets must resolve from
cited application data, not text the agent labels as original." Owner, 2026-10-05: "no chart
reads agent-supplied numbers before a person approves them."

Today the dataset exists only as a Zod schema in `ui/src/features/views/PresentView.tsx:20-22`
(an array of records of string, number, boolean or null, at most 100 000 rows), and nothing
produces it.

### Types (`crates/contract/src/views.rs`)

```rust
/// The data kind of one column.
pub enum ColumnKind {
    /// Text.
    String,
    /// A real number.
    Number,
    /// A whole number.
    Integer,
    /// True or false.
    Boolean,
    /// A UTC instant in RFC 3339; an ambiguous document date stays a `string` (SPEC §3).
    DateTime,
}

/// One column of a dataset.
pub struct DatasetColumn {
    /// Field name a chart encodes.
    pub name: String,
    /// Data kind.
    pub kind: ColumnKind,
    /// Unit, such as `EUR` or `kg`.
    pub unit: Option<String>, // opt
}

/// One cell: a JSON scalar of its column's kind, or null. Untagged because a tag per cell would
/// multiply the dataset's size; the only untagged enum on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DatasetValue {
    /// Missing.
    Null,
    /// A boolean.
    Boolean(bool),
    /// A number, kept exact.
    Number(serde_json::Number),
    /// Text, or an RFC 3339 instant for a `date_time` column.
    Text(String),
}

/// A typed table resolved from cited application data; its canonical JSON bytes are the blob
/// `ViewBinding::materialized` names.
pub struct Dataset {
    /// Format version, currently 1.
    pub schema_version: u32,
    /// The exact citation the rows were read from.
    pub source: crate::source::SourceReference,
    /// Whose text the values were read from.
    pub text_origin: crate::extraction::TextOrigin,
    /// Columns, in order.
    #[schemars(length(min = 1, max = 1024))]
    pub columns: Vec<DatasetColumn>,
    /// Rows; each has exactly one value per column.
    #[schemars(length(max = 100000))]
    pub rows: Vec<Vec<DatasetValue>>,
    /// Extraction warnings of the source that bear on these values, such as
    /// `cell_values_only` for a dataset read from spreadsheet cells.
    pub warnings: Vec<crate::extraction::ExtractionWarning>, // opt-list
}

/// Which chart of a View a result is about.
pub enum ChartRef {                // tag "kind"
    /// The View's own `spec`, for `vega_lite` grammar.
    Spec,
    /// A named entry of `ViewDocument::charts`, for `json_render` grammar.
    Named {
        /// The key in `charts`.
        name: String,
    },
}

/// Why one chart cannot be drawn.
pub enum ChartFailure {
    /// The Vega-Lite specification fails server validation.
    InvalidSpec,
    /// The chart reads a dataset name no binding declares.
    UnknownBinding,
    /// A binding's data could not be resolved or materialized.
    DatasetUnavailable,
    /// A binding cites purged content (section 6).
    Invalidated,
    /// The dataset exceeds the row or byte limit.
    TooLarge,
}

/// The state of one chart in a present or resolve result.
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChartStatus {
    /// Every dataset it reads is materialized.
    Ready {
        /// Binding names it reads.
        bindings: Vec<String>,
    },
    /// It cannot be drawn; the View's text and other charts are unaffected.
    Failed {
        /// Typed cause.
        reason: ChartFailure,
        /// The binding at fault, when one is.
        binding: Option<String>, // opt
        /// Safe explanation for the chart's own alert.
        message: String,
    },
}

/// One chart and its state.
pub struct ChartResult {
    /// Which chart.
    pub chart: ChartRef,
    /// Its state.
    pub status: ChartStatus,
}
```

Functions: `Dataset::check(&self) -> Result<(), ApiError>` (row width equals column count; each
value is null or of its column's kind; `date_time` values parse as `Timestamp`);
`ViewDocument::chart_refs(&self) -> Vec<ChartRef>` (`[Spec]` for `vega_lite`, one `Named` per
`charts` key for `json_render`).

### Operations whose shapes change

- `PresentResponse` (returned by `present_view` and `resolve_view`) gains
  `charts: Vec<ChartResult>`: exactly one per entry of `view.chart_refs()`, in that order. A
  View-level fault (schema, a binding outside the workspace) still refuses the whole request; a
  chart-level fault never does. `resolved_bindings[i].materialized` is absent for a binding whose
  dataset could not be produced, and every chart reading it is `failed`.
- `PresentResponse` gains `as_of: Timestamp`: when the bindings were resolved. With the resolved
  revision every binding's `source.revision` already carries, this is SPEC §10's "Explicit live
  mode reports each resolved revision and as-of time". A View binds only to its own workspace,
  so one resolution moment covers every binding; for a pinned View `as_of` is the time the
  pinned inputs were read, and the revisions are the pinned ones.
- The dataset is a derived object of the bound source. Core produces it while resolving
  (section 9.5), writes it to the blob store, and records it with
  `RecordStore::record_derived_object` against the binding's (item, revision). Because of that
  record: `get_object` with `source` = the binding's citation accepts the dataset's digest as an
  object of that item revision (the same check that serves originals and conversion assets);
  the recorded object is a garbage-collection root; and an item or workspace purge reaches it as
  a derivative of the item (SPEC §8 "derivatives"). Writing a content-addressed derived blob and
  its record during a `Read` operation is not a change of user state, as an index entry is not.
  The UI fetches the dataset through `read_object` as today, verifies the digest, and parses it
  with the generated `zDataset`.
- `xtask` registers `Dataset` as an OpenAPI component (it is in no operation's schema, being
  fetched as bytes) and writes `api/forms/dataset.schema.json`, so Hey API generates `zDataset`.
- No operation is added.

### Contract tests

- `semantic::a_dataset_row_has_one_value_per_column`: `check` refuses a short row, a value of the
  wrong kind and an unparsable `date_time`.
- `schema::every_request_and_response_has_one_wire_shape` gains `Dataset`.
- `semantic::every_chart_of_a_view_has_one_ref`: `chart_refs` for both grammars.
- `semantic::a_failed_chart_states_a_typed_reason`: `ChartStatus` round trip; an unknown reason
  is refused.
- Core (core-cli): `views::a_materialized_dataset_is_an_object_of_the_bound_revision`: after
  `present_view`, `get_object` with the binding's citation serves the dataset's digest, and with
  the citation of another revision of the same item it is `not_found`.

### Deliberately left out

The rendered image of a chart for `export_view`.

## 5. Unprocessed files and agent-supplied text

Owner, 2026-10-05: "unprocessed files (failed, unsupported or partial) are listable by people and
by a connected agent, and an agent may propose extracted text as its own proposal kind labelled as
supplied by an agent, applied only on approval"; "when a later re-extraction succeeds for a file
whose text an agent supplied, the converter's text is shown and indexed by default, the agent's
text stays in history and a person may keep it". SPEC §5: "Generated extraction and human
corrections remain distinguishable." "AI enrichment occurs only when the user's external agent
proposes it, with truthful origin labels." SPEC §8: "Proposals ... cannot set the review identity
or turn an unapproved proposal into a fact."

### Types

In `crates/contract/src/extraction.rs`:

```rust
/// A filter over sources by extraction.
///
/// One variant on purpose: a defaulted `bool` would split the one wire shape, and `Option<bool>`
/// would have a third state with no meaning.
pub enum ExtractionFilter {
    /// Sources whose converter status is `partial`, `failed` or `unsupported`, whether or not
    /// text was supplied for them.
    Unprocessed,
}

/// Accepted agent-supplied text of one source.
pub struct SuppliedText {
    /// The accepted proposal.
    pub proposal_id: crate::identity::ProposalId,
    /// Subject of the agent that proposed it, recorded by the server.
    pub supplied_by: String,
    /// OAuth or connector client it came through.
    pub client_id: Option<String>, // opt
    /// Subject who accepted the proposal.
    pub approved_by: String,
    /// When it was accepted.
    pub approved_at: crate::identity::Timestamp,
    /// SHA-256 of the supplied Markdown.
    pub content_digest: crate::identity::Digest,
    /// Pages the agent says the text covers: a claim, never a location; empty for the whole.
    pub pages: Vec<crate::common::PageRange>,
    /// Who chose this text over a later complete conversion.
    pub kept_by: Option<String>, // opt
}
```

In `crates/contract/src/proposal.rs`:

```rust
pub enum Change {
    // ... Create, Edit, Move, Archive unchanged ...
    /// Propose the complete text of an unprocessed source.
    SupplyExtraction {
        #[doc = "Source item."]
        item_id: crate::identity::ItemId,
        #[doc = "Digest the agent saw; absent when the source has none. A different current digest is a conflict."]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        based_on: Option<crate::identity::Digest>,
        #[doc = "Pages the text covers, as the agent claims; empty for the whole."]
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pages: Vec<crate::common::PageRange>,
        #[doc = "The complete proposed text, Markdown."]
        markdown: String,
    },
}

/// What a proposal does; derived from its changes, never chosen by the proposer.
pub enum ProposalKind {
    /// Content changes: create, edit, move, archive.
    Content,
    /// Agent-supplied extraction text.
    SupplyExtraction,
}
```

`ProposalKind::of(changes: &[Change]) -> Result<ProposalKind, ApiError>`: `invalid_input` on
`/changes` when supply changes are mixed with others or one proposal supplies the same item twice.

`Proposal` gains `kind: ProposalKind`, `created_via: crate::access::AccessRoute` and
`client_id: Option<String>` (opt), all server-set. `created_at` becomes `Timestamp` (section 7).

### Rules

1. `open_proposal` with a supply change is accepted only from the `mcp_delegation` route (a
   person corrects text with `correct_digest`), only for a source whose status is unprocessed,
   and only when `based_on` equals the current digest. Otherwise `invalid_input` or `conflict`.
2. Accepting it commits the text beside the source card, in Git (`TreeEdit::SupplyExtraction`,
   section 9.3), so the search index rebuilds from Git alone. The converter outcome is unchanged.
3. Shown and indexed text, decided by core, in order: a human correction of the shown text; the
   supplied text when the source has one and no `completed` conversion was recorded after it,
   or when a person kept it; otherwise the converter text. `text_origin` says which.
4. A person keeps supplied text after a later complete conversion with `correct_digest`, whose
   request gains `adopts: Option<ProposalId>` (opt): the corrected Markdown must equal the
   accepted supplied text of that proposal (compared by digest), and the result sets
   `SuppliedText::kept_by` with `text_origin: supplied_by_agent` instead of `corrected`.
5. A citation into supplied text has the location `unresolved`, `supplied_text`; a chart's
   dataset read from it has `text_origin: supplied_by_agent`. Supplied text exists only after
   acceptance, so no chart reads unapproved numbers.

### Operations whose shapes change

- `ListItemsRequest` gains `extraction: Option<ExtractionFilter>` (opt); with it, only sources
  that match are listed among the folder's items; child folders are listed as before.
- `SearchRequest` gains `extraction: Option<ExtractionFilter>` (opt). `query` may be empty only
  when a filter is set: then every matching source under `folder` is a hit, ordered by path, with
  its description as snippet. This is the workspace-wide listing for agents, since a failed file
  has no text for a query to match. `check_rules` refuses an empty query without a filter.
- `SearchHit` gains `extraction: Option<ExtractionSummary>` (opt), present for a source.
- `open_proposal`, `get_proposal`, `list_proposals`, `accept_proposal`: through `Change` and
  `Proposal`. `correct_digest`: `adopts`.
- New descriptions for the agent tools (operation table, exact text):
  - `list_items`: "List a folder with one-line descriptions at one resolved revision. With
    extraction = \"unprocessed\", list only the source files whose conversion is partial, failed
    or unsupported."
  - `search_items`: "Search authorized content and return cited snippets, not whole-document
    dumps. The query may be empty only with a filter; extraction = \"unprocessed\" lists every
    source file whose text the converter could not fully extract."
  - `open_proposal`: "Create a suggested change set without merging or marking anything
    reviewed. A `supply_extraction` change proposes the complete text of an unprocessed source
    file; it is applied only when a person accepts it and is labelled as supplied by an agent."
    (The identifier is in code style because core's `Application` trait takes each description
    as a doc comment and strict Clippy's `doc_markdown` refuses it bare.)
- No operation is added.

### Contract tests

- `semantic::the_unprocessed_filter_is_optional_on_listing_and_search`: both requests parse
  without it and refuse an unknown value.
- `scope::an_empty_search_query_needs_a_filter`: `check_rules`.
- `scope::a_supply_proposal_holds_only_supply_changes`: `check_rules` of `OpenProposalRequest`.
- `semantic::proposal_kind_is_derived_from_changes`: `ProposalKind::of` for each combination.
- `semantic::text_origin_words_are_converter_supplied_by_agent_and_none`.

### Deliberately left out

A per-connector proposal limit and the confirmation binding to a supply proposal's content
digest beyond what `accept_proposal` already binds. The switch that lets originals reach a
connected AI (owner: off by default) is not one of the eight shapes.

## 6. Archive, Purge and the invalidated state

SPEC §8: "Archive is reversible and keeps history. Purge is explicit and destructive: it removes
the original bytes, derivatives, index entries, historical content and installation-managed
backups of an item or a workspace, and marks the revisions, citations and Views that referred to
them as invalidated. Purge never claims to erase exports, clones or backups outside the
application's control. Workspace purge is built first; item purge follows with the same
guarantees." Ledger 2026-10-08: purge "eliminates recoverable target content from every
application-managed copy incl. managed backups and retained history; never completes while an
affected managed backup stays restorable; shared CAS objects with surviving legitimate references
are tracked honestly; no tombstone-only substitute." Ruling (owner Q2): "after item purge,
citations of other items whose content digest is unchanged are remapped to the rewritten revision
and keep review coverage; citations of the purged item are invalidated."

### Archive

- `Workspace` gains `archived_at: Option<Timestamp>` (opt).
- `ListWorkspacesRequest` gains `include_archived: Option<bool>` (opt); absent means false. It is
  optional so the `workspaces` model tool gains no required argument.
- New operation `unarchive_workspace` (`UnarchiveWorkspaceRequest { workspace_id,
  idempotency_key }`, response `MutationResult`).
- While a workspace is archived, an operation that would change its content or open work on it
  (a commit, a draft, a proposal, an upload, an import or redigest) is refused as `conflict`;
  reads, export, backup, `unarchive_workspace` and purge are allowed.
- `archive_workspace` stops being marked destructive: it destroys nothing.
- An item's archive is the `archived` flag of section 13 (decision E), set and cleared through
  `set_lifecycle`; it is reversible and keeps history.

### Types (`crates/contract/src/purge.rs`, `PurgeId` in `identity.rs`)

```rust
uuid_id!(PurgeId, "Identity of one purge; its record outlives what it removed.");

/// Permanently remove a workspace.
pub struct PurgeWorkspaceRequest {
    /// Workspace to remove.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Exact revision on which this purge is based; a moved head conflicts.
    pub base_revision: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Permanently remove one item, its history and its derivatives.
pub struct PurgeItemRequest {
    /// Workspace of the item.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Item to remove.
    pub item_id: crate::identity::ItemId,
    /// Exact revision on which this purge is based; a moved head conflicts.
    pub base_revision: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Read one purge record.
pub struct GetPurgeRequest {
    /// Purge identity.
    pub purge_id: crate::identity::PurgeId,
}

/// What a purge removes; identities only, never a name or a path.
pub enum PurgeTarget {             // tag "kind"
    /// A whole workspace.
    Workspace {
        /// Removed workspace.
        workspace_id: crate::identity::WorkspaceId,
    },
    /// One item.
    Item {
        /// Its workspace.
        workspace_id: crate::identity::WorkspaceId,
        /// Removed item.
        item_id: crate::identity::ItemId,
    },
}

/// Progress of a purge.
pub enum PurgeState {
    /// Accepted; nothing removed yet.
    Requested,
    /// Removing.
    Running,
    /// Every application-managed copy is removed or rewritten; no affected backup is restorable.
    Completed,
    /// Stopped; repeating the request resumes it.
    Failed,
}

/// What a completed purge did; counts only.
pub struct PurgeReport {
    /// Stored objects deleted.
    pub objects_removed: String,
    /// Objects kept because a surviving item still references them.
    pub objects_kept_shared: String,
    /// Managed backups deleted (workspace archives, and their copies in pre-migration backups).
    pub backups_removed: u32,
    /// Managed backups rewritten without the target.
    pub backups_rewritten: u32,
    /// Retained export and View export artifacts deleted.
    pub exports_removed: u32,
    /// Proposals deleted with their candidate references and comments.
    pub proposals_removed: u32,
    /// Drafts deleted.
    pub drafts_removed: u32,
    /// Revisions that no longer exist or were rewritten.
    pub revisions_invalidated: u32,
    /// Stored citations of other items remapped to a rewritten revision.
    pub citations_remapped: u32,
    /// Stored citations (reviews, receipts, comments, View bindings) of the target invalidated.
    pub citations_invalidated: u32,
    /// Saved Views with an invalidated binding.
    pub views_invalidated: u32,
}

/// A durable, tenant-level purge record; it holds no content of the target.
pub struct Purge {
    /// Identity.
    pub id: crate::identity::PurgeId,
    /// What is removed.
    pub target: PurgeTarget,
    /// Progress.
    pub state: PurgeState,
    /// The tenant job doing the work.
    pub job_id: crate::identity::JobId,
    /// Administrator subject.
    pub requested_by: String,
    /// When it was accepted.
    pub requested_at: crate::identity::Timestamp,
    /// When it completed.
    pub completed_at: Option<crate::identity::Timestamp>, // opt
    /// Present exactly when completed.
    pub report: Option<PurgeReport>, // opt
    /// Last failure.
    pub error: Option<crate::error::ApiError>, // opt
}
```

### Authorization, resumption and the job

- `purge_workspace`, `purge_item` and `get_purge` authorize at the tenant (`Deployment(Admin)`)
  only, and the two purges are in `HUMAN_SESSION_ONLY`. A purge removes the workspace's grants,
  so a workspace target could not authorize the retry of a half-finished purge; a tenant target
  can, before and after. A workspace administrator who is not a tenant administrator archives
  instead.
- A purge is unique per target while it is not completed: repeating `purge_workspace` or
  `purge_item` for that target (any idempotency key) returns the unfinished `Purge`, and its job
  is queued again; that is how a failed purge is resumed. The unfinished purge of the same target
  is found and returned before `base_revision` is checked (final-review decision 4), so a head
  that moved or no longer exists never blocks the resumption. A purge cannot be cancelled: a
  half-removed target is worse than either end.
- The work runs as a tenant job (section 9.2, `JobScope::Tenant`) of kind `purge_workspace` or
  `purge_item`; tenant jobs are not reachable through `retry_job` and `cancel_job`, which take a
  workspace.
- `purge_item` is declared now and returns `not_implemented` until its storage gate passes.

### What a purge reaches

Workspace purge removes, for that workspace: its Git repository; every stored object only it
references (originals, conversion records and their structured exports, page images, pictures,
recorded datasets, retained export, View export and backup archive bytes); its SQLite rows (jobs,
reviews, receipts, proposals with their candidate references, comments, drafts, uploads,
confirmations, sandbox capabilities, events, artifact records, derived-object records), except
its purge job row and the `Purge` record, which hold no content; its index entries; its grants;
its id in every connector's workspace list; its workspace archives; and its archive inside every
pre-migration backup storage retains (SPEC §2: "takes a recoverable backup before an upgrade
migration"), which is a managed backup too. An upload slot whose object is a backup archive is
not a surviving reference for this purpose. In other workspaces, a comment whose `source` cites
the purged workspace is marked invalidated and its stored path is scrubbed. Installation
archives hold no workspace content (section 8), so a purge neither deletes nor rewrites them;
the next installation archive carries the completed `Purge` record, and a replacement restore
refuses a workspace archive whose workspace that record says was purged.

Item purge removes the item's files from every commit (a history rewrite, below); its source
bytes and every derivative (conversion records, structured exports, page images, pictures, and
the datasets recorded against any of its revisions), unless a surviving item references the same
object; every proposal with a change touching it, including `SupplyExtraction` Markdown, with the
proposal's candidate reference and comments; every draft of it, for every editor; every retained
export or View export that contains it; and rewrites or deletes every workspace archive and
pre-migration backup that holds it. Comments anywhere that cite it, and reviews, receipts and saved
View bindings that cite it, are invalidated and scrubbed.

### Invalidated state and the revision map

- Item purge writes a durable revision map for the workspace (`RecordStore`, section 9.2): for
  every rewritten commit, old revision to new revision. A commit whose only change was the purged
  item has no rewrite and maps to the new revision of its parent. Recorded datasets (derived
  objects of other items' revisions) are mapped through the same durable map after an item purge
  (final-review decision 8; storage implements). Stored receipts, reviews and
  comments are not rewritten; the application resolves their revisions through the map when it
  reads them.
- A request that names a removed or rewritten revision is `not_found` with `ErrorDetail` gaining
  `Invalidated { purge_id: PurgeId, replacement: Option<Revision> }`: `replacement` is the map's
  new revision after an item purge and absent after a workspace purge. A client that does not
  know the detail still sees not-found, and one that does re-pins to the replacement.
- A stored citation of another item whose content digest at the new revision equals the one at
  the old revision is shown remapped to the new revision and keeps its review coverage. A stored
  citation of the purged item is invalidated: `ReviewCoverage` gains `Invalidated`; `Receipt` and
  `Comment` gain `invalidated_by: Option<PurgeId>` (opt); `ChartFailure::Invalidated` (section 4);
  `AttentionKind` gains `Invalidated` (a saved View or citation refers to purged content; action
  `get_view`).
- Scrubbing: the history rewrite removes the purged item's path and title from the messages of
  the rewritten commits (each occurrence becomes "a purged item"); the stored `path` of every
  invalidated citation is replaced by `purged/<item id>`, a valid `WorkspacePath` that names
  nothing. Views bind only to their own workspace (SPEC §10), so a workspace purge invalidates
  no View elsewhere.

### Operations

Added (rows in section 10): `unarchive_workspace`, `purge_workspace`, `get_purge`, `purge_item`.
`purge_workspace` and `purge_item` return `Purge` with status 202. `JobKind` gains
`purge_workspace` and `purge_item`.

Human-session rule, `crates/contract/src/operations.rs`, after `DRAFT_BEARING`:

```rust
/// Operations refused on every route that is not a human browser session, whatever the grants:
/// recovery and destruction belong to a person (ledger 2026-10-08), never to an agent or a
/// service identity. In table order.
pub const HUMAN_SESSION_ONLY: &[OperationName] = &[
    OperationName::PurgeWorkspace,
    OperationName::BackupWorkspace,
    OperationName::RestoreWorkspace,
    OperationName::BackupInstallation,
    OperationName::PurgeItem,
];
```

The same rule reaches jobs those operations started. `retry_job` and `cancel_job` take `write`
on the workspace, which would let a writer retry a backup or restore job on any route. So
`JobKind` gains `started_by(self) -> OperationName` (the operation that creates a job of that
kind), and the application, after loading the job, requires the table permission of `started_by`
fresh from `AccessControl` and, when `started_by` is in `HUMAN_SESSION_ONLY`, a human route.
For today's kinds that means `backup_workspace`, `restore_workspace` and `rebuild_index` jobs
need `admin`, the first two also a human session; `import`, `redigest`, `export_workspace` and
`export_view` jobs keep `write`. Tenant kinds are never controlled through these operations.

### Contract tests

- `operations::the_surface_is_77_operations_12_model_tools_and_1_app_tool` (renamed from 69).
- `operations::human_session_only_operations_are_never_agent_tools` and
  `operations::the_human_session_only_list_is_exactly_recovery_and_purge`, pinned by name like
  the draft-bearing list.
- `operations::destructive_hints_come_from_the_table`: the list loses `archive_workspace` and
  gains `purge_workspace` and `purge_item`.
- `semantic::job_kinds_name_the_operation_that_starts_them`: `started_by` is a bijection from
  workspace job kinds to the operations returning `Job` that create them, and tenant kinds map
  to `backup_installation`, `purge_workspace`, `purge_item`.
- `scope::purge_authorizes_at_the_tenant_only` and `scope::get_purge_needs_only_the_tenant_grant`.
- `error::an_invalidated_revision_is_a_typed_not_found_with_its_replacement`.
- `semantic::review_coverage_can_be_invalidated`.
- Core (core-cli): `access::human_session_only_operations_are_refused_on_agent_and_service_routes`
  (every member of `HUMAN_SESSION_ONLY` and `DRAFT_BEARING`, on `mcp_delegation` and `service`,
  with every grant); `dispatch::retrying_or_cancelling_a_backup_or_restore_job_needs_a_human_admin`
  (a `write`-only browser session is refused; `admin` on `mcp_delegation` and `service` is
  refused; `admin` in a browser session succeeds; an `import` job still needs only `write`).

### Deliberately left out

A purge preview operation and a confirmation challenge for purge: the tenant `admin` grant, the
human-session rule and `base_revision` are the guard.

## 7. Instants, workspace paths and security events

SPEC §3: "Instants are UTC in storage and on the wire (RFC 3339)". Owner, 2026-10-08: "instants
are UTC RFC 3339 `date-time` on the wire"; "WorkspacePath refuses Windows reserved names, trailing
dots and spaces, and NFC and case collisions; EventKind gains security events (connector issued
and revoked, sign-in, permission denied)."

### `Timestamp` (`crates/contract/src/identity.rs`)

```rust
/// A UTC instant in one canonical RFC 3339 spelling: `YYYY-MM-DDTHH:MM:SS.sssZ`, always three
/// fraction digits and the `Z` offset, such as `2026-10-08T14:03:07.250Z`.
///
/// Every value has the same width, so comparing the strings compares the instants.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);
```

Canonical form: exactly milliseconds. Schema: `{"type": "string", "format": "date-time",
"pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-5][0-9]:[0-5][0-9]\.[0-9]{3}Z$"}`: minutes
and seconds are `00` to `59`, so a leap second (`:60`) is refused by the pattern (final-review
decision 1). `TryFrom<String>` requires that pattern, parses with
`time::OffsetDateTime::parse(_, &Rfc3339)`, so an impossible date (`2026-02-30`) is refused too,
and then requires that writing the parsed instant with `from_utc` reproduces the input exactly:
one instant, one spelling, whatever leniency the parser has (time's RFC 3339 parser reads
`:60` as the preceding nanosecond).
Every other spelling is refused: no fraction (`...:07Z`), another fraction width (`...:07.5Z`,
`...:07.250000Z`), an offset other than `Z` (`+00:00` included), a lower-case `t` or `z`, a
leap second. Producers write with `Timestamp::from_utc(OffsetDateTime)`, which converts to UTC
and truncates to the millisecond (never rounds, so a value is never later than the instant it
records).

Why milliseconds: it is the precision of JavaScript's `Date` and exactly what
`Date.prototype.toISOString` writes, so the browser, the MCP App and Zod produce and compare the
canonical form without a library; it is finer than any instant the product shows or orders by
(sign-ins, saves, reviews, expiries); Git commit times are whole seconds and become `.000Z`. Nine
digits would claim a precision no source here has and would not survive a JavaScript round trip.
Two events in the same millisecond are ordered by their own identities (event cursor, job id),
never by the timestamp alone.

With one width, the derived `Ord` on the string is time order, and the old ambiguity
(`...:07.500Z` sorting before `...:07Z` as text while being later in time) cannot arise because
`...:07Z` is not a valid value.

`time` is already a selected workspace dependency at 0.3.55; the contract crate needs it with the
`parsing` and `formatting` features. That is a manifest change for the integration owner, and
Cargo.lock changes with the new dependency edge (ruling: batched into one lock, then Docling and
MCP Apps are requalified once; section 12).

`from_utc` returns `Result`: a UTC year outside 0000 to 9999 has no RFC 3339 spelling. It
writes through time's `Iso8601` formatter configured for three fraction digits, which truncates.
`Timestamp::instant()` reads the value back as an `OffsetDateTime`.

Fields that change from `String` to `Timestamp`: `Draft::saved_at`,
`SandboxCapability::expires_at`, `SourceName::observed_at`, `Receipt::returned_at`,
`Proposal::created_at`, `Comment::created_at`, `Review::reviewed_at`, `Confirmation::expires_at`,
`Commit::committed_at` (converted to UTC from the commit's offset), `Workspace::created_at`,
`Connector::created_at` and `revoked_at`; new: `Workspace::archived_at`, `Event::at`,
`SuppliedText::approved_at`, `Purge::requested_at` and `completed_at`,
and `PresentResponse::as_of`. Core:
`UploadRecord::created_at`, the expiry fields of `confirmations.rs`, `credentials.rs`,
`sandbox.rs`, and the new `JobLease::expires_at`.

### `WorkspacePath` rule

`TryFrom<String>` keeps today's checks and adds, for every `/`-separated segment:

1. no character from `< > " | ? *` (with today's `\`, `:` and controls, the set Windows refuses);
2. no trailing `.` and no trailing space;
3. its stem (the segment up to its first `.`), compared ASCII case-insensitively, is none of `CON`,
   `PRN`, `AUX`, `NUL`, `COM0` to `COM9`, `LPT0` to `LPT9`, `COM¹`, `COM²`, `COM³`, `LPT¹`, `LPT²`,
   `LPT³`, `CONIN$`, `CONOUT$` (so `con.txt` and `Lpt1.md` are refused);

and, for the whole value:

4. it is in Unicode NFC (`unicode_normalization::is_nfc`); a non-NFC value is refused, never
   silently changed on parse.

Normalization rule: a path is stored and compared in NFC. Code that builds a path from a supplied
filename (import naming, uploads' `relative_path`) calls `WorkspacePath::from_supplied(&str)`,
which converts to NFC and then applies the same checks; the supplied name itself is preserved
unchanged in `SourceName`. Two paths collide when their `collision_key()` values are equal:
NFC, then `str::to_lowercase` (locale-independent Unicode lowercasing), segment by segment.
`VersionStore::commit` and `create_candidate` refuse with `conflict` an edit that would give an
item a path whose collision key equals another item's, or two items one key in a single commit;
`preview_names` reports such a pair as a `DuplicateObservation`.
`unicode-normalization` 0.1.25 is already a selected workspace dependency and locked; the
contract crate needs it (manifest change) [inferred: no new package in Cargo.lock].
`WORKSPACE_PATH_PATTERN` adds `<>"|?*` to its excluded class; the NFC, stem and trailing rules
are stated in the doc comment as not expressible by the pattern.

### Security events (`crates/contract/src/events.rs`)

```rust
pub enum EventKind {
    // Changed, Imported, JobUpdated, Reviewed, ProposalUpdated unchanged
    /// A local connector credential was issued (tenant-level).
    ConnectorIssued,
    /// A local connector credential was revoked (tenant-level).
    ConnectorRevoked,
    /// A browser session began (tenant-level).
    SignedIn,
    /// An authenticated request was refused for lack of a grant or by its route.
    PermissionDenied,
}

/// Who caused a security event.
pub struct EventActor {
    /// Authenticated subject.
    pub subject: String,
    /// Route it came by.
    pub route: crate::access::AccessRoute,
    /// OAuth or connector client.
    pub client_id: Option<String>, // opt
}
```

`Event` changes: `workspace_id` becomes `Option<WorkspaceId>` (opt; absent for a tenant-level
event); new `at: Timestamp`, `connector_id: Option<ConnectorId>` (opt), `actor:
Option<EventActor>` (opt, present on security events), `operation: Option<OperationName>` (opt,
the refused operation of `permission_denied`).

Where each is written: `connector_issued` and `connector_revoked` by `create_connector` and
`revoke_connector`, tenant-level; `signed_in` by the server's session routes through a core
method (`ApplicationService::record_sign_in`), tenant-level; `permission_denied` by core dispatch
when authorization refuses an authenticated caller: in the workspace's log for a workspace
target, tenant-level otherwise. A handler's own `Forbidden` (the job-kind rule of `retry_job` and
`cancel_job`) is recorded the same way as `permission_denied` (added by the core-ports package);
a refusal with no evaluated refusing target, whether by route before targets or from a handler,
goes to the tenant's log when the request has any `Deployment` target, to the workspace's log when
its targets name exactly one distinct workspace, and to the tenant's log otherwise (none, or two
or more). An append to a workspace the tenant does not hold records at tenant level instead and
never creates or revives rows for it. `list_events` and `stream_events` return security events of a
workspace only to callers holding `admin` on it. New operation `list_tenant_events`
(`ListTenantEventsRequest { after: Option<String>, page: PageRequest }`, response
`ListEventsResponse`, tenant `admin`).

### Contract tests

- `semantic::timestamps_have_one_canonical_spelling`: accepts `2026-10-08T14:03:07.250Z`;
  refuses `2026-10-08T14:03:07Z`, `...:07.5Z`, `...:07.250000Z`, `...:07.250+00:00`, lower-case
  `t` and `z`, `2026-02-30T00:00:00.000Z`, `2026-10-08T24:00:00.000Z`,
  `2026-12-31T23:59:60.000Z` and a date alone; the schema states `date-time` and the pattern.
- `semantic::timestamp_order_is_time_order`: for pairs including `...:07.999Z` against
  `...:08.000Z`, `...:59.999Z` against the next minute's `.000Z`, and `2026-12-31T23:59:59.999Z`
  against `2027-01-01T00:00:00.000Z`, the derived `Ord` agrees with the parsed instants; and
  `from_utc` of an instant with sub-millisecond digits truncates (`07.2509Z`-precision input
  writes `.250Z`).
- `semantic::every_instant_is_a_timestamp`: in every operation schema, every property whose name
  ends in `_at` refers to `Timestamp`.
- `semantic::workspace_paths_refuse_windows_reserved_names`: `CON`, `con.txt`, `docs/LPT1.md`,
  `COM9`, `CONIN$` refused; `console.md` and `null-results.md` accepted.
- `semantic::workspace_paths_refuse_trailing_dots_spaces_and_reserved_characters`.
- `semantic::workspace_paths_must_be_nfc`: the NFD spelling of `résumé.md` is refused;
  `from_supplied` accepts it and returns the NFC spelling.
- `semantic::collision_keys_fold_case_and_normalization`: `Notes/Résumé.md` and `notes/résumé.md`
  collide; `a/b.md` and `a/c.md` do not.
- `semantic::a_tenant_event_has_no_workspace`: a `signed_in` event round-trips without
  `workspace_id`.
- `scope::tenant_events_need_the_tenant_admin_grant`.

### Deliberately left out

A per-segment length limit; retention and rate limits for `permission_denied` events.

## 8. Backup, restore, export and artifact access

SPEC §12: "Portable export gathers references into an independently readable folder/archive.
Full backup additionally preserves promised app records, retained objects and versions, except
what Purge (§8) removed. Restore must be exercised." SPEC §4: "Backups and garbage collection
include retained history and app records as appropriate." SPEC §2: "takes a recoverable backup
before an upgrade migration". The ledger decisions of 2026-10-08 bind this section, with the
ruling on owner Q1: "backup unit = one self-contained archive per workspace + one installation
archive (local identity and installation records needed to restore drafts to owners); replacement
restore takes all; qualification remains 'delete data dir, restore'." Owner decision F
(2026-10-06): "app reviews: written into the file as OKF verified on export; an imported verified
is an unconfirmed claim."

### The two archives

A **workspace archive** is one self-contained archive of a consistent checkpoint of one
workspace, taken under its write lock: its Git history; a `VACUUM INTO` snapshot of the SQLite
records filtered to that workspace before it is published (temporary file, integrity check,
publish on success); every retained object it references, copied, never a reference into the
live store; its drafts; its catalog entry (name, description, created and archived times); its
grants by subject; and the tenant id it belongs to, so a restore can check it against the
installation. It holds no installation identity, no connector record, no session, no tenant
event, and no earlier backup or export archive.

An **installation archive** holds no workspace content: the installation identity (the local
identity, the tenant id), the tenant grants the installation stores, connector metadata without
secrets (id, label, workspace ids, permissions, issue and revocation times; never a secret or its
hash), purge records, and tenant-level events. Hosted, it is per tenant: the tenant's records
only.

Producers:

- `backup_workspace` (unchanged request): a workspace job writing a workspace archive.
- `backup_installation` (new, `BackupInstallationRequest { idempotency_key }`, response `Job`,
  202): a tenant job writing an installation archive; tenant `admin`, human session only.
- The offline command `okf-jawn-server backup --output <directory>` (server lane), run with no
  service on the data directory (it takes the single-writer lock): writes the installation
  archive and every workspace archive; hosted, `--tenant <id>` limits it to one tenant.
- Before an upgrade migration, storage writes the same set as the offline command and keeps it as
  a managed, pre-migration backup (SPEC §2); purge reaches it (section 6).

Restore:

- **Replacement of the installation** is the offline command `okf-jawn-server restore
  --replacement <installation archive> <workspace archive>...`, run against a data directory with
  no running service. The installation archive is restored first: identity, tenant grants,
  purge records, tenant events, and connector records, each restored as revoked at the restore
  time because no secret is kept (the owner issues new ones). Then each workspace archive, which
  is refused when its tenant id differs from the installation archive's, when the installation
  archive holds a completed purge of that workspace, or when the workspace already exists. It
  keeps workspace ids, item ids and revisions, and gives each draft back to its editor, who is a
  validated identity because the installation archive restored them. Qualification: delete the
  data directory, run this command, start the service, find the same content and drafts.
- **Import into another installation** is the operation `restore_workspace`: it fills a blank
  workspace from a workspace archive uploaded into that workspace; the installation archive is
  not used. It keeps item ids, rewrites View bindings to the new workspace id (a View binds only
  to its own workspace), gives each draft whose editor is a validated subject with `write` on
  the target workspace back to that editor, and keeps the others in the archive, unassigned, with
  their count reported. Restoring a backup of another workspace of the same installation from a
  retained artifact is not offered: its purge semantics for the copy are not simple (round 1
  review, N5). For the same reason an uploaded archive of this tenant is refused when its
  workspace still exists or has a purge record (final-review decision 3): purged content never
  comes back through an import. The contract cannot express this (the archive's workspace and
  tenant are inside the archive, which only storage reads); `RestoreWorkspaceRequest`'s doc
  comment states it, and the storage lane pins it in
  `backup::an_import_refuses_an_archive_of_a_live_or_purged_workspace`.
- The upload a restore read is consumed when the restore completes: its slot records the
  consuming job, its object is released unless something else references it, `start_import`
  refuses it, and `restore_workspace` refuses an upload that was imported. An upload whose
  object is a backup archive is never a surviving reference for purge.

Export: committed content at one revision, with the originals and retained derivatives it
references, and its history when asked. Each item that has an app review whose coverage is
`current` at that revision gets that review written into its header as an OKF `verified` entry
`{ by, at }` [verified: okf-core-0.2.7/src/trust.rs:69-74, frontmatter.rs:289-297]. Export
excludes drafts, sessions and credentials. An imported `verified` stays an unconfirmed claim:
it shows as `ReviewCoverage::Imported` and never creates a review record.

Artifact access: the download boundary decides by artifact kind and route (below). `get_object`
and `download_object` never serve a digest recorded as a backup artifact's object, even when a
caller names it.

### Types (`crates/contract/src/workspace.rs`)

```rust
/// What a retained artifact is (moved from core `jobs.rs`).
pub enum ArtifactKind {
    /// A portable export of a workspace.
    Export,
    /// A workspace archive.
    WorkspaceBackup,
    /// An installation archive (hosted: one tenant's).
    InstallationBackup,
    /// An export of one View.
    ViewExport,
}

/// Where an artifact's records live, and so which transport serves it.
pub enum ArtifactScope {
    /// A workspace's artifact, served by `download_artifact`.
    Workspace,
    /// A tenant's artifact, served by `download_tenant_artifact`.
    Tenant,
}

impl ArtifactKind {
    /// Where the artifact lives: `InstallationBackup` is tenant-scoped, the rest workspace-scoped.
    pub const fn scope(self) -> ArtifactScope;
    /// Permission the download transport requires, on the workspace or on the tenant.
    /// `WorkspaceBackup` and `InstallationBackup`: `Admin`; `Export`, `ViewExport`: `Read`.
    pub const fn download_permission(self) -> crate::access::Permission;
    /// Routes that may download it; every other route is refused whatever its grants.
    /// Both backups: `[BrowserSession, LocalOwner]`.
    /// `Export`, `ViewExport`: `[BrowserSession, LocalOwner, Service]`.
    pub const fn download_routes(self) -> &'static [crate::access::AccessRoute];
}

/// Start an installation archive.
pub struct BackupInstallationRequest {
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Fill a blank workspace of this installation from an uploaded workspace archive.
pub struct RestoreWorkspaceRequest {
    /// Blank target workspace.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Completed upload of the archive into this workspace.
    pub upload_id: crate::identity::UploadId,
    /// SHA-256 the archive must have; checked before anything is written.
    pub sha256: crate::identity::Digest,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// What a restore did.
pub struct RestoreReport {
    /// Items restored.
    pub items: u32,
    /// Drafts given back to their editors.
    pub drafts_restored: u32,
    /// Drafts whose editor is not a validated identity here; kept in the archive.
    pub drafts_unassigned: u32,
}
```

`DownloadArtifact` gains `kind: ArtifactKind`; its `download_path` is
`/api/workspaces/{workspace_id}/artifacts/{artifact_id}` for a workspace artifact and
`/api/artifacts/{artifact_id}` for a tenant artifact. `Job` changes: `workspace_id` becomes
`Option<WorkspaceId>` (opt; absent for a tenant job), and it gains `restore:
Option<RestoreReport>` (opt), present on a completed restore job. `JobKind` gains
`backup_installation` (with the purge kinds of section 6). `JobKind::BackupWorkspace` keeps its
wire name. `ExportWorkspaceRequest` is unchanged.

Exports are refused on `mcp_delegation`: an export holds originals, and the owner's switch for
sending originals to a connected AI is off by default. Downloads are never MCP tools.

### Operations whose shapes change

- `restore_workspace`: the request above; description "Fill a blank workspace from an uploaded
  workspace archive, keeping item identities; drafts of editors unknown here stay in the archive
  and are counted. Human administrator session only." Target: `admin` on the target workspace.
  Destructive hint stays: it overwrites the blank target.
- `backup_workspace`: description "Back up the workspace's history, retained objects and
  application records, drafts included, into one self-contained archive. Human administrator
  session only."
- `export_workspace`: description "Build a portable export with resolvable referenced assets;
  current app reviews are written into the files as OKF verified. Drafts, sessions and
  credentials are never included."
- Added: `backup_installation`, and `get_tenant_job` and `list_tenant_jobs` to follow tenant jobs
  (rows in section 10).
- Transports: `download_artifact` description "Download a completed workspace export, View export
  or workspace archive. The artifact kind decides the permission and the routes
  (ArtifactKind::download_permission and download_routes); an archive is served only to a
  workspace administrator in a human browser session."; new `download_tenant_artifact` (`get`,
  `/api/artifacts/{artifact_id}`, `application/zip`, 200, `Principal`): "Download a completed
  installation archive; only a tenant administrator in a human browser session."; `download_object`
  gains "Never serves a backup artifact's object."
- `backup_workspace`, `restore_workspace` and `backup_installation` are in `HUMAN_SESSION_ONLY`
  (section 6), and so are their jobs.

### Contract tests

- `semantic::artifact_kind_decides_the_download_rule`: for all four kinds, the scope, the
  permission and the routes; both archives refuse `service` and `mcp_delegation`; exports refuse
  `mcp_delegation`.
- `scope::restore_reads_only_its_target_workspace`.
- `scope::backup_installation_and_tenant_jobs_need_the_tenant_admin_grant`.
- `semantic::a_download_artifact_states_its_kind`.
- `semantic::every_job_states_its_kind`: `JOB_KINDS` gains `backup_installation`,
  `purge_workspace` and `purge_item`.
- `semantic::a_tenant_job_has_no_workspace`: both directions (final-review decision 5).
  `JobKind::is_tenant` names the tenant kinds and `Job::check` refuses, on `/workspace_id`, a
  workspace job without `workspace_id` and a tenant job with one; the test decodes every kind
  with and without the field.
- Core (core-cli): `reading::get_object_refuses_a_backup_artifact_digest` (a workspace archive's
  digest, named with a valid citation of the workspace, is `not_found`).
- Storage (construction receipt, storage lane): `backup::replacement_restore_after_deleting_the_data_directory`
  and `backup::an_installation_archive_holds_no_workspace_content`.

### Deliberately left out

Encryption of backups (the ledger: the recovery administrator is a trusted custodian, no
crypto-privacy claim). Scheduled backups. Cancelling a tenant job.

## 9. Core ports (`core-ports-carry-extraction-and-views`, core-cli)

These follow from sections 1 to 8 and 13. Each names its producer and consumer.

### 9.1 Conversion (`crates/core/src/conversion.rs`)

```rust
/// One window of one document to convert inside a bounded worker.
pub struct ConversionInput {
    pub source: LocalSource,
    pub file_name: String,
    /// Contract settings (section 3).
    pub settings: okf_jawn_contract::extraction::ConversionSettings,
    /// One-based inclusive page window (docling `page_range`); `None` converts the whole
    /// document, which is the only choice for a format docling converts whole.
    pub window: Option<PageRange>,
    pub timeout: Duration,
    pub output_directory: PathBuf,
}

/// The worker bounds, set from the whole-document versus windowed measurement.
pub struct ConverterLimits {
    /// Pages per window.
    pub window_pages: u32,
    /// Hard memory cap of the child process, in bytes.
    pub memory_limit_bytes: u64,
}

/// Docling's status, plus the application's states.
pub enum ConversionStatus {
    Success,
    PartialSuccess,
    Unsupported,
    Failure(FailureReason), // contract type
}

/// Coverage of one window: the three lists are ascending, disjoint and together cover exactly
/// `window`. `check` enforces it; the contract `PageCoverage::check` applies only to the merged,
/// whole-document coverage over `1..=page_count`.
pub struct WindowCoverage {
    pub window: PageRange,
    pub converted: Vec<PageRange>,
    pub partly_extracted: Vec<PageRange>,
    pub not_converted: Vec<PageRange>,
}

pub struct Conversion {
    pub source_digest: Digest,
    /// Identity including settings, packages, hashed models and the window size.
    pub converter: ConverterIdentity, // contract type; replaces `ConverterIdentity` and `settings`
    pub status: ConversionStatus,
    pub document: Option<ConvertedDocument>,
    /// For a windowed (paginated) conversion: what this window converted.
    pub coverage: Option<WindowCoverage>,
    pub issues: Vec<ConverterIssue>, // contract type; replaces `ConversionIssue`
}

pub struct ConvertedDocument {
    pub markdown: String,
    /// The docling JSON export of this window.
    pub structured: PathBuf,
    pub outline: Vec<OutlineEntry>,
    pub assets: Vec<ConvertedAsset>,
    /// Line-to-location map of `markdown`: ascending, non-overlapping line ranges.
    pub locations: Vec<LineLocation>,
    /// Tables, for datasets (9.5).
    pub tables: Vec<ConvertedTable>,
    pub warnings: Vec<ExtractionWarning>, // contract type
}

pub struct LineLocation {
    pub lines: TextRange,
    pub location: SourceLocation, // contract type
}

pub struct ConvertedTable {
    /// Lines of `markdown` that render it.
    pub lines: TextRange,
    pub location: SourceLocation,
    pub num_rows: u32,
    pub num_cols: u32,
    /// Docling `table_cells`, zero-based grid offsets.
    pub cells: Vec<ConvertedCell>,
}

pub struct ConvertedCell {
    pub row: u32,
    pub column: u32,
    pub row_span: u32,
    pub column_span: u32,
    pub text: String,
    pub column_header: bool,
    pub row_header: bool,
}

pub struct ConvertedAsset {
    pub path: PathBuf,
    pub media_type: String,
    pub role: AssetRole,            // contract type
    pub location: SourceLocation,   // replaces `selection`
    pub pixel_size: Option<PixelSize>,
    /// The document's own caption or alternative text (replaces `AssetCaption`).
    pub caption: Option<String>,
}

/// The docling JSON export of one window, retained.
pub struct WindowExport {
    /// The window; `None` when the document was converted whole.
    pub window: Option<PageRange>,
    pub digest: Digest,
}

/// The retained record a digest names; JSON in the blob store, written by ingest and read by
/// core for citations (locations) and datasets (tables). Serialize + Deserialize.
pub struct ConversionRecord {
    pub converter: ConverterIdentity,
    pub outcome: ConversionOutcome,
    pub page_count: Option<u32>,
    /// One docling export per converted window, in page order.
    pub structured: Vec<WindowExport>,
    /// SHA-256 of the Markdown the record describes.
    pub markdown: Digest,
    pub locations: Vec<LineLocation>,
    /// Added by the core-ports package: the whole-document outline, window outlines joined with
    /// lines in the joined Markdown; a heading entry selects its section as lines (see 9.6).
    pub outline: Vec<OutlineEntry>,
    pub tables: Vec<ConvertedTable>,
    pub assets: Vec<RetainedAsset>, // ConvertedAsset with `digest` in place of `path`
    pub warnings: Vec<ExtractionWarning>,
}

pub trait Converter: Send + Sync {
    /// Page count of a paginated original (docling `pdf_page_count`; 1 for an image);
    /// `None` for a format converted whole.
    fn page_count<'a>(&'a self, source: &'a LocalSource, file_name: &'a str)
        -> PortFuture<'a, Option<u32>>;
    /// Convert one window in a child process under `limits().memory_limit_bytes`. A document
    /// fault is a `Conversion` (cap reached: `Failure(MemoryLimit)`); a worker fault, including
    /// a missing model or native library, is an error. Dropping the future kills the child.
    fn convert(&self, input: ConversionInput) -> PortFuture<'_, Conversion>;
    /// The configured bounds.
    fn limits(&self) -> ConverterLimits;
}
```

Library support: `page_range(first, last)` converts only that one-based window and skips other
pages before rasterising [verified: docling-1.93.5/src/converter.rs:248-257]; `pdf_page_count` is
re-exported [verified: docling-1.93.5/src/lib.rs:62-66] and takes the PDF's bytes and an optional
password [verified: docling.rs@e500c23/crates/docling-pdf/src/lib.rs:3161], so `page_count`
reads the retained bytes in the worker, not in the service process. Whether a window boundary
changes what docling assembles across pages (a paragraph or heading level spanning the boundary)
is not known [inferred]; the `converter-worker-memory-ceiling` measurement compares the two, and
`page_window` records the size used. A hard memory cap on a child process needs an OS facility;
the ruling gives that to the ingest lane with a vendor lookup.

The windowing loop is ingest's import and redigest handler: it asks `page_count`, converts window
by window, merges the windows into one `ConversionRecord` (Markdown in page order, line ranges
shifted, one `WindowExport` per window, each `WindowCoverage` checked and joined into the
contract `PageCoverage`, which is then checked over `1..=page_count`), retains it, and writes the
card. A window whose child hit the cap becomes `not_converted` pages, not a failed document. For
`unconverted_only`, the new record joins the previous record's converted windows with the new
ones.

Converter identity inputs:

- `packages` is generated: `xtask` reads the `docling`, `docling-core`, `docling-onnx` and
  `docling-pdf` entries of Cargo.lock and writes `generated/converter/packages.json` (name,
  version, source); ingest embeds it with `include_str!` and parses it at startup. `gen-check`
  fails when Cargo.lock changes and the file does not, so the identity cannot drift from the lock.
- `models` is computed once when the worker starts: every entry of `docling::model_inventory()`
  is hashed; a missing file stops the worker from starting (a worker fault, never a document
  outcome), and readiness reports it.

`ConversionSettings` and `OcrPolicy` move to the contract (section 3); `CaptionOrigin` and
`AssetCaption` are removed (section 3). Both landed with the contract package, because the
contract removed what they served: core re-exports the contract settings,
`ConvertedAsset::caption` is `Option<String>`, and `crates/core/tests/ports.rs`
`a_converted_image_carries_its_size_and_caption_origin` is rewritten as
`a_converted_image_carries_its_size_and_its_own_caption` (final-review decision 6). The rest of
9.1 is this package's.

### 9.2 Jobs and records (`crates/core/src/jobs.rs`)

- `JobScope { Tenant(TenantId), Workspace(StorageScope) }`. `RecordStore::create_job`,
  `get_job`, `list_jobs`, `cancel_job`, `retry_job`, `record_artifact` and `get_artifact` take
  `&JobScope`; `ClaimedJob::scope` and `pending_jobs` carry it; `JobQueue::enqueue` takes it. A
  tenant job's wire `Job` has no `workspace_id`.
- `JobSpec::Import` gains `settings: ConversionSettings` (resolved at acceptance).
- `JobSpec::Redigest` gains `pages: Option<Vec<PageRange>>` (the `not_converted` pages at
  `base_revision`, when `unconverted_only`).
- `JobSpec::RestoreWorkspace` becomes `{ upload_id: UploadId, archive: Digest }`, the archive
  checked against the request's `sha256` at acceptance.
- New, tenant-scoped: `JobSpec::BackupInstallation`, `PurgeWorkspace { purge_id, workspace_id }`,
  `PurgeItem { purge_id, workspace_id, item_id }`. `JobSpec::kind` maps them.
- `JobLease` gains `expires_at: Timestamp`. `RecordStore::update_progress` renews the lease to
  now plus the lease duration and returns the job, so a handler converting a long window calls
  it as a heartbeat at most every third of the lease; a cancelled job is seen there.
- `JobCompletion` gains `restore: Option<RestoreReport>`.
- `ArtifactKind` moves to the contract and is re-exported; `NewArtifact` and `ArtifactRecord`
  carry the `JobScope`; `artifact_download_path(&JobScope, ArtifactId)` gives either path.
- Tenant-scoped purge records, which outlive the workspace: `create_purge(&TenantId, MutationId,
  NewPurge) -> Purge` (unique on the id, and returning the unfinished purge of the same target),
  `get_purge(&TenantId, PurgeId) -> Purge`, `update_purge(&TenantId, Purge) -> Purge`.
- Derived objects (section 4): `record_derived_object(&StorageScope, DerivedObject) ->
  DerivedObject`, unique on (item, revision, digest), with `DerivedObject { item_id, revision,
  digest, kind: DerivedKind::Dataset, media_type }`; `derived_object(&StorageScope, ItemId,
  &Revision, &Digest) -> Option<DerivedObject>`. Recorded objects are garbage-collection roots.
- Revision map (section 6): written by `Purger`; `revision_mapping(&StorageScope, &Revision) ->
  Option<RevisionMapping { purge_id, replacement: Option<Revision> }>`.
- `UploadRecord` gains `consumed_by: Option<JobId>`; `UploadStore` gains `consume(scope, upload,
  job)`, refusing a second consumer.

### 9.3 Storage (`crates/core/src/storage.rs`)

- `SourceCard::extraction` becomes the contract `Extraction`; core's `Extraction` enum is
  removed. The card body is the shown text of section 5 rule 3, empty when `text_origin` is
  `none`. The card header carries the application header of section 13 (item id, archived flag,
  original name and digest, the `Extraction`), so index rebuild and export keep it. The import
  handler commits each card with outcome `pending` before converting (the producer of `pending`)
  and replaces it afterwards.
- `TreeEdit::SetLifecycle` becomes `SetStatus { item_id, status: Option<ItemStatus>, archived:
  Option<bool> }` (section 13); `Change::Archive` maps to `archived: Some(true)`.
- `TreeEdit::SupplyExtraction { item_id, based_on: Option<Digest>, pages: Vec<PageRange>,
  markdown: String, proposal_id: ProposalId, supplier: Provenance }`: writes the supplied text
  and its metadata beside the card. Approval is the promotion commit (its committer is the
  approver), from which `VersionStore::show` fills `SuppliedText::approved_by` and `approved_at`.
- `TreeEdit::CorrectDigest` gains `adopts: Option<ProposalId>`.
- `TreeEdit::from_change(change, &ChangeContext)` with `ChangeContext { new_item_id,
  new_item_kind, proposal_id, proposer: Provenance }`; still exhaustive over `Change`.
- `VersionStore::commit` and `create_candidate` refuse a path collision (section 7) and a change
  to the application header (`okf_jawn`, section 13) made through `CreateItem`, `EditItem`, a
  type definition (`set_type`) or a draft (final-review decision 2). The server's own header
  updates are allowed: `SetStatus` (the `archived` flag), a redigest's `WriteSourceCard`,
  `CorrectDigest` and `SupplyExtraction`.
- Already landed with the contract package, because the contract forced them:
  `TreeEdit::SetStatus`, `TreeEdit::SupplyExtraction`, `TreeEdit::from_change(change,
  &ChangeContext)`, and `ArtifactKind` re-exported from the contract (`ArtifactRecord::download`
  sets `kind` and takes the path by the kind's scope).
- `WorkspaceCatalog::list(tenant, include_archived: bool)`; new `unarchive(scope, MutationId,
  author)`; `archive` and `unarchive` set `archived_at`.
- New port `Purger` (storage implements; one call owns every store it must clear):
  `purge_workspace(&TenantId, MutationId, PurgeId, WorkspaceId) -> PurgeReport` and
  `purge_item(&StorageScope, MutationId, PurgeId, ItemId) -> PurgeReport`. Idempotent on the
  mutation id and resumable after a crash; returns only when nothing section 6 lists still holds
  the target, writes the revision map for an item purge, and counts objects kept for surviving
  references.
- New port `Backups` (storage implements): `write_workspace_archive(&StorageScope, MutationId) ->
  ObjectInfo`, `write_installation_archive(&TenantId, MutationId) -> ObjectInfo` (the handlers
  record them as artifacts of the two backup kinds), and `restore_import(&StorageScope, archive:
  Digest, MutationId, editors: Vec<String>) -> RestoreReport` (`editors`: subjects with `write`
  on the target, computed by core from `AccessControl`). The offline backup, the pre-migration
  backup and the replacement restore are storage functions the server command and startup call,
  not core ports.

### 9.4 Search, events and access

- `SearchQuery` gains `extraction: Option<ExtractionFilter>`; `text` may be empty only with a
  filter. The index stores each source's `ExtractionStatus` and each item's archived flag.
- `EventLog` takes `EventScope { Tenant(TenantId), Workspace(StorageScope) }` for `append` and
  `list`; `NewEvent` gains `connector_id`, `actor: Option<EventActor>` and `operation`; the store
  stamps `at`.
- `access::check_draft_route` becomes `check_human_route(principal, operation)`, refusing
  `DRAFT_BEARING` and `HUMAN_SESSION_ONLY` operations on `mcp_delegation` and `service`.
  Dispatch records `permission_denied` when authorization refuses an authenticated caller.
- `retry_job` and `cancel_job` apply the job-kind rule of section 6 after loading the job.

### 9.5 Application duties these ports enable

- Citations: core fills `SourceReference::locations` from the `ConversionRecord` of the cited
  digest (lines of the shown converter text), `corrected_text` or `supplied_text` otherwise,
  applies the omitted-fills, present-must-match rule, and strips locations from saved View
  bindings (section 2).
- Revisions: a request naming a revision is checked against the revision map first (section 6);
  stored citations are remapped or invalidated when read.
- Datasets: `present_view` and `resolve_view` materialize each binding from the record's
  `ConvertedTable` that the binding's selection covers (or the cells of an XLSX `cells`
  selection), write the `Dataset` blob, record it with `record_derived_object` against the
  binding's item and revision, and return `charts` and `as_of`. This is the "producer of a View's
  materialized dataset" of the gate. A dataset is read only from the converter's tables:
  `materialize` takes the source's `Extraction` and refuses with `DatasetUnavailable`, before any
  table is read, when the source is corrected, its text was supplied by an agent, or it has none;
  a produced `Dataset` always has `text_origin: converter` (added by the core-ports package).
- `get_object` serves an object only when it belongs to the cited item revision: the original of
  its source card, its conversion record, that record's structured exports and assets, or a
  derived object recorded for that (item, revision). It never serves a backup artifact's object.
- Export writes `verified` for current reviews (section 8); import reads an incoming `verified`
  as `imported` coverage.

### 9.6 Added by the core-ports package, beyond sections 9.1 to 9.5

- `Backups::open_installation_archive(tenant, digest, offset, length)` reads an installation
  archive back for the `download_tenant_artifact` transport: `BlobStore` is workspace-scoped and
  a tenant may have no workspace.
- A handler's own `Forbidden` is recorded as `permission_denied`, in the log decided by the
  scope rule of section 7 (the tenant's for a `Deployment` target; the one named workspace's;
  otherwise the tenant's).
- `SearchQuery::check` refuses a query with neither text nor an extraction filter. The rule has
  one implementation, `SearchRequest::check_rules` in the contract; `SearchQuery::check` asks it
  about the same text and filter and names the field `/text` for the request's `/query`. A
  shared function in the contract would be simpler than the placeholder request this builds and
  is the integration owner's to add.
- `ConversionRecord::outline` (section 9.1) is the outline of the Markdown the record describes,
  for the whole document: it is what the `outline` read view serves for a converted source and
  the input of `reading::section_lines`. A heading entry selects its section as lines, from the
  heading line to the line before the next heading of the same or a higher level, or to the end
  of the text (`OutlineEntry::selection` states it; ingest applies it, `section_lines` trusts it).
- `ChartFailure::TooLarge` is a row, column, cell or column-name size bound; no byte cap exists
  on a dataset beyond the conversion record it is read from.

## 10. Operation table delta

Inserted rows, exact, in table order (with matching `OperationName` variants):

```rust
// after archive_workspace
(unarchive_workspace, $crate::workspace::UnarchiveWorkspaceRequest, $crate::common::MutationResult, "/api/workspaces/unarchive-workspace", "Unarchive", "", "", "", Admin, "", 200, false, "Return an archived workspace to ordinary listings; its history and records were kept."),
(purge_workspace, $crate::purge::PurgeWorkspaceRequest, $crate::purge::Purge, "/api/workspaces/purge-workspace", "Purge", "", "", "", Admin, "", 202, true, "Permanently remove a workspace's originals, derivatives, index entries, history, managed backups and retained exports, and mark what referred to them invalidated. Never claims to erase copies outside the application. Tenant administrator in a human session only; repeating the request resumes an unfinished purge."),
(get_purge, $crate::purge::GetPurgeRequest, $crate::purge::Purge, "/api/workspaces/get-purge", "Purge status", "", "", "", Admin, "", 200, false, "Read a purge's progress and counts; available after the workspace is gone."),
// after restore_workspace
(backup_installation, $crate::workspace::BackupInstallationRequest, $crate::import::Job, "/api/workspaces/backup-installation", "Back up installation", "", "", "", Admin, "", 202, false, "Back up the installation's identities, tenant grants, connector records without secrets, purge records and tenant events; no workspace content. Tenant administrator in a human session only."),
// after delete_item
(purge_item, $crate::purge::PurgeItemRequest, $crate::purge::Purge, "/api/items/purge-item", "Purge", "", "", "", Admin, "", 202, true, "Permanently remove one item's bytes, derivatives, index entries and history, rewrite or delete the managed backups that hold it, and mark what referred to it invalidated. Tenant administrator in a human session only."),
// after list_jobs
(get_tenant_job, $crate::import::GetTenantJobRequest, $crate::import::Job, "/api/imports/get-tenant-job", "Progress", "", "", "", Admin, "", 200, false, "Read an installation-level job: an installation backup or a purge."),
(list_tenant_jobs, $crate::import::ListTenantJobsRequest, $crate::import::ListJobsResponse, "/api/imports/list-tenant-jobs", "Installation jobs", "", "", "", Admin, "", 200, false, "List installation-level jobs, newest first, with their artifacts."),
// after list_events
(list_tenant_events, $crate::events::ListTenantEventsRequest, $crate::events::ListEventsResponse, "/api/events/list-tenant-events", "Security activity", "", "", "", Admin, "", 200, false, "Read installation-level notifications: sign-ins, connector issue and revocation, and refusals outside a workspace."),
```

New request types: `GetTenantJobRequest { job_id: JobId }` and `ListTenantJobsRequest { page:
PageRequest }` (in `import.rs`).

Changed rows: `archive_workspace` destructive `false`; descriptions of `list_items`,
`search_items`, `open_proposal` (section 5), `export_workspace`, `backup_workspace` and
`restore_workspace` (section 8). New transport `download_tenant_artifact` (section 8).

`RequestScope`: `UnarchiveWorkspaceRequest` and `RestoreWorkspaceRequest` in
`workspace_keyed!(Admin: ...)`; `PurgeWorkspaceRequest`, `PurgeItemRequest` and
`BackupInstallationRequest` target `Deployment(Admin)` with their key; `GetPurgeRequest`,
`GetTenantJobRequest`, `ListTenantJobsRequest` and `ListTenantEventsRequest` target
`Deployment(Admin)` with no key; `SetLifecycleRequest`, `SearchRequest` and
`OpenProposalRequest` gain `check_rules` (sections 13 and 5).

None of the new operations is an MCP tool. Totals: 77 operations, 12 model tools, 1 app tool.

## 11. SPEC sentences

Unchanged sentences this design satisfies are quoted in each section. Proposed changes, exact:

§12, replace "Portable export gathers references into an independently readable folder/archive.
Full backup additionally preserves promised app records, retained objects and versions, except
what Purge (§8) removed. Restore must be exercised." with:

> Portable export gathers committed content at one revision, and the originals and derivatives
> it references, into an independently readable folder/archive; a current app review is written
> into its file as OKF `verified`, and drafts, sessions and credentials are never included. A
> full backup is independently recoverable and never depends on the live object store. It is one
> self-contained archive per workspace, a consistent checkpoint of its versions, retained objects
> and application records, drafts included, except what Purge (§8) removed; and one installation
> archive with the installation's identities, tenant grants, connector records without secrets,
> purge records and tenant events, and no workspace content. Only an administrator in a human
> browser session requests or downloads a backup, or an operator runs the offline command; the
> service itself takes one before an upgrade migration. The download boundary decides by
> artifact kind and route, so no agent, service route, export or object read returns one, and
> the custodian of an unencrypted backup can read it. Replacing an
> installation runs with no service on the data directory, restores the installation archive
> first and then every workspace archive, and keeps the original identities and drafts.
> Importing a workspace archive into another installation fills a blank workspace, gives each
> draft to its editor when that editor is a validated identity there, and keeps the other drafts
> in the archive, unassigned, with their count reported. Restore must be exercised, including
> after the original data directory is deleted.

§8, after "Purge never claims to erase exports, clones or backups outside the application's
control.", add:

> A purge does not complete while a managed backup or retained export that holds the target can
> still be restored or downloaded; stored objects that a surviving item still references are
> kept and counted, and a record that only hides the target is not a purge. The purge leaves a
> record of what was removed, by identity, with no content. After an item purge, a citation of
> another, unchanged item follows its rewritten revision and keeps its review coverage.

§5, after "Re-extraction must not erase corrections.", add:

> A partly converted file stays readable, names the pages that were not converted, and a retry
> converts only those pages. An agent may propose the complete text of a file the converter did
> not fully extract; the text is applied only when a person accepts the proposal, is labelled as
> supplied by that agent, and never changes the recorded converter outcome. A later complete
> conversion is shown instead unless a person keeps the supplied text, which stays in history.

§3, after "Ordinary editing and navigation must not require Git vocabulary.", add:

> Workspace paths stay portable: a path segment that Windows reserves or cannot store (a
> reserved device name, a trailing dot or space, a reserved character) is refused, paths are
> stored in Unicode NFC, and two paths that differ only in case or normalization collide.

§11, after "Logging out ends the session immediately.", add:

> Sign-ins, connector issue and revocation, and refused requests are recorded as events an
> administrator can read.

## 12. Generated outputs and qualification

`generated/converter/` is a generated directory like the others (final-review decision 7):
AGENTS.md lists it, `scripts/lib/generation.mjs` replaces and compares it, `lanes.mjs`
`generatedRoots` and CI's clean-tree step and artifact include it.

Every change above alters `api/`, `generated/cli/` and `ui/src/api/generated/`, including the
generated Zod inside the MCP App bundle and the schemas of the `workspaces`, `ls`, `grep`, `show`,
`propose` and `present` tools; `generated/converter/packages.json` is new. Cargo.lock changes
too: the contract crate gains `time` (features `parsing`, `formatting`) and
`unicode-normalization`, both already selected and locked, so the lock gains dependency edges.
The Docling receipt lists `Cargo.lock` among its inputs and the MCP App bundle contains the
generated Zod, so both receipts go stale. Ruling: any other dependency change of this stage is
batched into the same `lock`, and Docling and MCP Apps are requalified once, after the contract
lands, before `check-receipts` is relied on again. The harness's dataset fixture becomes a
`Dataset` document together with the views lane's switch of `PresentView` from its row-record
Zod schema to `zDataset`; until then `tests/fixtures/views/present-metrics-dataset.json` stays
rows of records, while `present-response.json` already carries `charts` and `as_of`. The views lane's `views-chart-failure-isolation` gate consumes `charts`.

## 13. Addendum: the 2026-10-06 OKF decisions, mapped

Ruling: these are decided; each is mapped here to a wire type or a storage behaviour. Idempotency
keys scoped by route: ruled not needed (SPEC keys them by tenant, subject, client and operation).

**Application header.** Every item file carries one application-owned header mapping under the
key `okf_jawn` (contract constant `item::APP_HEADER_KEY`): `item_id`, `archived` (when true),
and, on a source card, `source: { original_name, digest }` and `extraction` (section 1). It
appears in `ItemDocument::properties` and is server-owned: `create_item`, `save_draft`,
`Change::Create`, `Change::Edit` and `set_type` refuse a value that changes it as `invalid_input`
on `/properties/okf_jawn`, and `VersionStore` refuses such an edit (made through Create, Edit,
`set_type` or a draft; the server's own header updates are allowed, section 9.3). OKF preserves unknown
frontmatter keys, so the header survives export.

A type schema (`set_type`) is checked against the header at every schema position (a property,
a `required` entry or a conditional requirement named `okf_jawn`), and its references are
limited: a `$ref`, `$dynamicRef` or `$recursiveRef` is allowed only as `#`, `#/$defs/<token>` or
`#/definitions/<token>` with one RFC 6901 reference token, otherwise `invalid_input` on
`{at}/$ref` (the keyword used). Every `$defs` and `definitions` entry is scanned, so every
subschema a reference reaches has been scanned. A type is evaluated against an item's properties
without the header, so no type constrains its value; a schema that demands it by other means
(for example `not` over `propertyNames`) is satisfied by no item, like the schema `false`.

| Decision | Mapping |
| --- | --- |
| D. Snapshot conflict: per item, keep mine or take theirs, with a diff | No new type. `DraftConflictItem` already carries the item's committed changes. Keep mine is `save_draft` with `base_revision` = the item's `current_revision`, then `commit_items`; take theirs is `discard_draft`. `DraftConflictItem`'s doc comment states the two. |
| E. Status words Draft, Stable, Deprecated, Archived a separate flag | `item::Lifecycle` is removed. `ItemStatus { Draft, Stable, Deprecated, Other }` is OKF's `status` (absent reads `stable`; `other` is a producer value outside the three, kept exactly in the file's `status` property) [verified: okf-core-0.2.7/src/trust.rs:220-237]. `ItemSummary` replaces `lifecycle` with `status: ItemStatus` and `archived: bool`. `SetLifecycleRequest` replaces `lifecycle` with `status: Option<ItemStatus>` (opt) and `archived: Option<bool>` (opt); `check_rules` requires one of them and refuses `other`. `SearchRequest::include_archived` now means only the archived flag; deprecated items are always searchable. |
| F. App reviews written as OKF `verified` on export; an imported `verified` is an unconfirmed claim | Export behaviour of section 8; import shows `ReviewCoverage::Imported` and creates no review. |
| G. Item id in the file's header | `okf_jawn.item_id`. An ordinary import assigns a new id and writes it; restore keeps the archived ids. |
| H. Source card name `report-pdf.md`, original name in the header | `conventions::source_card_name(original: &str) -> String`: split at the last `.`; stem, `-`, extension, then `.md` (`report.pdf` gives `report-pdf.md`, `v1.2.notes.txt` gives `v1.2.notes-txt.md`); a name with no `.`, or whose only `.` is its first character, gets `.md` alone (`README` gives `README.md`, `.env` gives `.env.md`). Case is kept. A collision takes the numbered-suffix rule (`report-pdf-2.md`), then naming rules apply. The header keeps `okf_jawn.source.original_name` exactly as supplied. A character outside OKF's portable set gets OKF's own portability warning and is not refused. |
| I. An imported file whose header does not parse needs attention and blocks nothing | Its bytes are kept as the source and its card is written by the application with a valid header; the card body is the file's text; `AttentionKind` gains `UnparseableHeader` (action `get_item`). |

Contract tests: `conventions::source_card_name_joins_the_extension` (the examples above);
`semantic::item_status_words_are_okf_status_words`; `scope::set_lifecycle_needs_status_or_archived`;
`semantic::the_application_header_key_is_one_constant`. Core (core-cli):
`items::a_write_that_changes_the_application_header_is_refused`.

## 14. Owner questions

None open. The two questions of round 0 were answered by the orchestrator's rulings (2026-10-08),
recorded with what changes if a ruling is wrong:

1. Backup unit: one self-contained archive per workspace plus one installation archive;
   replacement restore takes all, installation archive first; qualification stays "delete the
   data directory, restore" (section 8). If wrong: the archive layout changes before storage
   writes it.
2. Unrelated citations after an item purge: remapped to the rewritten revision when the cited
   item's content digest is unchanged, keeping review coverage; citations of the purged item are
   invalidated. Conditions carried in section 6: a durable old-to-new revision map instead of
   rewriting receipts, feeding `ErrorDetail::Invalidated::replacement`; an emptied commit maps to
   its parent's rewrite; commit messages and the `path` of invalidated citations are scrubbed.
   If wrong: only item purge changes, and it is built after workspace purge.
