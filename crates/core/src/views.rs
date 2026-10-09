//! The producer of a View binding's materialized dataset (Stage 1b design sections 4 and 9.5).
//!
//! A binding cites one item revision and digest with a selection. Its dataset is read from the
//! one `ConvertedTable` of that digest's `ConversionRecord` the selection covers: a line range
//! holding the table's lines, a page range holding its page, the whole source when it has
//! exactly one table, or a cell range inside a spreadsheet table. The table becomes a typed
//! `Dataset` (header rows name the columns; a column is `integer`, `number` or `boolean` only
//! when every value in it is; nothing is guessed from locale, and a date stays text). Its
//! canonical JSON bytes are retained and recorded against the binding's item and revision with
//! `RecordStore::record_derived_object`, which makes the dataset an object `get_object` serves
//! for that revision and no other.
//!
//! A binding that cannot be materialized is a chart-level fault (`BindingFailure`), never a
//! refusal of the View.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};

use okf_jawn_contract::{
    common::CellRange,
    error::{ApiError, ErrorCode},
    extraction::{ExtractionWarning, TextOrigin},
    identity::Digest,
    read::Selection,
    source::{SourceLocation, SourceLocator},
    views::{ChartFailure, ColumnKind, Dataset, DatasetColumn, DatasetValue, ViewBinding},
};

use crate::conversion::{ConversionRecord, ConvertedTable};
use crate::jobs::{DerivedKind, DerivedObject, RecordStore};
use crate::reading::location_page;
use crate::storage::{BlobStore, StorageScope};

/// Why a binding has no dataset; the chart that reads it fails alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingFailure {
    /// Typed cause, as the chart reports it.
    pub reason: ChartFailure,
    /// Safe explanation for the chart's own alert.
    pub message: String,
}

/// One cell of the grid a table is laid out in.
#[derive(Debug, Clone, Copy)]
struct GridCell<'a> {
    text: &'a str,
    header: bool,
}

/// Zero-based rows and columns of a table that a cell range selects.
#[derive(Debug, Clone, Copy)]
struct CellWindow {
    first_row: u32,
    last_row: u32,
    first_column: u32,
    last_column: u32,
}

/// Media type a dataset is retained and served with.
pub const DATASET_MEDIA_TYPE: &str = "application/json";
/// Format version of the datasets this module produces.
pub const DATASET_SCHEMA_VERSION: u32 = 1;
/// Most rows a dataset may hold (the contract schema's bound).
pub const MAX_DATASET_ROWS: usize = 100_000;
/// Most columns a dataset may hold (the contract schema's bound).
pub const MAX_DATASET_COLUMNS: usize = 1024;

/// Build the dataset of `binding` from the conversion record of the digest it cites.
///
/// `text_origin` is whose text the record describes; `warnings` are the source's extraction
/// warnings, of which those that bear on cell values are kept.
///
/// # Errors
/// Returns `DatasetUnavailable` when the selection covers no table or several, or a kind the
/// record cannot answer; `TooLarge` past the row or column bound.
pub fn materialize(
    binding: &ViewBinding,
    record: &ConversionRecord,
    text_origin: TextOrigin,
    warnings: &[ExtractionWarning],
) -> Result<Dataset, BindingFailure> {
    let (table, cells) = covered_table(record, &binding.source.selection)?;
    let grid = grid_of(table, cells.as_ref());
    let header_rows = grid
        .iter()
        .take_while(|row| row.iter().any(|cell| cell.is_some_and(|cell| cell.header)))
        .count();
    let (headers, body) = grid.split_at(header_rows);
    let width = grid.first().map_or(0, Vec::len);
    if width == 0 {
        return Err(unavailable("the selected table has no columns"));
    }
    if width > MAX_DATASET_COLUMNS || body.len() > MAX_DATASET_ROWS {
        return Err(BindingFailure {
            reason: ChartFailure::TooLarge,
            message: format!(
                "the selected table has {} rows and {width} columns; a dataset holds at most \
                 {MAX_DATASET_ROWS} rows and {MAX_DATASET_COLUMNS} columns",
                body.len()
            ),
        });
    }
    let names = column_names(headers, width);
    let kinds: Vec<ColumnKind> = (0..width).map(|column| column_kind(body, column)).collect();
    let columns = names
        .into_iter()
        .zip(&kinds)
        .map(|(name, kind)| DatasetColumn {
            unit: binding.units.get(&name).cloned(),
            name,
            kind: *kind,
        })
        .collect();
    let rows = body
        .iter()
        .map(|row| {
            row.iter()
                .zip(&kinds)
                .map(|(cell, kind)| value_of(cell.map(|cell| cell.text), *kind))
                .collect()
        })
        .collect();
    let dataset = Dataset {
        schema_version: DATASET_SCHEMA_VERSION,
        source: binding.source.clone(),
        text_origin,
        columns,
        rows,
        warnings: warnings
            .iter()
            .filter(|warning| matches!(warning, ExtractionWarning::CellValuesOnly { .. }))
            .cloned()
            .collect(),
    };
    dataset
        .check()
        .map_err(|error| unavailable(&format!("the dataset is not well formed: {error}")))?;
    Ok(dataset)
}

