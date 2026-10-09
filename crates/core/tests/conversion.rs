//! A window's coverage is checked before ingest joins it, and the retained record keeps one
//! shape through the blob store.

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, PageCoverage,
};
use okf_jawn_contract::identity::Digest;
use okf_jawn_contract::read::{AssetRole, OutlineEntry, OutlineEntryKind, Selection};
use okf_jawn_contract::source::{SourceLocation, SourceLocator, UnresolvedReason};
use okf_jawn_core::conversion::{
    ConversionRecord, ConvertedCell, ConvertedTable, LineLocation, PixelSize, RetainedAsset,
    WindowCoverage, WindowExport,
};

use check::{TestResult, err_of};

fn pages(start: u32, end: u32) -> PageRange {
    PageRange { start, end }
}

fn coverage(
    window: PageRange,
    converted: Vec<PageRange>,
    not_converted: Vec<PageRange>,
) -> WindowCoverage {
    WindowCoverage {
        window,
        converted,
        partly_extracted: Vec::new(),
        not_converted,
    }
}

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn identity(page_window: Option<u32>) -> ConverterIdentity {
    ConverterIdentity {
        name: "docling".to_owned(),
        version: "2.3.0".to_owned(),
        packages: Vec::new(),
        settings: ConversionSettings::default(),
        models: Vec::new(),
        page_window,
    }
}

#[test]
fn a_window_covered_exactly_passes() -> TestResult {
    coverage(
        pages(5, 8),
        vec![pages(5, 6), pages(8, 8)],
        vec![pages(7, 7)],
    )
    .check()?;
    let mixed = WindowCoverage {
        window: pages(1, 4),
        converted: vec![pages(1, 1)],
        partly_extracted: vec![pages(2, 3)],
        not_converted: vec![pages(4, 4)],
    };
    mixed.check()?;
    Ok(())
}

#[test]
fn a_page_of_the_window_left_uncovered_is_refused() -> TestResult {
    let gap = err_of(coverage(pages(5, 8), vec![pages(5, 6)], vec![pages(8, 8)]).check())?;
    assert_eq!(gap.code, ErrorCode::Internal);
    assert_eq!(gap.field.as_deref(), Some("/not_converted/0"));
    let short = err_of(coverage(pages(5, 8), vec![pages(5, 7)], Vec::new()).check())?;
    assert_eq!(short.field.as_deref(), Some("/window"));
    Ok(())
}

#[test]
fn a_range_outside_its_window_is_refused() -> TestResult {
    let before = err_of(coverage(pages(5, 8), vec![pages(4, 8)], Vec::new()).check())?;
    assert_eq!(before.field.as_deref(), Some("/converted/0"));
    let after = err_of(coverage(pages(5, 8), vec![pages(5, 9)], Vec::new()).check())?;
    assert_eq!(after.field.as_deref(), Some("/converted/0"));
    Ok(())
}

#[test]
fn overlapping_or_unordered_ranges_are_refused() -> TestResult {
    let overlap = err_of(coverage(pages(1, 4), vec![pages(1, 3)], vec![pages(3, 4)]).check())?;
    assert_eq!(overlap.field.as_deref(), Some("/not_converted/0"));
    let unordered =
        err_of(coverage(pages(1, 4), vec![pages(3, 4), pages(1, 2)], Vec::new()).check())?;
    assert_eq!(unordered.field.as_deref(), Some("/converted/1"));
    let reversed = err_of(coverage(pages(1, 4), vec![pages(4, 1)], Vec::new()).check())?;
    assert_eq!(reversed.field.as_deref(), Some("/converted/0"));
    Ok(())
}

#[test]
fn a_window_that_is_not_a_page_range_is_refused() -> TestResult {
    let zero = err_of(coverage(pages(0, 3), vec![pages(1, 3)], Vec::new()).check())?;
    assert_eq!(zero.field.as_deref(), Some("/window"));
    let reversed = err_of(coverage(pages(3, 2), Vec::new(), Vec::new()).check())?;
    assert_eq!(reversed.field.as_deref(), Some("/window"));
    Ok(())
}

