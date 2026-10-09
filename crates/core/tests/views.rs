//! A binding's dataset is read from the table its selection covers, and once recorded it is an
//! object of the bound revision only.

use std::collections::BTreeMap;

use okf_jawn_contract::common::{CellRange, PageRange, TextRange};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::identity::ItemId;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::source::{SourceLocation, SourceLocator};
use okf_jawn_contract::views::{ChartFailure, ColumnKind, Dataset, DatasetValue, ViewBinding};
use okf_jawn_core::conversion::{ConversionRecord, ConvertedCell};
use okf_jawn_core::jobs::DerivedKind;
use okf_jawn_core::reading::{ObjectRole, RevisionObjects, object_role};
use okf_jawn_core::views::{dataset_bytes, dataset_object, materialize};
use uuid::Uuid;

use check::{TestResult, err_of};
use records::{Built, ITEM, appearance, citation, record, revision};

fn binding(selection: Selection) -> Built<ViewBinding> {
    Ok(ViewBinding {
        name: "revenue".to_owned(),
        source: citation(revision('1')?, selection)?,
        units: BTreeMap::from([("Revenue".to_owned(), "EUR".to_owned())]),
        transforms: Vec::new(),
        materialized: None,
    })
}

#[test]
fn a_materialized_dataset_is_an_object_of_the_bound_revision() -> TestResult {
    let record = record()?;
    let source = appearance()?;
    let bound = binding(Selection::Lines {
        range: TextRange { start: 4, end: 9 },
    })?;
    let dataset = materialize(&bound, &record, TextOrigin::Converter, &[])
        .map_err(|failure| failure.message)?;
    let (bytes, digest) = dataset_bytes(&dataset)?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes)?.get("schema_version"),
        Some(&serde_json::json!(1))
    );
    let derived = dataset_object(&bound, digest.clone());
    assert_eq!(derived.kind, DerivedKind::Dataset);
    assert_eq!(derived.revision, revision('1')?);

    // With the binding's citation, the dataset is served as that revision's derived object.
    let item_id = ItemId(Uuid::from_u128(ITEM));
    let bound_revision = revision('1')?;
    let at_bound = RevisionObjects {
        item_id,
        revision: &bound_revision,
        source: Some(&source),
        record: Some(&record),
        derived: Some(&derived),
    };
    assert_eq!(
        object_role(&at_bound, &digest)?,
        ObjectRole::Derived(DerivedKind::Dataset)
    );

    // With the citation of another revision of the same item, it is not found, even if a
    // record for the dataset were handed over.
    let other_revision = revision('2')?;
    let at_other = RevisionObjects {
        revision: &other_revision,
        ..at_bound
    };
    let refused = err_of(object_role(&at_other, &digest))?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    let unrecorded = RevisionObjects {
        derived: None,
        ..at_other
    };
    assert_eq!(
        err_of(object_role(&unrecorded, &digest))?.code,
        ErrorCode::NotFound
    );
    Ok(())
}

#[test]
fn a_dataset_types_its_columns_from_every_value() -> TestResult {
    let record = record()?;
    let dataset = materialize(
        &binding(Selection::Pages {
            range: PageRange { start: 2, end: 2 },
        })?,
        &record,
        TextOrigin::Converter,
        &[],
    )
    .map_err(|failure| failure.message)?;
    let names: Vec<&str> = dataset
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(names, ["Quarter", "Revenue", "Audited"]);
    let kinds: Vec<ColumnKind> = dataset.columns.iter().map(|column| column.kind).collect();
    assert_eq!(
        kinds,
        [ColumnKind::String, ColumnKind::Number, ColumnKind::Boolean]
    );
    let unit = dataset
        .columns
        .get(1)
        .and_then(|column| column.unit.as_deref());
    assert_eq!(unit, Some("EUR"));
    assert_eq!(dataset.rows.len(), 2);
    let second = dataset.rows.get(1).ok_or("a second row")?;
    assert_eq!(second.get(1), Some(&DatasetValue::Null));
    assert_eq!(second.get(2), Some(&DatasetValue::Boolean(false)));
    // The same binding read twice is the same bytes and digest, so a repeated present records
    // nothing new.
    let again = materialize(
        &binding(Selection::Pages {
            range: PageRange { start: 2, end: 2 },
        })?,
        &record,
        TextOrigin::Converter,
        &[],
    )
    .map_err(|failure| failure.message)?;
    assert_eq!(dataset_bytes(&dataset)?, dataset_bytes(&again)?);
    // The whole source holds one table, so it reads that table.
    let whole = materialize(
        &binding(Selection::All)?,
        &record,
        TextOrigin::Converter,
        &[],
    )
    .map_err(|failure| failure.message)?;
    assert_eq!(whole.rows, dataset.rows);
    Ok(())
}

