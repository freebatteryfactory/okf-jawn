# Stage 1b: contract design for the eight shared shapes

Status: design for review. Temporary working document, removed when Stage 1 closes like the
Stage 1 design beside it; SPEC.md, README.md and AGENTS.md stay the canonical prose. Base:
`eda0253` on `main`. Branch: `cure/stage-1b-contract`.

This document fixes the shared wire types and port shapes for eight subjects, so that storage,
ingest, MCP and the UI mean the same thing by the same object. It is the input of two gates:
`contract-expresses-extraction-sources-views` (integration owner, sections 1 to 8 and 10) and
`core-ports-carry-extraction-and-views` (core-cli, section 9). It is not a lane plan.

Marks. `[verified: path:line]` cites the source that shows a library capability; library paths
are relative to the crate root in the Cargo registry (`docling-1.93.5/src/...`,
`docling-core-1.93.6/src/...`, `docling-pdf-1.93.6/src/...`) or to this repository.
`[inferred]` marks what was not read in source.

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
`lib.rs`. The operation table grows from 69 to 74 operations (section 10); the model tool set (12)
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
  bytes }`, with no hash [verified: docling-pdf-1.93.6/src/lib.rs:372-383]; the qualification
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
    pub packages: Vec<ConverterPackage>,
    /// Settings applied.
    pub settings: ConversionSettings,
    /// Model files the pipeline loaded; empty for formats that load none.
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

/// One model file of `docling::model_inventory()`, hashed by the worker.
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
[verified: docling-core-1.93.6/src/confidence.rs:1-21] and no per-item OCR flag in the export
[inferred: no `from_ocr` field appears in docling-pdf 1.93.6]; nothing consumes it yet. Sheets the
XLSX backend skips past `DOCLING_RS_SHEET_MAX_CELLS` are reported only on stderr
[verified: docling-1.93.5/src/backend/xlsx.rs:66-72]; no typed warning is designed until ingest can
observe it other than by parsing stderr. Formula versus value distinctions (SPEC §5) are not
given by the backend and are not modelled here.

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
| PDF | `prov` per item: `page_no`, `bbox {l,t,r,b,coord_origin}`, `charspan`; captions lose their box [verified: qualification/docling/src/locate.rs:3-9, 228-242] | `region`, unit `point` [inferred: PDF page units], `direct`; or `inferred` by the text-layer rule; or `unresolved` with the rule's reason |
| Image | one page; every item needs a page and a box, and the fixture's picture is located by the export [verified: qualification/docling/lib/criteria.mjs:52; qualification/receipts/docling.json:84-87] | `region`, unit `pixel` [inferred], `direct` |
| PPTX | `page_no` is the slide; the box is in EMU and the page size is the slide size in EMU; top-left origin [verified: docling-core-1.93.6/src/tree.rs:117-133]; 26 of 28 items located in the corpus deck [verified: qualification/receipts/docling.json:45] | `region`, unit `emu`, `direct`; an item with no `prov` is `unresolved`, `not_located_by_converter` |
| XLSX | every sheet is a page in workbook order; the box is the item's cell-index box, top-left origin, half-open on the right and bottom [verified: docling-1.93.5/src/backend/xlsx.rs:137-139, 268-271, 309-311, 370-381] | `cells`: sheet name from the page number's position in workbook order, rows and columns converted to one-based inclusive, `direct` |
| DOCX, HTML | `prov: []` [verified: docling-core-1.93.6/src/tree.rs:163-165] | `unresolved`, `format_has_no_locator` |
| OCR text (scanned PDF, image) | recognition runs per layout region, so recognised text arrives as ordinary items with the region's box [verified: docling-pdf-1.93.6/src/ocr.rs:1-6] | the same `region` at item granularity; no word or line boxes exist in the export |

### Types (`crates/contract/src/source.rs`, with `Selection` in `read.rs`)

```rust
/// Unit of a region's coordinates; fixed by the format, stated so readers need no format logic.
pub enum RegionUnit {
    /// PDF points.
    Point,
    /// Image pixels.
    Pixel,
    /// PPTX English Metric Units.
    Emu,
}

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