/// The canonical JSON bytes of a dataset and their digest.
///
/// # Errors
/// Returns `Internal` if the dataset cannot be serialized.
pub fn dataset_bytes(dataset: &Dataset) -> Result<(Vec<u8>, Digest), ApiError> {
    let bytes = serde_json::to_vec(dataset).map_err(|error| {
        ApiError::new(
            ErrorCode::Internal,
            format!("a dataset did not serialize: {error}"),
        )
    })?;
    let hash = Sha256::digest(&bytes);
    let digest = Digest::try_from(format!("{hash:x}"))
        .map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?;
    Ok((bytes, digest))
}

/// The record that makes `digest` a dataset object of the binding's cited item revision.
#[must_use]
pub fn dataset_object(binding: &ViewBinding, digest: Digest) -> DerivedObject {
    DerivedObject {
        item_id: binding.source.item_id,
        revision: binding.source.revision.clone(),
        digest,
        kind: DerivedKind::Dataset,
        media_type: DATASET_MEDIA_TYPE.to_owned(),
    }
}

/// Retain a dataset's bytes and record them against the binding's item revision.
///
/// Both writes are idempotent (the blob store keeps one copy of identical bytes; the derived
/// record is unique on item, revision and digest), so a repeated `present_view` or
/// `resolve_view` records nothing new. Recording during a read is not a change of user state,
/// as an index entry is not.
///
/// # Errors
/// Returns any error of the two ports.
pub async fn retain_dataset(
    blobs: &dyn BlobStore,
    records: &dyn RecordStore,
    scope: &StorageScope,
    binding: &ViewBinding,
    dataset: &Dataset,
) -> Result<DerivedObject, ApiError> {
    let (bytes, digest) = dataset_bytes(dataset)?;
    let limit = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    blobs
        .put(
            scope,
            Box::pin(std::io::Cursor::new(bytes)),
            limit,
            Some(digest.clone()),
        )
        .await?;
    records
        .record_derived_object(scope, dataset_object(binding, digest))
        .await
}

/// The one table the selection covers and, for a cell range, the window of it to read.
fn covered_table<'a>(
    record: &'a ConversionRecord,
    selection: &Selection,
) -> Result<(&'a ConvertedTable, Option<CellWindow>), BindingFailure> {
    let covered: Vec<(&ConvertedTable, Option<CellWindow>)> = match selection {
        Selection::All => record.tables.iter().map(|table| (table, None)).collect(),
        Selection::Lines { range } => record
            .tables
            .iter()
            .filter(|table| table.lines.start >= range.start && table.lines.end <= range.end)
            .map(|table| (table, None))
            .collect(),
        Selection::Pages { range } => record
            .tables
            .iter()
            .filter(|table| {
                location_page(&table.location)
                    .is_some_and(|page| page >= range.start && page <= range.end)
            })
            .map(|table| (table, None))
            .collect(),
        Selection::Cells { range } => record
            .tables
            .iter()
            .filter_map(|table| cell_window(table, range).map(|window| (table, Some(window))))
            .collect(),
        Selection::Section { .. } | Selection::Region { .. } => {
            return Err(unavailable(
                "a dataset is read from a line, page or cell selection, or a whole source",
            ));
        }
    };
    match covered.as_slice() {
        [one] => Ok(*one),
        [] => Err(unavailable(
            "the selection covers no table of the cited digest",
        )),
        several => Err(unavailable(&format!(
            "the selection covers {} tables; select one",
            several.len()
        ))),
    }
}