#[test]
fn a_selection_covering_no_table_fails_only_its_chart() -> TestResult {
    let record = record()?;
    let failure = err_of(materialize(
        &binding(Selection::Lines {
            range: TextRange { start: 1, end: 3 },
        })?,
        &record,
        TextOrigin::Converter,
        &[],
    ))?;
    assert_eq!(failure.reason, ChartFailure::DatasetUnavailable);
    let section = err_of(materialize(
        &binding(Selection::Section {
            heading: "Revenue".to_owned(),
        })?,
        &record,
        TextOrigin::Converter,
        &[],
    ))?;
    assert_eq!(section.reason, ChartFailure::DatasetUnavailable);
    Ok(())
}

/// The fixture record with its table replaced by one header cell and one column of `values`.
fn one_column(values: &[&str]) -> Built<ConversionRecord> {
    let mut record = record()?;
    let table = record.tables.first_mut().ok_or("the fixture has a table")?;
    let height = u32::try_from(values.len())?;
    table.num_rows = height.saturating_add(1);
    table.num_cols = 1;
    table.cells = std::iter::once(("Code", true))
        .chain(values.iter().map(|value| (*value, false)))
        .zip(0_u32..)
        .map(|((text, header), row)| ConvertedCell {
            row,
            column: 0,
            row_span: 1,
            column_span: 1,
            text: text.to_owned(),
            column_header: header,
            row_header: false,
        })
        .collect();
    Ok(record)
}

fn read_one_column(values: &[&str]) -> Built<Dataset> {
    Ok(materialize(
        &binding(Selection::All)?,
        &one_column(values)?,
        TextOrigin::Converter,
        &[],
    )
    .map_err(|failure| failure.message)?)
}

#[test]
fn identifiers_and_long_numbers_stay_text_exactly() -> TestResult {
    for values in [
        ["02134", "10115"],
        ["+5", "6"],
        ["12345678901234567890123", "1"],
        ["0.10000000000000000001", "0.5"],
        ["1e5", "2"],
    ] {
        let dataset = read_one_column(&values)?;
        let kind = dataset.columns.first().map(|column| column.kind);
        assert_eq!(kind, Some(ColumnKind::String), "{values:?}");
        let first = dataset.rows.first().and_then(|row| row.first());
        let written = values
            .first()
            .map(|text| DatasetValue::Text((*text).to_owned()));
        assert_eq!(first, written.as_ref(), "{values:?}");
    }
    // A nonzero decimal below a double's range would parse as 0.0; it stays text exactly.
    let tiny = format!("0.{}1", "0".repeat(400));
    let dataset = read_one_column(&[tiny.as_str(), "0.5"])?;
    let kind = dataset.columns.first().map(|column| column.kind);
    assert_eq!(kind, Some(ColumnKind::String));
    let first = dataset.rows.first().and_then(|row| row.first());
    assert_eq!(first, Some(&DatasetValue::Text(tiny.clone())));
    let integers = read_one_column(&["-12", "9007199254740992", "0"])?;
    let kind = integers.columns.first().map(|column| column.kind);
    assert_eq!(kind, Some(ColumnKind::Integer));
    let decimals = read_one_column(&["-0.25", "3", "1200.5"])?;
    let kind = decimals.columns.first().map(|column| column.kind);
    assert_eq!(kind, Some(ColumnKind::Number));
    assert_eq!(
        serde_json::to_value(&decimals.rows)?,
        serde_json::json!([[-0.25], [3], [1200.5]])
    );
    Ok(())
}

