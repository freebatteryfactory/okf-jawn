//! A citation opens only the objects of its item revision, reads at an invalidated revision are
//! typed, and citation locations are the server's.

use okf_jawn_contract::common::{CellRange, PageRange, TextRange};
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::identity::{ArtifactId, ItemId, JobId, PurgeId, TenantId, WorkspaceId};
use okf_jawn_contract::read::{AssetRole, Selection};
use okf_jawn_contract::source::{SourceLocation, SourceLocator, UnresolvedReason};
use okf_jawn_core::conversion::ConversionRecord;
use okf_jawn_core::jobs::{ArtifactKind, ArtifactRecord, JobScope, RevisionMapping};
use okf_jawn_core::reading::{
    ObjectRole, RevisionObjects, cited_locations, decode_conversion_record, fill_locations,
    invalidated, object_role, section_lines,
};
use okf_jawn_core::storage::{ObjectInfo, StorageScope};
use serde_json::json;
use uuid::Uuid;

use check::{TestResult, err_of};
use records::{Built, ITEM, appearance, citation, converted, digest, record, revision};

fn workspace_archive() -> Built<ArtifactRecord> {
    Ok(ArtifactRecord {
        id: ArtifactId(Uuid::from_u128(12)),
        scope: JobScope::Workspace(StorageScope {
            tenant_id: TenantId::try_from("local".to_owned())?,
            workspace_id: WorkspaceId(Uuid::from_u128(1)),
        }),
        kind: ArtifactKind::WorkspaceBackup,
        object: ObjectInfo {
            digest: digest('f')?,
            size: 4096,
        },
        media_type: "application/zip".to_owned(),
        created_by_job: JobId(Uuid::from_u128(7)),
    })
}

#[test]
fn get_object_refuses_a_backup_artifact_digest() -> TestResult {
    let source = appearance()?;
    let record = record()?;
    let at = revision('1')?;
    let cited = citation(at.clone(), Selection::All)?;
    let objects = RevisionObjects {
        item_id: cited.item_id,
        revision: &cited.revision,
        source: Some(&source),
        record: Some(&record),
        derived: None,
    };
    // Every object of the cited revision is served, each in its role.
    assert_eq!(object_role(&objects, &digest('a')?)?, ObjectRole::Original);
    assert_eq!(
        object_role(&objects, &digest('b')?)?,
        ObjectRole::ConversionRecord
    );
    assert_eq!(
        object_role(&objects, &digest('c')?)?,
        ObjectRole::StructuredExport
    );
    assert_eq!(
        object_role(&objects, &digest('d')?)?,
        ObjectRole::Asset(AssetRole::PageImage)
    );
    // A workspace archive's digest, named with this valid citation of the workspace, is not.
    let archive = workspace_archive()?;
    let refused = err_of(object_role(&objects, &archive.object.digest))?;
    assert_eq!(refused.code, ErrorCode::NotFound);
    // Nor is the record's Markdown digest, which names no retained object of its own.
    assert_eq!(
        err_of(object_role(&objects, &digest('e')?))?.code,
        ErrorCode::NotFound
    );
    assert_eq!(cited.item_id, ItemId(Uuid::from_u128(ITEM)));
    Ok(())
}

#[test]
fn a_purged_revision_is_a_typed_not_found_with_its_replacement() -> TestResult {
    let purge_id = PurgeId(Uuid::from_u128(13));
    let error = invalidated(&RevisionMapping {
        purge_id,
        replacement: Some(revision('2')?),
    });
    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(matches!(
        error.detail.as_deref(),
        Some(ErrorDetail::Invalidated { purge_id: id, replacement: Some(_) }) if *id == purge_id
    ));
    Ok(())
}

#[test]
fn converter_text_is_located_from_the_record() -> TestResult {
    let record = record()?;
    let extraction = converted()?;
    let page = |page_no| SourceLocation::Direct {
        locator: SourceLocator::Page { page_no },
    };
    let lines = Selection::Lines {
        range: TextRange { start: 2, end: 6 },
    };
    assert_eq!(
        cited_locations(&extraction, Some(&record), &lines),
        vec![page(1), page(2)]
    );
    let second = Selection::Pages {
        range: PageRange { start: 2, end: 2 },
    };
    assert_eq!(
        cited_locations(&extraction, Some(&record), &second),
        vec![page(2)]
    );
    let cells = CellRange {
        sheet: "Q1".to_owned(),
        row_start: 1,
        row_end: 4,
        column_start: 1,
        column_end: 2,
    };
    assert_eq!(
        cited_locations(
            &extraction,
            None,
            &Selection::Cells {
                range: cells.clone()
            }
        ),
        vec![SourceLocation::Direct {
            locator: SourceLocator::Cells { range: cells }
        }]
    );
    Ok(())
}

