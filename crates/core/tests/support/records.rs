//! Fixture conversion records and citations shared by the reading and views tests.

use std::collections::BTreeMap;

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::extraction::{
    ConversionOutcome, ConversionSettings, ConverterIdentity, Extraction, TextOrigin,
};
use okf_jawn_contract::identity::{Digest, ItemId, Revision, WorkspaceId, WorkspacePath};
use okf_jawn_contract::read::{AssetRole, OutlineEntry, OutlineEntryKind, Selection};
use okf_jawn_contract::source::{SourceAppearance, SourceLocation, SourceLocator, SourceReference};
use okf_jawn_core::conversion::{
    ConversionRecord, ConvertedCell, ConvertedTable, LineLocation, RetainedAsset, WindowExport,
};
use uuid::Uuid;

/// Result of a fixture builder.
pub type Built<T> = Result<T, Box<dyn std::error::Error>>;

/// The fixture source item.
pub const ITEM: u128 = 10;

/// A digest of 64 copies of `fill`.
///
/// # Errors
/// Returns when `fill` is not a lowercase hex digit.
pub fn digest(fill: char) -> Built<Digest> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

/// A revision of 40 copies of `fill`.
///
/// # Errors
/// Returns when `fill` is not a lowercase hex digit.
pub fn revision(fill: char) -> Built<Revision> {
    Ok(Revision::try_from(fill.to_string().repeat(40))?)
}

fn page(page_no: u32) -> SourceLocation {
    SourceLocation::Direct {
        locator: SourceLocator::Page { page_no },
    }
}

fn cell(row: u32, column: u32, text: &str, header: bool) -> ConvertedCell {
    ConvertedCell {
        row,
        column,
        row_span: 1,
        column_span: 1,
        text: text.to_owned(),
        column_header: header,
        row_header: false,
    }
}

/// A completed extraction whose conversion record is `digest('b')`.
///
/// # Errors
/// Returns when a fixture digest does not parse.
pub fn converted() -> Built<Extraction> {
    Ok(Extraction {
        outcome: ConversionOutcome::Completed,
        converter: None,
        digest: Some(digest('b')?),
        page_count: Some(2),
        text_origin: TextOrigin::Converter,
        corrected: false,
        supplied: None,
        warnings: Vec::new(),
    })
}

/// A source appearance: original `digest('a')`, record `digest('b')`.
///
/// # Errors
/// Returns when a fixture digest does not parse.
pub fn appearance() -> Built<SourceAppearance> {
    Ok(SourceAppearance {
        object: digest('a')?,
        names: Vec::new(),
        media_type: "application/pdf".to_owned(),
        size: "2048".to_owned(),
        metadata: BTreeMap::new(),
        parent_item_id: None,
        supersedes: None,
        extraction: converted()?,
    })
}

/// A two-page record: lines 1-3 on page 1, a revenue table on lines 5-8 of page 2, an export
/// `digest('c')` and a page image `digest('d')`.
///
/// # Errors
/// Returns when a fixture digest does not parse.
pub fn record() -> Built<ConversionRecord> {
    Ok(ConversionRecord {
        converter: ConverterIdentity {
            name: "docling".to_owned(),
            version: "2.3.0".to_owned(),
            packages: Vec::new(),
            settings: ConversionSettings::default(),
            models: Vec::new(),
            page_window: Some(4),
        },
        outcome: ConversionOutcome::Completed,
        page_count: Some(2),
        structured: vec![WindowExport {
            window: Some(PageRange { start: 1, end: 2 }),
            digest: digest('c')?,
        }],
        markdown: digest('e')?,
        locations: vec![
            LineLocation {
                lines: TextRange { start: 1, end: 3 },
                location: page(1),
            },
            LineLocation {
                lines: TextRange { start: 5, end: 8 },
                location: page(2),
            },
        ],
        outline: vec![
            OutlineEntry {
                label: "Overview".to_owned(),
                level: 1,
                selection: Selection::Lines {
                    range: TextRange { start: 1, end: 4 },
                },
                kind: OutlineEntryKind::Heading,
            },
            OutlineEntry {
                label: "Revenue".to_owned(),
                level: 1,
                selection: Selection::Lines {
                    range: TextRange { start: 5, end: 8 },
                },
                kind: OutlineEntryKind::Heading,
            },
        ],
        tables: vec![ConvertedTable {
            lines: TextRange { start: 5, end: 8 },
            location: page(2),
            num_rows: 3,
            num_cols: 3,
            cells: vec![
                cell(0, 0, "Quarter", true),
                cell(0, 1, "Revenue", true),
                cell(0, 2, "Audited", true),
                cell(1, 0, "Q1", false),
                cell(1, 1, "1200.5", false),
                cell(1, 2, "true", false),
                cell(2, 0, "Q2", false),
                cell(2, 1, "", false),
                cell(2, 2, "false", false),
            ],
        }],
        assets: vec![RetainedAsset {
            digest: digest('d')?,
            media_type: "image/png".to_owned(),
            role: AssetRole::PageImage,
            location: page(2),
            pixel_size: None,
            caption: None,
        }],
        warnings: Vec::new(),
    })
}

/// A citation of the fixture item at `at`, digest `digest('b')`, selecting `selection`.
///
/// # Errors
/// Returns when a fixture value does not parse.
pub fn citation(at: Revision, selection: Selection) -> Built<SourceReference> {
    Ok(SourceReference {
        workspace_id: WorkspaceId(Uuid::from_u128(1)),
        item_id: ItemId(Uuid::from_u128(ITEM)),
        path: WorkspacePath::try_from("sources/report-pdf.md".to_owned())?,
        revision: at,
        digest: Some(digest('b')?),
        selection,
        locations: Vec::new(),
    })
}
