//! Conversion outcome, converter identity, settings and supplied text of a source.
//!
//! The converter's verdict (`ConversionOutcome`) is recorded once per conversion and is never
//! changed by text an agent supplied or a person corrected: those change only `TextOrigin` and
//! `corrected`. A partly converted paginated source names the pages that were not converted in
//! its `PageCoverage`, so a retry can convert exactly those.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::PageRange;
use crate::error::{ApiError, ErrorCode};

/// The converter's verdict on one source, as a listing and a filter use it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversionOutcome {
    /// Accepted; the card was committed before conversion and shows no text.
    Pending,
    /// Docling's `Success`.
    Completed,
    /// Docling's `PartialSuccess`, or a windowed conversion with windows not converted.
    Partial {
        /// Pages converted and not converted; absent for a format docling converts whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coverage: Option<PageCoverage>,
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
/// The three lists are ascending, disjoint, and together cover exactly `1..=page_count`;
/// `check` enforces it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PageCoverage {
    /// Pages in the original.
    #[schemars(range(min = 1))]
    pub page_count: u32,
    /// Pages converted with no known loss.
    pub converted: Vec<PageRange>,
    /// Pages converted whose text is partly undecodable (`ExtractionWarning::UndecodableGlyphs`).
    pub partly_extracted: Vec<PageRange>,
    /// Pages with no extraction; a retry converts exactly these.
    pub not_converted: Vec<PageRange>,
}

/// Why a conversion produced no document.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FailureReason {
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConverterIssue {
    /// `document_backend`, `model`, `doc_assembler` or `user_input`.
    pub component_type: String,
    /// Stage that recorded it, such as `pipeline`.
    pub module_name: String,
    /// The converter's message, with every worker filesystem path reduced to its file name.
    pub error_message: String,
}

/// The converter that produced a digest; part of the digest's identity (SPEC section 4).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConverterIdentity {
    /// `docling`.
    pub name: String,
    /// The docling crate version, such as `2.3.0`.
    pub version: String,
    /// The converter crates as Cargo.lock resolves them; a patched fork is visible here.
    /// Generated from Cargo.lock by xtask into `generated/converter/packages.json`, so
    /// gen-check holds it to the lock.
    pub packages: Vec<ConverterPackage>,
    /// Settings applied.
    pub settings: ConversionSettings,
    /// Model files the pipeline loads, hashed once when the worker starts; empty for formats
    /// that load none.
    pub models: Vec<ModelIdentity>,
    /// Pages per conversion window; absent when the document was converted whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_window: Option<u32>,
}

/// One converter crate, as Cargo.lock records it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConverterPackage {
    /// Crate name, such as `docling-pdf`.
    pub name: String,
    /// Crate version.
    pub version: String,
    /// Cargo.lock `source`, such as `git+https://github.com/Heyoub/docling.rs?rev=...`.
    pub source: String,
}

/// One model file of `docling::model_inventory()`, hashed by the worker at startup.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtractionWarning {
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
    /// formats were not extracted (SPEC section 5 formula/value/format distinction).
    CellValuesOnly {
        /// Sheets read, in workbook order.
        sheets: Vec<String>,
    },
}

/// Undecodable glyphs on one page.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PageGlyphs {
    /// One-based page.
    #[schemars(range(min = 1))]
    pub page_no: u32,
    /// Placeholders the library printed on that page.
    pub glyphs: u32,
}

/// Whose text a source card shows and search indexes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextOrigin {
    /// The converter's extraction.
    Converter,
    /// Text an agent proposed and a person accepted.
    SuppliedByAgent,
    /// No text: pending, failed or unsupported, with nothing supplied.
    None,
}

/// What a listing shows about one source.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtractionSummary {
    /// The converter's verdict.
    pub status: ExtractionStatus,
    /// Whose text is shown.
    pub text_origin: TextOrigin,
    /// A human correction is shown over that text.
    pub corrected: bool,
}