#[test]
fn corrected_or_supplied_text_has_no_place_in_the_original() -> TestResult {
    let record = record()?;
    let mut extraction = converted()?;
    extraction.corrected = true;
    assert_eq!(
        cited_locations(&extraction, Some(&record), &Selection::All),
        vec![SourceLocation::Unresolved {
            reason: UnresolvedReason::CorrectedText
        }]
    );
    extraction.corrected = false;
    extraction.text_origin = TextOrigin::SuppliedByAgent;
    assert_eq!(
        cited_locations(&extraction, Some(&record), &Selection::All),
        vec![SourceLocation::Unresolved {
            reason: UnresolvedReason::SuppliedText
        }]
    );
    Ok(())
}

#[test]
fn omitted_locations_are_filled_and_supplied_ones_must_match() -> TestResult {
    let record = record()?;
    let computed = cited_locations(&converted()?, Some(&record), &Selection::All);
    let mut omitted = citation(revision('1')?, Selection::All)?;
    fill_locations(&mut omitted, computed.clone(), "/source")?;
    assert_eq!(omitted.locations, computed);
    let mut matching = omitted.clone();
    fill_locations(&mut matching, computed.clone(), "/source")?;
    let mut guessed = omitted.clone();
    guessed.locations = vec![SourceLocation::Inferred {
        locator: SourceLocator::Page { page_no: 1 },
    }];
    let refused = err_of(fill_locations(&mut guessed, computed, "/bindings/0/source"))?;
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(
        refused.field.as_deref(),
        Some("/bindings/0/source/locations")
    );
    Ok(())
}

#[test]
fn a_stored_record_decodes_and_garbage_does_not() -> TestResult {
    let record = record()?;
    let bytes = serde_json::to_vec(&record)?;
    assert_eq!(decode_conversion_record(&bytes)?, record);
    let refused = err_of(decode_conversion_record(b"{\"outcome\":{}}"))?;
    assert_eq!(refused.code, ErrorCode::Internal);
    Ok(())
}

#[test]
fn a_stored_outcome_with_a_field_beside_its_tag_is_refused() -> TestResult {
    let mut value = serde_json::to_value(record()?)?;
    let object = value
        .as_object_mut()
        .ok_or("a conversion record is a mapping")?;
    object.insert(
        "outcome".to_owned(),
        json!({ "status": "completed", "pages_skipped": 3 }),
    );
    let bytes = serde_json::to_vec(&value)?;
    // Plain serde reads the outcome as completed and drops the extra field.
    let lenient: ConversionRecord = serde_json::from_slice(&bytes)?;
    assert_eq!(lenient.page_count, Some(2));
    let refused = err_of(decode_conversion_record(&bytes))?;
    assert_eq!(refused.code, ErrorCode::Internal);
    assert_eq!(refused.field.as_deref(), Some("/outcome"));
    Ok(())
}

#[test]
fn a_section_is_located_as_the_lines_its_heading_opens() -> TestResult {
    let markdown = [
        "# Report",
        "intro",
        "",
        "## Revenue",
        "Q1 was up",
        "~~~",
        "# not a heading",
        "~~~",
        "## Costs",
        "flat",
    ]
    .join("\n");
    let revenue = section_lines(&markdown, "Revenue")?;
    assert_eq!(revenue, TextRange { start: 4, end: 8 });
    let report = section_lines(&markdown, "Report")?;
    assert_eq!(report, TextRange { start: 1, end: 10 });
    let record = record()?;
    let located = cited_locations(
        &converted()?,
        Some(&record),
        &Selection::Lines { range: revenue },
    );
    assert_eq!(
        located,
        vec![SourceLocation::Direct {
            locator: SourceLocator::Page { page_no: 2 }
        }]
    );
    // A heading inside a fenced block is text, not a heading.
    let fenced = err_of(section_lines(&markdown, "not a heading"))?;
    assert_eq!(fenced.code, ErrorCode::NotFound);
    assert_eq!(fenced.field.as_deref(), Some("/selection/heading"));
    let twice = err_of(section_lines("## A\nx\n## A\ny", "A"))?;
    assert_eq!(twice.code, ErrorCode::InvalidInput);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/records.rs"]
mod records;