#[test]
fn a_table_within_the_row_and_column_bounds_but_too_many_cells_is_too_large() -> TestResult {
    // 100,000 rows by 1,024 columns passes each bound alone; laid out it would be about 102
    // million cells, so it is refused before any grid is allocated.
    let mut record = one_column(&["1"])?;
    let table = record.tables.first_mut().ok_or("the fixture has a table")?;
    table.num_rows = 100_000;
    table.num_cols = 1024;
    let failure = err_of(materialize(
        &binding(Selection::All)?,
        &record,
        TextOrigin::Converter,
        &[],
    ))?;
    assert_eq!(failure.reason, ChartFailure::TooLarge);
    Ok(())
}

#[test]
fn a_cell_outside_its_table_fails_the_chart() -> TestResult {
    let mut record = one_column(&["1"])?;
    let table = record.tables.first_mut().ok_or("the fixture has a table")?;
    table.num_rows = 1;
    let failure = err_of(materialize(
        &binding(Selection::All)?,
        &record,
        TextOrigin::Converter,
        &[],
    ))?;
    assert_eq!(failure.reason, ChartFailure::DatasetUnavailable);
    Ok(())
}

/// Read the fixture record after `change` edits its table's cells.
fn read_with(change: impl FnOnce(&mut Vec<ConvertedCell>)) -> Built<Result<Dataset, ChartFailure>> {
    let mut record = record()?;
    let table = record.tables.first_mut().ok_or("the fixture has a table")?;
    change(&mut table.cells);
    Ok(materialize(
        &binding(Selection::All)?,
        &record,
        TextOrigin::Converter,
        &[],
    )
    .map_err(|failure| failure.reason))
}

/// The fixture cell at (`row`, `column`).
fn cell_at(cells: &mut [ConvertedCell], row: u32, column: u32) -> Option<&mut ConvertedCell> {
    cells
        .iter_mut()
        .find(|cell| cell.row == row && cell.column == column)
}

#[test]
fn an_empty_out_of_table_or_overlapping_span_fails_the_chart() -> TestResult {
    // An empty span.
    let empty = read_with(|cells| {
        if let Some(cell) = cell_at(cells, 1, 0) {
            cell.row_span = 0;
        }
    })?;
    assert_eq!(err_of(empty)?, ChartFailure::DatasetUnavailable);
    // A span past the last column.
    let past = read_with(|cells| {
        if let Some(cell) = cell_at(cells, 0, 2) {
            cell.column_span = 2;
        }
    })?;
    assert_eq!(err_of(past)?, ChartFailure::DatasetUnavailable);
    // A span over the next cell of its row.
    let overlap = read_with(|cells| {
        if let Some(cell) = cell_at(cells, 0, 0) {
            cell.column_span = 2;
        }
    })?;
    assert_eq!(err_of(overlap)?, ChartFailure::DatasetUnavailable);
    // A valid span: Q1's revenue covers two rows, and the position it covers reads as empty.
    let spanned = read_with(|cells| {
        cells.retain(|cell| !(cell.row == 2 && cell.column == 1));
        if let Some(cell) = cell_at(cells, 1, 1) {
            cell.row_span = 2;
        }
    })?
    .map_err(|reason| format!("{reason:?}"))?;
    let second = spanned.rows.get(1).ok_or("a second row")?;
    assert_eq!(second.get(1), Some(&DatasetValue::Null));
    Ok(())
}

