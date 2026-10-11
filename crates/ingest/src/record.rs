//! Assembling one `ConversionRecord` and the contract outcome from the conversion's windows.
//!
//! Stage 1b design section 9.1 says how. The Markdown of the windows is joined in page order,
//! and each window's line ranges are shifted onto the joined text. There is one `WindowExport`
//! per window that produced a document, naming the lines of the joined text that window
//! produced (`None` when it produced no text), so a redigest of only the unconverted pages can
//! replace exactly those ranges. Each `WindowCoverage` is checked and joined into the
//! contract `PageCoverage`, which is then checked over `1..=page_count`. The outline is
//! computed again over the joined Markdown (`outline`).
//!
//! A window that produced no document is `not_converted` pages, not a failed document,
//! whatever its reason: the cap (`Failure(MemoryLimit)`), the time bound, a crash. Only when no
//! window produced anything is the outcome `Failed`, with the first window's reason. A page
//! with undecodable text is `partly_extracted`. Its coverage can only be carried by `Partial`,
//! because `Completed` has no coverage.
//!
//! A format converted whole has one window (`None`), and its status maps straight onto the
//! outcome. `Unsupported` is a property of the format, so it ends the whole document.

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConverterIdentity, ConverterIssue, ExtractionWarning, FailureReason,
    PageCoverage, PageGlyphs,
};
use okf_jawn_contract::identity::Digest;
use okf_jawn_core::conversion::{
    ConversionRecord, ConversionStatus, ConvertedTable, LineLocation, RetainedAsset,
    WindowCoverage, WindowExport,
};
use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

use crate::outline::{line_count, outline_of};

/// What one window produced, once its export and assets are retained.
#[derive(Debug, Clone)]
pub struct ConvertedWindow {
    /// The window's Markdown, placeholders already replaced (`glyphs::scrub_placeholders`).
    pub markdown: String,
    /// Digest of the retained docling JSON export of the window.
    pub export: Digest,
    /// Line-to-location map, lines relative to `markdown`.
    pub locations: Vec<LineLocation>,
    /// Tables, lines relative to `markdown`.
    pub tables: Vec<ConvertedTable>,
    /// Retained page renders and pictures.
    pub assets: Vec<RetainedAsset>,
    /// What the converter or the worker noticed.
    pub warnings: Vec<ExtractionWarning>,
}

/// The outcome of one window.
#[derive(Debug, Clone)]
pub struct WindowOutcome {
    /// The window; `None` for a format converted whole.
    pub window: Option<PageRange>,
    /// How the window ended.
    pub status: ConversionStatus,
    /// What the window converted; present for a document produced in a paginated window.
    pub coverage: Option<WindowCoverage>,
    /// What the converter recorded.
    pub issues: Vec<ConverterIssue>,
    /// The document; present exactly when the status is `Success` or `PartialSuccess`.
    pub converted: Option<ConvertedWindow>,
}

/// The assembled result of a conversion.
#[derive(Debug, Clone)]
pub struct Assembled {
    /// The converter's verdict over the whole document.
    pub outcome: ConversionOutcome,
    /// The record to retain; present exactly when a document was produced.
    pub record: Option<ConversionRecord>,
    /// The joined Markdown the record describes; empty when no document was produced.
    pub markdown: String,
}

/// Assemble the windows of one conversion, in page order.
///
/// # Errors
/// Returns `Internal` when the windows are not what the worker was asked for: a paginated
/// window without coverage, coverage of another window, coverage that fails its check, a
/// status that disagrees with the presence of a document, or joined coverage that does not
/// cover `1..=page_count`. Each of these is a worker fault.
pub fn assemble(
    converter: ConverterIdentity,
    page_count: Option<u32>,
    windows: Vec<WindowOutcome>,
) -> Result<Assembled, ApiError> {
    for window in &windows {
        check_window(window, page_count.is_some())?;
    }
    if windows
        .iter()
        .any(|window| window.status == ConversionStatus::Unsupported)
    {
        return Ok(nothing(ConversionOutcome::Unsupported));
    }
    let issues: Vec<ConverterIssue> = windows
        .iter()
        .flat_map(|window| window.issues.iter().cloned())
        .collect();
    let produced = windows.iter().any(|window| window.converted.is_some());
    if !produced {
        let reason = windows
            .iter()
            .find_map(|window| match &window.status {
                ConversionStatus::Failure(reason) => Some(reason.clone()),
                ConversionStatus::Success
                | ConversionStatus::PartialSuccess
                | ConversionStatus::Unsupported => None,
            })
            .unwrap_or(FailureReason::ConverterError);
        return Ok(nothing(ConversionOutcome::Failed { reason, issues }));
    }
    let coverage = match page_count {
        Some(pages) => Some(joined_coverage(pages, &windows)?),
        None => None,
    };
    let any_partial = windows
        .iter()
        .any(|window| window.status == ConversionStatus::PartialSuccess);
    let complete = coverage.as_ref().is_none_or(|coverage| {
        coverage.partly_extracted.is_empty() && coverage.not_converted.is_empty()
    });
    let outcome = if complete && !any_partial {
        ConversionOutcome::Completed
    } else {
        ConversionOutcome::Partial {
            coverage,
            issues: issues.clone(),
        }
    };
    let (markdown, record) = join(converter, page_count, outcome.clone(), windows)?;
    Ok(Assembled {
        outcome,
        record: Some(record),
        markdown,
    })
}