/// The window of `table` that `range` selects, when the table is located by cells on the same
/// sheet and holds the whole range.
fn cell_window(table: &ConvertedTable, range: &CellRange) -> Option<CellWindow> {
    let SourceLocation::Direct {
        locator: SourceLocator::Cells { range: located },
    } = &table.location
    else {
        return None;
    };
    let inside = located.sheet == range.sheet
        && range.row_start >= located.row_start
        && range.row_end <= located.row_end
        && range.column_start >= located.column_start
        && range.column_end <= located.column_end;
    if !inside {
        return None;
    }
    Some(CellWindow {
        first_row: range.row_start.checked_sub(located.row_start)?,
        last_row: range.row_end.checked_sub(located.row_start)?,
        first_column: range.column_start.checked_sub(located.column_start)?,
        last_column: range.column_end.checked_sub(located.column_start)?,
    })
}

/// The table laid out row by row; a spanning cell fills its first position, the rest of its
/// span stays empty. `window` keeps only the selected part.
fn grid_of<'a>(
    table: &'a ConvertedTable,
    window: Option<&CellWindow>,
) -> Vec<Vec<Option<GridCell<'a>>>> {
    let (rows, columns) = window.map_or((0..table.num_rows, 0..table.num_cols), |window| {
        (
            window.first_row..window.last_row.saturating_add(1),
            window.first_column..window.last_column.saturating_add(1),
        )
    });
    let mut by_position: BTreeMap<(u32, u32), GridCell<'a>> = BTreeMap::new();
    for cell in &table.cells {
        by_position.insert(
            (cell.row, cell.column),
            GridCell {
                text: cell.text.as_str(),
                header: cell.column_header,
            },
        );
    }
    rows.map(|row| {
        columns
            .clone()
            .map(|column| by_position.get(&(row, column)).copied())
            .collect()
    })
    .collect()
}

/// Column names from the header rows, joined top to bottom; `column_<n>` where there is none.
/// A repeated name gets its column number appended, so every name is distinct.
fn column_names(headers: &[Vec<Option<GridCell<'_>>>], width: usize) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    (0..width)
        .map(|column| {
            let joined = headers
                .iter()
                .filter_map(|row| row.get(column).copied().flatten())
                .map(|cell| cell.text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let number = column.saturating_add(1);
            let base = if joined.is_empty() {
                format!("column_{number}")
            } else {
                joined
            };
            let count = seen.entry(base.clone()).or_insert(0);
            *count = count.saturating_add(1);
            if *count == 1 {
                base
            } else {
                format!("{base} ({number})")
            }
        })
        .collect()
}

/// The narrowest kind every non-empty value of a column has.
fn column_kind(body: &[Vec<Option<GridCell<'_>>>], column: usize) -> ColumnKind {
    let values: Vec<&str> = body
        .iter()
        .filter_map(|row| row.get(column).copied().flatten())
        .map(|cell| cell.text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    if values.is_empty() {
        ColumnKind::String
    } else if values.iter().all(|text| text.parse::<i64>().is_ok()) {
        ColumnKind::Integer
    } else if values.iter().all(|text| finite_number(text).is_some()) {
        ColumnKind::Number
    } else if values.iter().all(|text| boolean(text).is_some()) {
        ColumnKind::Boolean
    } else {
        ColumnKind::String
    }
}

/// One value of a column of `kind`; an empty cell is null.
fn value_of(text: Option<&str>, kind: ColumnKind) -> DatasetValue {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return DatasetValue::Null;
    };
    let typed = match kind {
        ColumnKind::Integer => text
            .parse::<i64>()
            .ok()
            .map(|value| DatasetValue::Number(value.into())),
        ColumnKind::Number => finite_number(text).map(DatasetValue::Number),
        ColumnKind::Boolean => boolean(text).map(DatasetValue::Boolean),
        ColumnKind::String | ColumnKind::DateTime => None,
    };
    typed.unwrap_or_else(|| DatasetValue::Text(text.to_owned()))
}

fn finite_number(text: &str) -> Option<serde_json::Number> {
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .and_then(serde_json::Number::from_f64)
}

fn boolean(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn unavailable(message: &str) -> BindingFailure {
    BindingFailure {
        reason: ChartFailure::DatasetUnavailable,
        message: message.to_owned(),
    }
}