/// Everything recorded about turning one source occurrence's bytes into text.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Extraction {
    /// What the converter did.
    pub outcome: ConversionOutcome,
    /// Present exactly when a converter ran (not for `pending` or `unsupported`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converter: Option<ConverterIdentity>,
    /// The retained conversion record; present exactly when a document was produced
    /// (`completed`, `partial`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<crate::identity::Digest>,
    /// Pages in the original, for a paginated format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    /// Whose text is shown.
    pub text_origin: TextOrigin,
    /// A human correction is shown over that text.
    pub corrected: bool,
    /// The latest accepted agent-supplied text, shown or kept in history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supplied: Option<SuppliedText>,
    /// Findings of the conversion.
    pub warnings: Vec<ExtractionWarning>,
}

/// When optical character recognition runs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrPolicy {
    /// Recognise only regions with no embedded text layer (docling default).
    Auto,
    /// Never run recognition; layout and tables are still detected (docling `skip_ocr`).
    Skip,
    /// Recognise every page from its render (docling `force_full_page_ocr`).
    ForceFullPage,
}

/// Converter settings; part of the identity of the digest they produce.
///
/// `Default` is a Rust convenience for a deployment's defaults, never a serde default: every
/// field is required on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversionSettings {
    /// When recognition runs.
    pub ocr: OcrPolicy,
    /// Docling `ocr_lang`, such as `en` or `ch`; absent uses docling's default (`en`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_language: Option<String>,
    /// Reconstruct table structure with the model (docling `no_table_former` off).
    pub table_structure: bool,
    /// Keep a render of every page (docling `generate_page_images`).
    pub page_images: bool,
    /// Render resolution; docling `images_scale` = this / 72. At most 144, docling's own render
    /// resolution: a higher scale only upsamples that render and adds no detail.
    #[schemars(range(min = 36, max = 144))]
    pub page_image_dpi: u16,
}

/// A filter over sources by extraction.
///
/// One variant on purpose: a defaulted `bool` would split the one wire shape, and `Option<bool>`
/// would have a third state with no meaning.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionFilter {
    /// Sources whose converter status is `partial`, `failed` or `unsupported`, whether or not
    /// text was supplied for them.
    Unprocessed,
}

/// Accepted agent-supplied text of one source.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SuppliedText {
    /// The accepted proposal.
    pub proposal_id: crate::identity::ProposalId,
    /// Subject of the agent that proposed it, recorded by the server.
    pub supplied_by: String,
    /// OAuth or connector client it came through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Subject who accepted the proposal.
    pub approved_by: String,
    /// When it was accepted.
    pub approved_at: crate::identity::Timestamp,
    /// SHA-256 of the supplied Markdown.
    pub content_digest: crate::identity::Digest,
    /// Pages the agent says the text covers: a claim, never a location; empty for the whole.
    pub pages: Vec<PageRange>,
    /// Who chose this text over a later complete conversion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_by: Option<String>,
}

/// Lowest page render resolution `ConversionSettings::page_image_dpi` accepts.
pub const MIN_PAGE_IMAGE_DPI: u16 = 36;
/// Highest page render resolution: docling renders a page once at 2.0 px/pt (144 dpi).
pub const MAX_PAGE_IMAGE_DPI: u16 = 144;

impl ExtractionStatus {
    /// Whether a source in this status is listed as unprocessed: `partial`, `failed` or
    /// `unsupported`.
    #[must_use]
    pub const fn is_unprocessed(self) -> bool {
        matches!(self, Self::Partial | Self::Failed | Self::Unsupported)
    }
}

impl ConversionOutcome {
    /// The status a listing and the `unprocessed` filter use.
    #[must_use]
    pub const fn status(&self) -> ExtractionStatus {
        match self {
            Self::Pending => ExtractionStatus::Pending,
            Self::Completed => ExtractionStatus::Completed,
            Self::Partial { .. } => ExtractionStatus::Partial,
            Self::Failed { .. } => ExtractionStatus::Failed,
            Self::Unsupported => ExtractionStatus::Unsupported,
        }
    }
}