/// The windows of a document of `page_count` pages, `window_pages` pages each.
#[must_use]
pub fn windows_of(page_count: u32, window_pages: u32) -> Vec<PageRange> {
    windows_within(
        &[PageRange {
            start: 1,
            end: page_count,
        }],
        window_pages,
    )
}

/// The given page ranges cut into windows of at most `window_pages` pages, in order. A range
/// whose start is 0 or after its end gives none.
#[must_use]
pub fn windows_within(ranges: &[PageRange], window_pages: u32) -> Vec<PageRange> {
    let size = window_pages.max(1);
    let mut windows = Vec::new();
    for range in ranges {
        if range.start == 0 || range.start > range.end {
            continue;
        }
        let mut start = range.start;
        while start <= range.end {
            let end = start.saturating_add(size.saturating_sub(1)).min(range.end);
            windows.push(PageRange { start, end });
            let Some(next) = end.checked_add(1) else {
                break;
            };
            start = next;
        }
    }
    windows
}

/// Lowercase hex SHA-256 of `bytes`, as the contract's `Digest`.
///
/// # Errors
/// Never in practice: a SHA-256 is always 64 lowercase hex characters. The contract type is
/// fallible, so a broken hasher is a worker fault, not a panic.
pub fn sha256_digest(bytes: &[u8]) -> Result<Digest, ApiError> {
    let hash = Sha256::digest(bytes);
    let hex = hash
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _written = write!(hex, "{byte:02x}");
            hex
        });
    Digest::try_from(hex).map_err(|error| fault(&format!("a SHA-256 was not a digest: {error}")))
}

/// Refuse a window whose parts disagree.
fn check_window(window: &WindowOutcome, paginated: bool) -> Result<(), ApiError> {
    let produced = matches!(
        window.status,
        ConversionStatus::Success | ConversionStatus::PartialSuccess
    );
    if produced != window.converted.is_some() {
        return Err(fault(
            "a window has a document exactly when it converted with success or partial success",
        ));
    }
    if paginated != window.window.is_some() {
        return Err(fault(
            "a paginated document is converted in windows, any other whole",
        ));
    }
    if let (Some(expected), true) = (&window.window, produced) {
        let coverage = window
            .coverage
            .as_ref()
            .ok_or_else(|| fault("a converted window reports its coverage"))?;
        if &coverage.window != expected {
            return Err(fault("a window's coverage is of another window"));
        }
        coverage.check()?;
    }
    Ok(())
}

/// The contract coverage of a paginated document, joined from its windows and checked.
fn joined_coverage(page_count: u32, windows: &[WindowOutcome]) -> Result<PageCoverage, ApiError> {
    let mut converted = Vec::new();
    let mut partly_extracted = Vec::new();
    let mut not_converted = Vec::new();
    for window in windows {
        match (&window.coverage, &window.converted, &window.window) {
            (Some(coverage), Some(_), _) => {
                extend(&mut converted, &coverage.converted);
                extend(&mut partly_extracted, &coverage.partly_extracted);
                extend(&mut not_converted, &coverage.not_converted);
            }
            (_, None, Some(range)) => extend(&mut not_converted, std::slice::from_ref(range)),
            (_, _, _) => return Err(fault("a converted window reports its coverage")),
        }
    }
    let coverage = PageCoverage {
        page_count,
        converted,
        partly_extracted,
        not_converted,
    };
    coverage.check().map_err(|error| {
        let mut fault = fault(&format!(
            "the joined coverage is not whole: {}",
            error.message
        ));
        fault.field = error.field;
        fault
    })?;
    Ok(coverage)
}