#[test]
fn checked_windows_join_into_whole_document_coverage() -> TestResult {
    // Ingest's merge: each window checked, the lists joined, then the contract check over
    // `1..=page_count`. A window whose child hit the memory cap is all `not_converted`.
    let windows = [
        coverage(pages(1, 4), vec![pages(1, 4)], Vec::new()),
        coverage(pages(5, 8), Vec::new(), vec![pages(5, 8)]),
        coverage(pages(9, 10), vec![pages(9, 10)], Vec::new()),
    ];
    let mut whole = PageCoverage {
        page_count: 10,
        converted: Vec::new(),
        partly_extracted: Vec::new(),
        not_converted: Vec::new(),
    };
    for window in &windows {
        window.check()?;
        whole.converted.extend(window.converted.iter().cloned());
        whole
            .partly_extracted
            .extend(window.partly_extracted.iter().cloned());
        whole
            .not_converted
            .extend(window.not_converted.iter().cloned());
    }
    whole.check()?;
    assert_eq!(whole.not_converted, vec![pages(5, 8)]);
    Ok(())
}

fn record() -> Result<ConversionRecord, Box<dyn std::error::Error>> {
    let page_two = SourceLocation::Direct {
        locator: SourceLocator::Page { page_no: 2 },
    };
    Ok(ConversionRecord {
        converter: identity(Some(4)),
        outcome: ConversionOutcome::Partial {
            coverage: Some(PageCoverage {
                page_count: 6,
                converted: vec![pages(1, 4)],
                partly_extracted: Vec::new(),
                not_converted: vec![pages(5, 6)],
            }),
            issues: Vec::new(),
        },
        page_count: Some(6),
        structured: vec![WindowExport {
            window: Some(pages(1, 4)),
            digest: digest('a')?,
        }],
        markdown: digest('b')?,
        locations: vec![
            LineLocation {
                lines: TextRange { start: 1, end: 3 },
                location: page_two.clone(),
            },
            LineLocation {
                lines: TextRange { start: 4, end: 4 },
                location: SourceLocation::Unresolved {
                    reason: UnresolvedReason::AmbiguousMatch { occurrences: 2 },
                },
            },
        ],
        outline: vec![OutlineEntry {
            label: "Results".to_owned(),
            level: 1,
            selection: Selection::Lines {
                range: TextRange { start: 1, end: 7 },
            },
            kind: OutlineEntryKind::Heading,
        }],
        tables: vec![ConvertedTable {
            lines: TextRange { start: 5, end: 7 },
            location: page_two.clone(),
            num_rows: 2,
            num_cols: 1,
            cells: vec![
                ConvertedCell {
                    row: 0,
                    column: 0,
                    row_span: 1,
                    column_span: 1,
                    text: "Quarter".to_owned(),
                    column_header: true,
                    row_header: false,
                },
                ConvertedCell {
                    row: 1,
                    column: 0,
                    row_span: 1,
                    column_span: 1,
                    text: "Q1".to_owned(),
                    column_header: false,
                    row_header: false,
                },
            ],
        }],
        assets: vec![RetainedAsset {
            digest: digest('c')?,
            media_type: "image/png".to_owned(),
            role: AssetRole::PageImage,
            location: page_two,
            pixel_size: Some(PixelSize {
                width: 1240,
                height: 1754,
            }),
            caption: None,
        }],
        warnings: Vec::new(),
    })
}

#[test]
fn a_conversion_record_round_trips_through_json() -> TestResult {
    let record = record()?;
    let text = serde_json::to_string(&record)?;
    let read: ConversionRecord = serde_json::from_str(&text)?;
    assert_eq!(read, record);
    assert_eq!(read.structured.len(), 1);
    Ok(())
}

#[test]
fn a_whole_document_export_names_no_window() -> TestResult {
    let whole = WindowExport {
        window: None,
        digest: digest('d')?,
    };
    let value = serde_json::to_value(&whole)?;
    assert!(value.get("window").is_none(), "{value}");
    let read: WindowExport = serde_json::from_value(value)?;
    assert_eq!(read, whole);
    Ok(())
}

#[test]
fn a_stored_record_with_an_unknown_field_is_refused() -> TestResult {
    let mut value = serde_json::to_value(record()?)?;
    let object = value
        .as_object_mut()
        .ok_or("a conversion record serializes as an object")?;
    object.insert("selection".to_owned(), serde_json::json!({ "kind": "all" }));
    let refused = serde_json::from_value::<ConversionRecord>(value);
    assert!(refused.is_err(), "{refused:?}");
    Ok(())
}

#[test]
fn a_conversion_record_keeps_its_outline_and_requires_it() -> TestResult {
    let record = record()?;
    let mut value = serde_json::to_value(&record)?;
    let kept = value
        .get("outline")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len);
    assert_eq!(kept, Some(1));
    // A record written without the outline would leave `section_lines` with no input.
    value
        .as_object_mut()
        .ok_or("a conversion record serializes as an object")?
        .remove("outline");
    let refused = serde_json::from_value::<ConversionRecord>(value);
    assert!(refused.is_err(), "{refused:?}");
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