#[test]
fn a_header_spanning_two_rows_leaves_no_empty_body_row() -> TestResult {
    let dataset = read_with(|cells| {
        cells.retain(|cell| cell.row != 1);
        for cell in cells.iter_mut().filter(|cell| cell.row == 0) {
            cell.row_span = 2;
        }
    })?
    .map_err(|reason| format!("{reason:?}"))?;
    let names: Vec<&str> = dataset
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(names, ["Quarter", "Revenue", "Audited"]);
    // Only the Q2 row is data; the second header row is not an invented row of nulls.
    assert_eq!(dataset.rows.len(), 1);
    let first = dataset.rows.first().ok_or("one body row")?;
    assert_eq!(first.first(), Some(&DatasetValue::Text("Q2".to_owned())));
    Ok(())
}

#[test]
fn every_column_name_is_distinct_even_against_generated_ones() -> TestResult {
    let dataset = read_with(|cells| {
        for (column, header) in [(0, "A"), (1, "A"), (2, "A (2)")] {
            if let Some(cell) = cell_at(cells, 0, column) {
                header.clone_into(&mut cell.text);
            }
        }
    })?
    .map_err(|reason| format!("{reason:?}"))?;
    let names: Vec<&str> = dataset
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    assert_eq!(names, ["A", "A (2)", "A (2) (3)"]);
    Ok(())
}

#[test]
fn a_reversed_cell_selection_reads_no_table() -> TestResult {
    let mut record = record()?;
    let table = record.tables.first_mut().ok_or("the fixture has a table")?;
    table.location = SourceLocation::Direct {
        locator: SourceLocator::Cells {
            range: CellRange {
                sheet: "Q1".to_owned(),
                row_start: 1,
                row_end: 3,
                column_start: 1,
                column_end: 3,
            },
        },
    };
    let select = |row_start, row_end| -> Built<ViewBinding> {
        binding(Selection::Cells {
            range: CellRange {
                sheet: "Q1".to_owned(),
                row_start,
                row_end,
                column_start: 1,
                column_end: 3,
            },
        })
    };
    let read = materialize(&select(1, 3)?, &record, TextOrigin::Converter, &[])
        .map_err(|failure| failure.message)?;
    assert_eq!(read.rows.len(), 2);
    let reversed = err_of(materialize(
        &select(3, 2)?,
        &record,
        TextOrigin::Converter,
        &[],
    ))?;
    assert_eq!(reversed.reason, ChartFailure::DatasetUnavailable);
    Ok(())
}

#[test]
fn a_cell_selection_past_the_table_grid_reads_no_table() -> TestResult {
    // The locator claims rows 1 to 100, but the table's grid has 3 rows: row 50 does not exist,
    // so selecting it must not read an invented empty row.
    let mut record = record()?;
    let table = record.tables.first_mut().ok_or("the fixture has a table")?;
    table.location = SourceLocation::Direct {
        locator: SourceLocator::Cells {
            range: CellRange {
                sheet: "Q1".to_owned(),
                row_start: 1,
                row_end: 100,
                column_start: 1,
                column_end: 3,
            },
        },
    };
    let past = binding(Selection::Cells {
        range: CellRange {
            sheet: "Q1".to_owned(),
            row_start: 1,
            row_end: 50,
            column_start: 1,
            column_end: 3,
        },
    })?;
    let failure = err_of(materialize(&past, &record, TextOrigin::Converter, &[]))?;
    assert_eq!(failure.reason, ChartFailure::DatasetUnavailable);
    Ok(())
}

#[test]
fn a_dataset_digest_does_not_depend_on_filled_locations() -> TestResult {
    let record = record()?;
    let bare = binding(Selection::All)?;
    let mut filled = bare.clone();
    filled.source.locations = vec![SourceLocation::Direct {
        locator: SourceLocator::Page { page_no: 2 },
    }];
    let read = |binding: &ViewBinding| {
        materialize(binding, &record, TextOrigin::Converter, &[]).map_err(|failure| failure.message)
    };
    assert_eq!(
        dataset_bytes(&read(&bare)?)?,
        dataset_bytes(&read(&filled)?)?
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/records.rs"]
mod records;