/// Append ranges to a list, merging a range that continues the last one.
fn extend(list: &mut Vec<PageRange>, ranges: &[PageRange]) {
    for range in ranges {
        match list.last_mut() {
            Some(previous) if previous.end.checked_add(1) == Some(range.start) => {
                previous.end = range.end;
            }
            Some(_) | None => list.push(range.clone()),
        }
    }
}

/// Join the produced windows into one Markdown text and its record.
///
/// # Errors
/// Returns `Internal` when the Markdown cannot be hashed into a digest.
fn join(
    converter: ConverterIdentity,
    page_count: Option<u32>,
    outcome: ConversionOutcome,
    windows: Vec<WindowOutcome>,
) -> Result<(String, ConversionRecord), ApiError> {
    let mut markdown = String::new();
    let mut structured = Vec::new();
    let mut locations = Vec::new();
    let mut tables = Vec::new();
    let mut assets = Vec::new();
    let mut warnings = Vec::new();
    for window in windows {
        let Some(produced) = window.converted else {
            continue;
        };
        if !produced.markdown.is_empty() && !markdown.is_empty() {
            if !markdown.ends_with('\n') {
                markdown.push('\n');
            }
            markdown.push('\n');
        }
        let offset = if produced.markdown.is_empty() {
            0
        } else {
            line_count(&markdown)
        };
        markdown.push_str(&produced.markdown);
        let produced_lines = line_count(&produced.markdown);
        structured.push(WindowExport {
            window: window.window,
            digest: produced.export,
            lines: (produced_lines > 0).then(|| TextRange {
                start: offset.saturating_add(1),
                end: offset.saturating_add(produced_lines),
            }),
        });
        locations.extend(produced.locations.into_iter().map(|mut location| {
            location.lines = shifted(&location.lines, offset);
            location
        }));
        tables.extend(produced.tables.into_iter().map(|mut table| {
            table.lines = shifted(&table.lines, offset);
            table
        }));
        assets.extend(produced.assets);
        for warning in produced.warnings {
            merge_warning(&mut warnings, warning);
        }
    }
    let record = ConversionRecord {
        converter,
        outcome,
        page_count,
        structured,
        markdown: sha256_digest(markdown.as_bytes())?,
        locations,
        outline: outline_of(&markdown),
        tables,
        assets,
        warnings,
    };
    Ok((markdown, record))
}

/// Add one window's warning to the document's, summing counts of the same kind.
fn merge_warning(warnings: &mut Vec<ExtractionWarning>, warning: ExtractionWarning) {
    for existing in warnings.iter_mut() {
        match (existing, &warning) {
            (
                ExtractionWarning::UndecodableGlyphs {
                    pages,
                    unlocated_glyphs,
                },
                ExtractionWarning::UndecodableGlyphs {
                    pages: more,
                    unlocated_glyphs: more_unlocated,
                },
            ) => {
                for page in more {
                    match pages.iter_mut().find(|known| known.page_no == page.page_no) {
                        Some(known) => known.glyphs = known.glyphs.saturating_add(page.glyphs),
                        None => pages.push(PageGlyphs {
                            page_no: page.page_no,
                            glyphs: page.glyphs,
                        }),
                    }
                }
                pages.sort_by_key(|page| page.page_no);
                *unlocated_glyphs = unlocated_glyphs.saturating_add(*more_unlocated);
                return;
            }
            (
                ExtractionWarning::InferredLocations { items },
                ExtractionWarning::InferredLocations { items: more },
            )
            | (
                ExtractionWarning::UnlocatedItems { items },
                ExtractionWarning::UnlocatedItems { items: more },
            ) => {
                *items = items.saturating_add(*more);
                return;
            }
            (
                ExtractionWarning::CellValuesOnly { sheets },
                ExtractionWarning::CellValuesOnly { sheets: more },
            ) => {
                sheets.extend(more.iter().cloned());
                return;
            }
            (_, _) => {}
        }
    }
    warnings.push(warning);
}

/// A line range moved down by `offset` lines.
fn shifted(range: &TextRange, offset: u32) -> TextRange {
    TextRange {
        start: range.start.saturating_add(offset),
        end: range.end.saturating_add(offset),
    }
}

/// An outcome with no document.
fn nothing(outcome: ConversionOutcome) -> Assembled {
    Assembled {
        outcome,
        record: None,
        markdown: String::new(),
    }
}

/// A worker fault.
fn fault(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
