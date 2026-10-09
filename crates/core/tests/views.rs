//! A binding's dataset is read from the table its selection covers, and once recorded it is an
//! object of the bound revision only.

use std::collections::BTreeMap;

use okf_jawn_contract::common::{PageRange, TextRange};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::identity::ItemId;
use okf_jawn_contract::read::Selection;
use okf_jawn_contract::views::{ChartFailure, ColumnKind, DatasetValue, ViewBinding};
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

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/records.rs"]
mod records;