impl PageCoverage {
    /// Refuse coverage of no pages, and coverage whose lists are not ascending, overlap, leave
    /// a gap, or reach past `page_count`.
    ///
    /// # Errors
    /// Returns `InvalidInput` whose field is a JSON Pointer, relative to the coverage, to the
    /// first range at fault, or to `/page_count` when it is zero or the pages end short of it.
    pub fn check(&self) -> Result<(), ApiError> {
        // A paginated original has at least one page; the schema's minimum is not enough,
        // because a value built in Rust or read from a stored record never meets the schema.
        if self.page_count == 0 {
            return Err(coverage_error(
                "a paginated original has at least one page",
                "/page_count".to_owned(),
            ));
        }
        let lists: [(&str, &[PageRange]); 3] = [
            ("converted", &self.converted),
            ("partly_extracted", &self.partly_extracted),
            ("not_converted", &self.not_converted),
        ];
        let mut all: Vec<(&PageRange, String)> = Vec::new();
        for (name, ranges) in lists {
            let mut previous_end = 0_u32;
            for (index, range) in ranges.iter().enumerate() {
                let field = format!("/{name}/{index}");
                if range.start == 0 || range.start > range.end {
                    return Err(coverage_error(
                        "a page range runs from one page to a later one",
                        field,
                    ));
                }
                if range.end > self.page_count {
                    return Err(coverage_error(
                        "a page range reaches past the page count",
                        field,
                    ));
                }
                if range.start <= previous_end {
                    return Err(coverage_error(
                        "page ranges of one list are ascending and do not overlap",
                        field,
                    ));
                }
                previous_end = range.end;
                all.push((range, field));
            }
        }
        all.sort_by_key(|(range, _)| range.start);
        let mut next = 1_u32;
        for (range, field) in all {
            if range.start != next {
                let message = if range.start < next {
                    "the coverage lists overlap"
                } else {
                    "the coverage lists leave a page uncovered"
                };
                return Err(coverage_error(message, field));
            }
            next = range.end.saturating_add(1);
        }
        if next.checked_sub(1) == Some(self.page_count) {
            Ok(())
        } else {
            Err(coverage_error(
                "the coverage lists leave a page uncovered",
                "/page_count".to_owned(),
            ))
        }
    }
}

impl Extraction {
    /// What a listing shows about this source.
    #[must_use]
    pub const fn summary(&self) -> ExtractionSummary {
        ExtractionSummary {
            status: self.outcome.status(),
            text_origin: self.text_origin,
            corrected: self.corrected,
        }
    }
}

impl ConversionSettings {
    /// Refuse a render resolution outside 36 to 144 dpi.
    ///
    /// # Errors
    /// Returns `InvalidInput` on `/settings/page_image_dpi`.
    pub fn check(&self) -> Result<(), ApiError> {
        if (MIN_PAGE_IMAGE_DPI..=MAX_PAGE_IMAGE_DPI).contains(&self.page_image_dpi) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::InvalidInput,
                format!(
                    "page_image_dpi must be between {MIN_PAGE_IMAGE_DPI} and {MAX_PAGE_IMAGE_DPI}"
                ),
            )
            .with_field("/settings/page_image_dpi"))
        }
    }
}

impl Default for ConversionSettings {
    /// Recognition only where no text layer exists, table structure on, and a 144 dpi render
    /// of every page, which a `region` citation is drawn on.
    fn default() -> Self {
        Self {
            ocr: OcrPolicy::Auto,
            ocr_language: None,
            table_structure: true,
            page_images: true,
            page_image_dpi: MAX_PAGE_IMAGE_DPI,
        }
    }
}

fn coverage_error(message: &str, field: String) -> ApiError {
    ApiError::new(ErrorCode::InvalidInput, message).with_field(field)
}