/// Extent of the page the box lies on, in the same unit.
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
    /// Coordinate unit.
    pub unit: RegionUnit,
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
  The server computes it from (item, revision, digest, selection). In a request it is accepted
  only as the server returned it: the application recomputes it and refuses a difference as
  `invalid_input` on `.../locations`. An agent therefore cannot supply a location. Because every
  citation type holds a `SourceReference`, locations reach `ReadItemResponse`, `SearchHit`,
  `MediaReference`, `ViewBinding`, `Review`, `Receipt`, `BlameResponse` and `AddCommentRequest`.

No operation is added.

### Contract tests

- `semantic::an_inferred_location_never_reads_as_direct`: `{"provenance":"inferred",...}` does
  not deserialize as `Direct` and keeps its variant through a round trip.
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
  [verified: docling-1.93.5/src/converter.rs:750-759; docling-core-1.93.6/src/document.rs:58];
  and a picture item's own image [verified: docling-core-1.93.6/src/tree.rs:87-105]. Page images
  exist for the PDF and image pipeline only [verified: qualification/receipts/docling.json:35].
  Nothing else is an image asset, so there are two roles.
- `PictureImage` carries `mimetype`, `width`, `height`, `data`, `dpi`
  [verified: docling-core-1.93.6/src/document.rs:655-669]; page renders are 144 dpi unless
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
    /// Render resolution; docling `images_scale` = this / 72. 144 is docling's own default.
    #[schemars(range(min = 36, max = 600))]
    pub page_image_dpi: u16,
}
```

`impl Default for ConversionSettings` (Rust only, not a serde default): `ocr: Auto`,
`ocr_language: None`, `table_structure: true`, `page_images: true`, `page_image_dpi: 144`.
`page_images` changes from `false` to `true`: page renders are what a `region` citation is drawn
on (section 2) and what SPEC §5 asks to retain; page windows (section 9.1) bound the memory.

In `crates/contract/src/read.rs` (moved from core, and the free string replaced):

```rust
/// What a retained image is.
pub enum AssetRole {
    /// A render of one whole page (docling page image).
    PageImage,
    /// A picture item's own image (docling `PictureItem` image).
    Picture,
}

/// Where a caption's text came from.
pub enum CaptionOrigin {
    /// The document's own caption or alternative text.
    Source,
    /// Generated by the conversion process.
    Process,
    /// Written by a person.
    Human,
    /// Supplied by an agent and accepted by a person.
    Agent,
}
```

`MediaReference` gains `role: AssetRole`; `caption_origin` changes from `String` to
`CaptionOrigin`. Its `source` locates the image: a page image's location is `direct` `page`; a
picture's is its region.

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
- The dataset is produced by core while resolving (section 9.5) and written to the blob store;
  writing a content-addressed derived blob during a `Read` operation is not a change of user state.
  The UI fetches it through `read_object` as today, verifies the digest, and parses it with the
  generated `zDataset`.
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

### Deliberately left out

Live-mode as-of times per binding (SPEC §10 "reports each resolved revision and as-of time"):
not in this list. The rendered image of a chart for `export_view`.

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
    reviewed. A supply_extraction change proposes the complete text of an unprocessed source
    file; it is applied only when a person accepts it and is labelled as supplied by an agent."
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
are tracked honestly; no tombstone-only substitute."

### Archive

- `Workspace` gains `archived_at: Option<Timestamp>` (opt).
- `ListWorkspacesRequest` gains `include_archived: bool` (required, as on `SearchRequest`).
- New operation `unarchive_workspace` (`UnarchiveWorkspaceRequest { workspace_id,
  idempotency_key }`, response `MutationResult`).
- While a workspace is archived, an operation that would change its content or open work on it
  (a commit, a draft, a proposal, an upload, an import or redigest) is refused as `conflict`;
  reads, export, backup, `unarchive_workspace` and purge are allowed.
- `archive_workspace` stops being marked destructive: it destroys nothing.
- Items keep `set_lifecycle` with `archived` and `active`, which is already reversible.

### Types (`crates/contract/src/purge.rs`, `PurgeId` in `identity.rs`)

```rust
uuid_id!(PurgeId, "Identity of one purge; its record outlives what it removed.");

