//! A binding's dataset is read from the table its selection covers, and once recorded it is an
//! object of the bound revision only.

use std::collections::BTreeMap;

use okf_jawn_contract::common::{PageRange, TextRange};
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
