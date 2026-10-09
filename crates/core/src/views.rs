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
use std::ops::Range;

use sha2::{Digest as _, Sha256};

use okf_jawn_contract::{
    common::CellRange,
    error::{ApiError, ErrorCode},
    extraction::{ExtractionWarning, TextOrigin},
    identity::Digest,
    read::Selection,
    source::{SourceLocation, SourceLocator, SourceReference},
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

/// A number a dataset keeps exactly.
#[derive(Debug, Clone, Copy)]
enum ExactNumber {
    /// An integer within ±2^53.
    Integer(i64),
    /// A decimal of at most 15 significant digits.
    Decimal(f64),
}

/// Media type a dataset is retained and served with.
pub const DATASET_MEDIA_TYPE: &str = "application/json";
/// Format version of the datasets this module produces.
pub const DATASET_SCHEMA_VERSION: u32 = 1;
/// Most rows a dataset may hold (the contract schema's bound).
pub const MAX_DATASET_ROWS: usize = 100_000;
/// Most columns a dataset may hold (the contract schema's bound).
pub const MAX_DATASET_COLUMNS: usize = 1024;
/// Most cells (rows times columns) a dataset may hold. The row and column bounds alone allow
/// about 102 million; the grid is laid out in memory before values are read, so the product is
/// bounded too.
pub const MAX_DATASET_CELLS: usize = 1_000_000;
/// Largest integer magnitude a JSON reader holds exactly (2^53).
const MAX_EXACT_INTEGER: u64 = 1 << 53;
/// Most significant digits of a decimal a double holds exactly as written.
const MAX_EXACT_DECIMAL_DIGITS: usize = 15;

/// Build the dataset of `binding` from the conversion record of the digest it cites.
///
/// `text_origin` is whose text the record describes; `warnings` are the source's extraction
/// warnings, of which those that bear on cell values are kept.
///
/// # Errors
/// Returns `DatasetUnavailable` when the selection covers no table or several, or a kind the
/// record cannot answer; `TooLarge` past the row, column or cell bound.
pub fn materialize(
    binding: &ViewBinding,
    record: &ConversionRecord,
    text_origin: TextOrigin,
    warnings: &[ExtractionWarning],
) -> Result<Dataset, BindingFailure> {
    let (table, cells) = covered_table(record, &binding.source.selection)?;
    let (rows, columns) = extent(table, cells.as_ref());
    // Bound the layout before allocating it: a record's counts are not trusted to be small.
    let (height, width) = (rows.len(), columns.len());
    if width > MAX_DATASET_COLUMNS
        || height > MAX_DATASET_ROWS
        || height.saturating_mul(width) > MAX_DATASET_CELLS
    {
        return Err(BindingFailure {
            reason: ChartFailure::TooLarge,
            message: format!(
                "the selected table has {height} rows and {width} columns; a dataset holds at \
                 most {MAX_DATASET_ROWS} rows, {MAX_DATASET_COLUMNS} columns and \
                 {MAX_DATASET_CELLS} cells"
            ),
        });
    }
    if width == 0 {
        return Err(unavailable("the selected table has no columns"));
    }
    let grid = grid_of(table, rows, columns)?;
    let header_rows = grid
        .iter()
        .take_while(|row| row.iter().any(|cell| cell.is_some_and(|cell| cell.header)))
        .count();
    let (headers, body) = grid.split_at(header_rows);
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
        // Locations are derived and filled on return; leaving them out keeps the bytes, and so
        // the digest, the same whether or not they were filled first.
        source: SourceReference {
            locations: Vec::new(),
            ..binding.source.clone()
        },
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

/// The rows and columns of `table` to read: all of it, or the selected window.
fn extent(table: &ConvertedTable, window: Option<&CellWindow>) -> (Range<u32>, Range<u32>) {
    window.map_or((0..table.num_rows, 0..table.num_cols), |window| {
        (
            window.first_row..window.last_row.saturating_add(1),
            window.first_column..window.last_column.saturating_add(1),
        )
    })
}

/// The table laid out row by row over `rows` and `columns`; a spanning cell fills its first
/// position, the rest of its span stays empty.
///
/// A cell outside the table's own grid, or two cells at one position, is a fault of the record:
/// the chart fails rather than reading a table with a cell silently dropped.
fn grid_of<'a>(
    table: &'a ConvertedTable,
    rows: Range<u32>,
    columns: Range<u32>,
) -> Result<Vec<Vec<Option<GridCell<'a>>>>, BindingFailure> {
    let mut by_position: BTreeMap<(u32, u32), GridCell<'a>> = BTreeMap::new();
    for cell in &table.cells {
        if cell.row >= table.num_rows || cell.column >= table.num_cols {
            return Err(unavailable(&format!(
                "the conversion record places a cell at row {}, column {} of a {} by {} table",
                cell.row, cell.column, table.num_rows, table.num_cols
            )));
        }
        let placed = GridCell {
            text: cell.text.as_str(),
            header: cell.column_header,
        };
        if by_position
            .insert((cell.row, cell.column), placed)
            .is_some()
        {
            return Err(unavailable(&format!(
                "the conversion record places two cells at row {}, column {}",
                cell.row, cell.column
            )));
        }
    }
    Ok(rows
        .map(|row| {
            columns
                .clone()
                .map(|column| by_position.get(&(row, column)).copied())
                .collect()
        })
        .collect())
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
    } else if values
        .iter()
        .all(|text| matches!(exact_number(text), Some(ExactNumber::Integer(_))))
    {
        ColumnKind::Integer
    } else if values.iter().all(|text| exact_number(text).is_some()) {
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
        ColumnKind::Integer | ColumnKind::Number => {
            exact_number(text).and_then(|number| match number {
                ExactNumber::Integer(value) => Some(DatasetValue::Number(value.into())),
                ExactNumber::Decimal(value) => {
                    serde_json::Number::from_f64(value).map(DatasetValue::Number)
                }
            })
        }
        ColumnKind::Boolean => boolean(text).map(DatasetValue::Boolean),
        ColumnKind::String | ColumnKind::DateTime => None,
    };
    typed.unwrap_or_else(|| DatasetValue::Text(text.to_owned()))
}

/// The value of `text` when it is a number that a dataset keeps exactly: the JSON number
/// grammar without an exponent (no leading `+`, no leading zero before other digits, so an
/// identifier such as `02134` stays text), an integer within ±2^53, or a decimal of at most 15
/// significant digits and 15 digits after the point, which a double holds exactly as written
/// and which cannot underflow to zero. Anything else is text.
fn exact_number(text: &str) -> Option<ExactNumber> {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || (whole.len() > 1 && whole.starts_with('0')) {
        return None;
    }
    match fraction {
        None => text
            .parse::<i64>()
            .ok()
            .filter(|value| value.unsigned_abs() <= MAX_EXACT_INTEGER)
            .map(ExactNumber::Integer),
        Some(fraction) if digits(fraction) => {
            // At most 15 significant digits, and at most 15 after the point: the value is then
            // at least 1e-15 when nonzero, far from where a double underflows to zero.
            let joined = format!("{whole}{fraction}");
            if joined.trim_start_matches('0').len() > MAX_EXACT_DECIMAL_DIGITS
                || fraction.len() > MAX_EXACT_DECIMAL_DIGITS
            {
                return None;
            }
            text.parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(ExactNumber::Decimal)
        }
        Some(_) => None,
    }
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