/// Permanently remove a workspace.
pub struct PurgeWorkspaceRequest {
    /// Workspace to remove.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Head the administrator saw; a moved head conflicts.
    pub expected_head: crate::identity::Revision,
    /// Retry identity.
    pub idempotency_key: crate::identity::IdempotencyKey,
}

/// Permanently remove one item, its history and its derivatives.
pub struct PurgeItemRequest {
    /// Workspace of the item.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Item to remove.
    pub item_id: crate::identity::ItemId,
    /// Head the administrator saw; a moved head conflicts.
    pub expected_head: crate::identity::Revision,
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
    /// Every application-managed copy is removed; no affected backup is restorable.
    Completed,
    /// Stopped; retrying the same request resumes it.
    Failed,
}

/// What a completed purge did; counts only.
pub struct PurgeReport {
    /// Stored objects deleted.
    pub objects_removed: String,
    /// Objects kept because a surviving item still references them.
    pub objects_kept_shared: String,
    /// Managed backups deleted.
    pub backups_removed: u32,
    /// Managed backups rewritten without the target.
    pub backups_rewritten: u32,
    /// Retained export artifacts deleted.
    pub exports_removed: u32,
    /// Revisions that no longer exist or were rewritten.
    pub revisions_invalidated: u32,
    /// Citations (reviews, receipts, View bindings) marked invalidated.
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
    /// The job doing the work.
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

### Invalidated state

- Revisions: `ErrorDetail` gains `Invalidated { purge_id: PurgeId, replacement:
  Option<Revision> }`, sent with `ErrorCode::NotFound` when a request names a revision or object a
  purge removed (`replacement` is the rewritten revision, for item purge). A client that does not
  know the detail still sees not-found.
- Citations: `ReviewCoverage` gains `Invalidated`; `Receipt` gains
  `invalidated_by: Option<PurgeId>` (opt).
- Views: `ChartFailure::Invalidated` (section 4); `AttentionKind` gains `Invalidated` (a saved View
  or citation refers to purged content; action `get_view`).
- Workspace purge removes the workspace's whole record set except the purge job row and the
  `Purge` record, which carry no content. Views bind only to their own workspace (SPEC §10), so a
  workspace purge invalidates no View elsewhere.

### Operations

Added (rows in section 10): `unarchive_workspace`, `purge_workspace`, `purge_item`,
`get_purge`. `purge_workspace` and `purge_item` return `Purge` with status 202; the work runs as a
job of the one execution system (`JobKind` gains `purge_workspace` and `purge_item`). `get_purge`
authorizes at the tenant, because the workspace may be gone. `purge_item` is declared now and
returns `not_implemented` until its storage gate passes; nothing claims it works earlier.

Human-session rule, `crates/contract/src/operations.rs`, after `DRAFT_BEARING`:

```rust
/// Operations refused on every route that is not a human browser session, whatever the grants:
/// recovery and destruction belong to a person (ledger 2026-10-08), never to an agent or a
/// service identity.
pub const HUMAN_SESSION_ONLY: &[OperationName] = &[
    OperationName::BackupWorkspace,
    OperationName::RestoreWorkspace,
    OperationName::PurgeWorkspace,
    OperationName::PurgeItem,
];
```

### Contract tests

- `operations::the_surface_is_74_operations_12_model_tools_and_1_app_tool` (renamed from 69).
- `operations::human_session_only_operations_are_never_agent_tools` and
  `operations::the_human_session_only_list_is_exactly_recovery_and_purge`, pinned by name like
  the draft-bearing list.
- `operations::destructive_hints_come_from_the_table`: list loses `archive_workspace`, gains
  `purge_workspace` and `purge_item`.
- `scope::get_purge_needs_only_the_tenant_grant`.
- `error::an_invalidated_revision_is_a_typed_not_found`.
- `semantic::review_coverage_can_be_invalidated`.

### Deliberately left out

How storage removes an item from Git history: section 14 asks the owner what happens to
unrelated citations at rewritten revisions. A purge preview operation and a confirmation
challenge for purge: the human-session rule and `expected_head` are the guard.

## 7. Instants, workspace paths and security events

SPEC §3: "Instants are UTC in storage and on the wire (RFC 3339)". Owner, 2026-10-08: "instants
are UTC RFC 3339 `date-time` on the wire"; "WorkspacePath refuses Windows reserved names, trailing
dots and spaces, and NFC and case collisions; EventKind gains security events (connector issued
and revoked, sign-in, permission denied)."

### `Timestamp` (`crates/contract/src/identity.rs`)

```rust
/// A UTC instant in RFC 3339 with a `Z` offset, such as `2026-10-08T14:03:07.250Z`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);
```

Schema: `{"type": "string", "format": "date-time", "pattern":
"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]{1,9})?Z$"}`. `TryFrom<String>`
parses with `time::OffsetDateTime::parse(_, &Rfc3339)` and accepts only the pattern's form: an
offset other than `Z`, a lower-case `t` or `z`, or an impossible date is refused. Because only one
spelling is accepted, string order is time order. `Timestamp::from_utc(OffsetDateTime)` writes
that form (fraction trimmed of trailing zeros) for producers. `time` is already a selected
workspace dependency at 0.3.55; the contract crate needs it with the `parsing` and `formatting`
features, which is a manifest change for the integration owner [inferred: no new package in
Cargo.lock].

Fields that change from `String` to `Timestamp`: `Draft::saved_at`,
`SandboxCapability::expires_at`, `SourceName::observed_at`, `Receipt::returned_at`,
`Proposal::created_at`, `Comment::created_at`, `Review::reviewed_at`, `Confirmation::expires_at`,
`Commit::committed_at` (converted to UTC from the commit's offset), `Workspace::created_at`,
`Connector::created_at` and `revoked_at`; new: `Workspace::archived_at`, `Event::at`,
`SuppliedText::approved_at`, `Purge::requested_at` and `completed_at`. Core:
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
target, tenant-level otherwise. `list_events` and `stream_events` return security events of a
workspace only to callers holding `admin` on it. New operation `list_tenant_events`
(`ListTenantEventsRequest { after: Option<String>, page: PageRequest }`, response
`ListEventsResponse`, tenant `admin`).

### Contract tests

- `semantic::timestamps_are_utc_rfc3339`: accepts `Z` forms with and without a fraction; refuses
  `+00:00`, `+02:00`, lower-case `z`, `2026-02-30T00:00:00Z` and a date alone; schema format is
  `date-time`.
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
include retained history and app records as appropriate." The ledger decisions of 2026-10-08 bind
this section (quoted in section 11).

### Decisions this design takes

- Unit. A full backup is per workspace (owner question 1, section 14): one self-contained archive
  of a consistent checkpoint, taken under the workspace's write lock, holding its Git history, a
  `VACUUM INTO` snapshot of the SQLite records filtered to that workspace before publishing
  (temporary file, integrity check, publish on success), every retained object it references
  (copied, never a reference into the live store), drafts, and ownership metadata (the tenant id,
  the installation's local identity, the workspace's grants). It never holds sessions or
  connector credentials; after a replacement restore the owner issues connectors again. A
  per-workspace unit keeps jobs, permissions and purge workspace-scoped: purging a workspace
  deletes its backups, and purging an item rewrites or deletes its workspace's backups.
- Replacement-installation restore is a server command, `okf-jawn-server restore --replacement
  <archive>...`, run against a data directory with no running service (it takes the single-writer
  lock): it restores original workspace ids, item ids, subjects, the local identity and drafts, and
  refuses a data directory that already holds one of the archive's workspaces. It is not an
  operation: a replacement installation has no session, workspace or upload slot to call one
  with, and the local identity must be in place before the service starts.
- Import into another installation is the operation `restore_workspace`: it fills a blank
  workspace from a backup, keeps item ids, rewrites View bindings to the new workspace id (a View
  binds only to its own workspace), restores each draft whose editor is a validated subject with
  `write` on the target workspace, and keeps the others in the archive, unassigned, with their
  count reported.
- Export: committed content at one revision with the originals and retained derivatives it
  references, with history when asked; never drafts, sessions, credentials or application
  records. `export_workspace` is unchanged.
- The download boundary decides by artifact kind and route (below). `get_object` and
  `download_object` never serve a digest recorded as a backup artifact's object, even when a
  caller names it.

### Types (`crates/contract/src/workspace.rs`)

```rust
/// What a retained artifact is (moved from core `jobs.rs`).
pub enum ArtifactKind {
    /// A portable export of a workspace.
    Export,
    /// A full backup of a workspace.
    Backup,
    /// An export of one View.
    ViewExport,
}

impl ArtifactKind {
    /// Permission the `download_artifact` transport requires on the artifact's workspace.
    pub const fn download_permission(self) -> crate::access::Permission; // Backup: Admin; others: Read
    /// Routes that may download it; every other route is refused whatever its grants.
    pub const fn download_routes(self) -> &'static [crate::access::AccessRoute];
    // Backup: [BrowserSession, LocalOwner]
    // Export, ViewExport: [BrowserSession, LocalOwner, Service]
}

/// Where the bytes of a backup to restore are.
pub enum BackupSource {            // tag "kind"
    /// An archive uploaded into the target workspace's upload slots and completed.
    Upload {
        /// The completed upload.
        upload_id: crate::identity::UploadId,
    },
    /// A retained backup artifact of a workspace in this installation.
    Artifact {
        /// Workspace that holds the artifact.
        workspace_id: crate::identity::WorkspaceId,
        /// The artifact.
        artifact_id: crate::identity::ArtifactId,
    },
}

/// Restore a backup into a blank workspace of this installation (import mode).
pub struct RestoreWorkspaceRequest {
    /// Blank target workspace.
    pub workspace_id: crate::identity::WorkspaceId,
    /// Where the archive is.
    pub archive: BackupSource,
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

`DownloadArtifact` gains `kind: ArtifactKind`. `Job` gains `restore: Option<RestoreReport>` (opt),
present on a completed restore job. `BackupWorkspaceRequest` and `ExportWorkspaceRequest` are
unchanged.

Exports are refused on `mcp_delegation`: an export holds originals, and the owner's switch for
sending originals to a connected AI is off by default. Downloads are never MCP tools.

### Operations whose shapes change

- `restore_workspace`: new request shape; description "Fill a blank workspace from a backup
  archive, keeping item identities; drafts of editors unknown here stay in the archive and are
  counted. Human administrator session only." Targets: `admin` on the target workspace, and
  `admin` on the source workspace for an `artifact` source. Destructive hint stays: it overwrites
  the blank target.
- `backup_workspace`: description "Back up the workspace's history, retained objects and
  application records, drafts included, into one self-contained archive. Human administrator
  session only."
- `download_artifact` transport description: "Download a completed export or backup. The
  artifact kind decides the permission and the routes (ArtifactKind::download_permission and
  download_routes); a backup is served only to a workspace administrator in a human browser
  session."
- `download_object` transport description gains: "Never serves a backup artifact's object."
- `backup_workspace` and `restore_workspace` are in `HUMAN_SESSION_ONLY` (section 6).
- No operation is added.

### Contract tests

- `semantic::artifact_kind_decides_the_download_rule`: backup needs `admin` and a human route;
  export and view export need `read` and refuse `mcp_delegation`.
- `scope::restore_names_every_workspace_it_reads`: an artifact source adds `admin` on the source
  workspace; an upload source does not.
- `semantic::a_download_artifact_states_its_kind`.
- `semantic::every_job_states_its_kind` gains `purge_workspace` and `purge_item` in `JOB_KINDS`.

### Deliberately left out

Encryption of backups (the ledger: the recovery administrator is a trusted custodian, no
crypto-privacy claim). Scheduled backups. A tenant-wide backup.

## 9. Core ports (`core-ports-carry-extraction-and-views`, core-cli)

These follow from sections 1 to 8. Each names its producer and consumer.

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

pub struct Conversion {
    pub source_digest: Digest,
    /// Identity including settings, packages, hashed models and the window size.
    pub converter: ConverterIdentity, // contract type; replaces `ConverterIdentity` and `settings`
    pub status: ConversionStatus,
    pub document: Option<ConvertedDocument>,
    /// The window converted, as requested.
    pub window: Option<PageRange>,
    /// Pages of the window converted, partly extracted, not converted; for a paginated format.
    pub coverage: Option<PageCoverage>, // contract type
    pub issues: Vec<ConverterIssue>,    // contract type; replaces `ConversionIssue`
}

pub struct ConvertedDocument {
    pub markdown: String,
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
    pub role: AssetRole,           // contract type
    pub location: SourceLocation,  // replaces `selection`
    pub pixel_size: Option<PixelSize>,
    pub caption: Option<AssetCaption>, // its `origin` becomes the contract `CaptionOrigin`
}

/// The retained record a digest names; JSON in the blob store, written by ingest and read by
/// core for citations (locations) and datasets (tables). Serialize + Deserialize.
pub struct ConversionRecord {
    pub converter: ConverterIdentity,
    pub outcome: ConversionOutcome,
    pub page_count: Option<u32>,
    /// Digest of the docling JSON export, retained.
    pub structured: Digest,
    /// SHA-256 of the Markdown the record describes.
    pub markdown: Digest,
    pub locations: Vec<LineLocation>,
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
password [verified: docling-pdf-1.93.6/src/lib.rs:3161], so `page_count` reads the retained bytes
in the worker, not in the service process. Whether a window boundary changes what
docling assembles across pages (a paragraph or heading level spanning the boundary) is not known
[inferred]; the `converter-worker-memory-ceiling` measurement must compare the two and the
`page_window` field records the size used. A hard memory cap on a child process needs an OS
facility (rlimit on Linux, a job object on Windows); no selected dependency provides it without
`unsafe` [inferred], so ingest may need a dependency decision from the integration owner.

The windowing loop is ingest's import and redigest handler: it asks `page_count`, converts window
by window, merges the windows into one `ConversionRecord` (Markdown in page order, line ranges
shifted, coverage joined), retains it, and writes the card. A window whose child hit the cap
becomes `not_converted` pages, not a failed document.

`CaptionOrigin` and `ConversionSettings`/`OcrPolicy` move to the contract (sections 3, 1).

### 9.2 Jobs (`crates/core/src/jobs.rs`)

- `JobSpec::Import` gains `settings: ConversionSettings` (resolved at acceptance).
- `JobSpec::Redigest` gains `pages: Option<Vec<PageRange>>` (the `not_converted` pages at
  `base_revision`, when `unconverted_only`).
- `JobSpec::RestoreWorkspace` becomes `{ archive: Digest }`: the archive's object, resolved from
  the `BackupSource` and checked against the request's `sha256` at acceptance.
- `JobSpec` gains `PurgeWorkspace { purge_id: PurgeId }` and `PurgeItem { purge_id: PurgeId,
  item_id: ItemId }`; `JobSpec::kind` maps them.
- `JobLease` gains `expires_at: Timestamp`. `RecordStore::update_progress` renews the lease to
  now plus the lease duration and returns the job, so a handler converting a long window calls
  it as a heartbeat at most every third of the lease; a cancelled job is seen there.
- `JobCompletion` gains `restore: Option<RestoreReport>`.
- `ArtifactKind` moves to the contract and is re-exported.
- `RecordStore` gains tenant-scoped purge records (they outlive the workspace):
  `create_purge(&TenantId, MutationId, NewPurge) -> Purge` (unique on the id),
  `get_purge(&TenantId, PurgeId) -> Purge`, `update_purge(&TenantId, Purge) -> Purge`.

### 9.3 Storage (`crates/core/src/storage.rs`)

- `SourceCard::extraction` becomes the contract `Extraction`; core's `Extraction` enum is
  removed. The card body is the shown text of section 5 rule 3, empty when `text_origin` is
  `none`. The card's frontmatter carries the `Extraction` under one key storage names, so index
  rebuild and export keep it. The import handler commits each card with outcome `pending`
  before converting (the producer of `pending`) and replaces it afterwards.
- `TreeEdit::SupplyExtraction { item_id, based_on: Option<Digest>, pages: Vec<PageRange>,
  markdown: String, proposal_id: ProposalId, supplier: Provenance }`: writes the supplied text
  and its metadata beside the card. Approval is the promotion commit (its committer is the
  approver), from which `VersionStore::show` fills `SuppliedText::approved_by` and
  `approved_at`.
- `TreeEdit::CorrectDigest` gains `adopts: Option<ProposalId>`.
- `TreeEdit::from_change(change, &ChangeContext)` with `ChangeContext { new_item_id,
  new_item_kind, proposal_id, proposer: Provenance }`; still exhaustive over `Change`.
- `VersionStore::commit` and `create_candidate` refuse a path collision (section 7).
- `WorkspaceCatalog::list(tenant, include_archived: bool)`; new `unarchive(scope, MutationId,
  author)`; `archive` and `unarchive` set `archived_at`.
- New port `Purger` (storage implements; one call owns every store it must clear):
  `purge_workspace(scope, MutationId, PurgeId) -> PurgeReport` and `purge_item(scope, MutationId,
  PurgeId, ItemId) -> PurgeReport`. Idempotent on the mutation id and resumable after a crash;
  returns only when no managed backup or retained export holds the target, and counts objects
  kept for surviving references.
- New port `Backups` (storage implements): `write_backup(scope, MutationId) -> ObjectInfo` (the
  archive, retained; the handler records it with `ArtifactKind::Backup`), `restore_import(scope,
  archive: Digest, MutationId, editors: Vec<String>) -> RestoreReport` (`editors`: subjects with
  `write` on the target, computed by core from `AccessControl`). The replacement restore is a
  storage function the server command calls, not a core port.

### 9.4 Search and events

- `SearchQuery` gains `extraction: Option<ExtractionFilter>`; `text` may be empty only with a
  filter. The index stores each source's `ExtractionStatus` from its card.
- `EventLog` takes `EventScope { Tenant(TenantId), Workspace(StorageScope) }` for `append` and
  `list`; `NewEvent` gains `connector_id`, `actor: Option<EventActor>` and `operation`; the store
  stamps `at`.
- `access::check_draft_route` becomes `check_human_route(principal, operation)`, refusing
  `DRAFT_BEARING` and `HUMAN_SESSION_ONLY` operations on `mcp_delegation` and `service`.
  Dispatch records `permission_denied` when authorization refuses an authenticated caller.

### 9.5 Application duties these ports enable

- Citations: core fills `SourceReference::locations` from the `ConversionRecord` of the cited
  digest (lines of the shown converter text), `corrected_text` or `supplied_text` otherwise, and
  recomputes request-supplied locations (section 2).
- Datasets: `present_view` and `resolve_view` materialize each binding from the record's
  `ConvertedTable` that the binding's selection covers (or the cells of an XLSX `cells`
  selection), write the `Dataset` blob, and return `charts`. This is the "producer of a View's
  materialized dataset" of the gate.
- `get_object` serves only an object that belongs to the cited item revision (the gate's
  remaining item) and never a backup artifact's object.

## 10. Operation table delta

Inserted rows, exact, in table order (with matching `OperationName` variants):

```rust
// after archive_workspace
(unarchive_workspace, $crate::workspace::UnarchiveWorkspaceRequest, $crate::common::MutationResult, "/api/workspaces/unarchive-workspace", "Unarchive", "", "", "", Admin, "", 200, false, "Return an archived workspace to ordinary listings; its history and records were kept."),
(purge_workspace, $crate::purge::PurgeWorkspaceRequest, $crate::purge::Purge, "/api/workspaces/purge-workspace", "Purge", "", "", "", Admin, "", 202, true, "Permanently remove a workspace's originals, derivatives, index entries, history, managed backups and retained exports, and mark what referred to them invalidated. Never claims to erase copies outside the application. Human administrator session only."),
(get_purge, $crate::purge::GetPurgeRequest, $crate::purge::Purge, "/api/workspaces/get-purge", "Purge status", "", "", "", Admin, "", 200, false, "Read a purge's progress and counts; available after the workspace is gone."),
// after delete_item
(purge_item, $crate::purge::PurgeItemRequest, $crate::purge::Purge, "/api/items/purge-item", "Purge", "", "", "", Admin, "", 202, true, "Permanently remove one item's bytes, derivatives, index entries and history, rewrite or delete the managed backups that hold it, and mark what referred to it invalidated. Human administrator session only."),
// after list_events
(list_tenant_events, $crate::events::ListTenantEventsRequest, $crate::events::ListEventsResponse, "/api/events/list-tenant-events", "Security activity", "", "", "", Admin, "", 200, false, "Read installation-level notifications: sign-ins, connector issue and revocation, and refusals outside a workspace."),
```

Changed rows: `archive_workspace` destructive `false`; descriptions of `list_items`,
`search_items`, `open_proposal` (section 5), `backup_workspace` and `restore_workspace`
(section 8).

`RequestScope`: `UnarchiveWorkspaceRequest`, `PurgeWorkspaceRequest`, `PurgeItemRequest` in
`workspace_keyed!(Admin: ...)`; `GetPurgeRequest` and `ListTenantEventsRequest` target
`Deployment(Admin)` with no key; `RestoreWorkspaceRequest` adds the source workspace for an
artifact source; `SearchRequest` and `OpenProposalRequest` gain `check_rules` (section 5).

None of the new operations is an MCP tool. Totals: 74 operations, 12 model tools, 1 app tool.

## 11. SPEC sentences

Unchanged sentences this design satisfies are quoted in each section. Proposed changes, exact:

§12, replace "Portable export gathers references into an independently readable folder/archive.
Full backup additionally preserves promised app records, retained objects and versions, except
what Purge (§8) removed. Restore must be exercised." with:

> Portable export gathers committed content at one revision, and the originals and derivatives
> it references, into an independently readable folder/archive; it never contains drafts,
> sessions, credentials or application records. A full backup of a workspace is independently
> recoverable: one self-contained archive of a consistent checkpoint of its versions, retained
> objects and application records, drafts included, except what Purge (§8) removed; it never
> depends on the live object store. Only a workspace administrator in a human browser session
> takes or downloads a backup: the download boundary decides by artifact kind and route, so no
> agent, service route, export or object read returns one, and the custodian of an unencrypted
> backup can read it. A backup restores in two modes. Replacing the installation it came from
> runs against a data directory with no running service and restores the original identities
> and drafts. Importing into another installation fills a blank workspace, gives each draft to
> its editor when that editor is a validated identity there, and keeps the other drafts in the
> backup, unassigned, with their count reported. Restore must be exercised, including after the
> original data directory is deleted.

§8, after "Purge never claims to erase exports, clones or backups outside the application's
control.", add:

> A purge does not complete while a managed backup or retained export that holds the target can
> still be restored or downloaded; stored objects that a surviving item still references are
> kept and counted, and a record that only hides the target is not a purge. The purge leaves a
> record of what was removed, by identity, with no content.

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

Every change above alters `api/`, `generated/cli/` and `ui/src/api/generated/`, including the
generated Zod inside the MCP App bundle and the schemas of the `workspaces`, `ls`, `grep`, `show`,
`propose` and `present` tools. The `mcp-apps-protocol-qualification` receipt goes stale with them
and is requalified after the contract lands, as after the pre-lane corrections. The harness's
dataset fixture becomes a `Dataset` document. The views lane's
`views-chart-failure-isolation` gate consumes `charts`.

## 13. In the contract gate but outside these eight shapes

The gate `contract-expresses-extraction-sources-views` also carries owner decisions this bounded
list does not design, so these eight shapes alone do not close it: status words Draft, Stable and
Deprecated with Archived as a separate flag (today `Lifecycle` has `active`, `deprecated`,
`archived`); an app review written as OKF `verified` on export, and an imported `verified` read as
an unconfirmed claim; the item id in the file's header; source card naming (`report.pdf` becomes
`report-pdf.md`, with the original name in its header); a header that does not parse needs
attention and blocks nothing; a snapshot conflict resolved per item by keep mine or take theirs.
The ledger also records idempotency keys not scoped by route (pre-lane concern 4).

## 14. Owner questions

1. **Backup unit.** The ledger says a backup is qualified "by restoring after deleting the
   original data dir", which reads as an installation, while jobs, permissions and purge are
   per workspace. Recommended default: per-workspace self-contained archives (section 8), with
   replacement restore as an offline server command that takes the archives of every workspace,
   so deleting the data directory and restoring is still the qualification.
2. **Unrelated citations after an item purge.** Removing an item from Git history rewrites every
   later commit, so revisions cited for other, unchanged items stop existing. Recommended
   default: a citation of an item whose content digest is unchanged is remapped to the rewritten
   revision and its review coverage kept; a citation of the purged item is invalidated. This
   decides only item purge, which is built after workspace purge.
