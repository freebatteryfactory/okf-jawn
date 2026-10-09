//! A citation opens only the objects of its item revision, reads at an invalidated revision are
//! typed, and citation locations are the server's.

use okf_jawn_contract::common::{CellRange, PageRange, TextRange};
use okf_jawn_contract::error::{ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::TextOrigin;
use okf_jawn_contract::identity::{
    ArtifactId, ItemId, JobId, PurgeId, TenantId, WorkspaceId, WorkspacePath,
};
use okf_jawn_contract::read::{AssetRole, OutlineEntry, OutlineEntryKind, Selection};
use okf_jawn_contract::source::{SourceLocation, SourceLocator, UnresolvedReason};
use okf_jawn_core::conversion::ConversionRecord;
use okf_jawn_core::jobs::{ArtifactKind, ArtifactRecord, JobScope, RevisionMapping};
use okf_jawn_core::reading::{
    NoteSource, ObjectRole, RevisionObjects, UncitedReason, UncitedSource, cited_locations,
    decode_conversion_record, fill_locations, invalidated, markdown_outline, note_sources,
    object_role, section_lines,
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

fn entry(label: &str, level: u16, kind: OutlineEntryKind, selection: Selection) -> OutlineEntry {
    OutlineEntry {
        label: label.to_owned(),
        level,
        selection,
        kind,
    }
}

fn heading(label: &str, level: u16, start: u32, end: u32) -> OutlineEntry {
    entry(
        label,
        level,
        OutlineEntryKind::Heading,
        Selection::Lines {
            range: TextRange { start, end },
        },
    )
}

#[test]
fn a_section_is_located_as_the_lines_its_heading_opens() -> TestResult {
    // The outline a Markdown parser made of the shown text; a table captioned like a heading
    // is not a heading.
    let outline = vec![
        heading("Report", 1, 1, 10),
        heading("Revenue", 2, 4, 8),
        entry(
            "Costs",
            0,
            OutlineEntryKind::Table,
            Selection::Lines {
                range: TextRange { start: 6, end: 7 },
            },
        ),
        heading("Costs", 2, 9, 10),
    ];
    let revenue = section_lines(&outline, "Revenue")?;
    assert_eq!(revenue, TextRange { start: 4, end: 8 });
    assert_eq!(
        section_lines(&outline, " Costs ")?,
        TextRange { start: 9, end: 10 }
    );
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
    let missing = err_of(section_lines(&outline, "not a heading"))?;
    assert_eq!(missing.code, ErrorCode::NotFound);
    assert_eq!(missing.field.as_deref(), Some("/selection/heading"));
    let twice = [heading("A", 2, 1, 2), heading("A", 2, 3, 4)];
    let ambiguous = err_of(section_lines(&twice, "A"))?;
    assert_eq!(ambiguous.code, ErrorCode::InvalidInput);
    assert_eq!(ambiguous.field.as_deref(), Some("/selection/heading"));
    // An entry that selects no lines is the outline producer's fault, not the caller's.
    let unlined = [entry(
        "A",
        2,
        OutlineEntryKind::Heading,
        Selection::Section {
            heading: "A".to_owned(),
        },
    )];
    assert_eq!(
        err_of(section_lines(&unlined, "A"))?.code,
        ErrorCode::Internal
    );
    Ok(())
}

#[test]
fn an_item_outline_sections_end_before_the_next_heading_of_the_same_or_a_higher_level() {
    let text =
        "# Plan\nintro\n## Goals\none\n### Detail\ntwo\n## Risks\nthree\n\nSetext top\n===\nlast\n";
    assert_eq!(
        markdown_outline(text),
        vec![
            heading("Plan", 1, 1, 9),
            heading("Goals", 2, 3, 6),
            heading("Detail", 3, 5, 6),
            heading("Risks", 2, 7, 9),
            heading("Setext top", 1, 10, 12),
        ]
    );
}

#[test]
fn an_item_outline_takes_headings_from_the_parser_not_from_code() {
    let text =
        "Intro\n\n```\n# not a heading\n```\n\n    # indented code\n\n# Real `code` heading\nbody";
    assert_eq!(
        markdown_outline(text),
        vec![heading("Real code heading", 1, 9, 10)]
    );
    assert_eq!(markdown_outline(""), Vec::new());
}

#[test]
fn section_lines_agrees_for_the_converter_outline_and_an_item_outline() -> TestResult {
    // The same Markdown the fixture record describes: Overview on lines 1-4, Revenue on 5-8.
    let text = "# Overview\nfirst\nsecond\n\n# Revenue\n| Quarter | Revenue |\n|---|---|\n| Q1 | 1200.5 |\n";
    let converter = record()?.outline;
    let parsed = markdown_outline(text);
    for heading in ["Overview", "Revenue"] {
        assert_eq!(
            section_lines(&parsed, heading)?,
            section_lines(&converter, heading)?,
            "{heading}"
        );
    }
    Ok(())
}

#[test]
fn a_notes_okf_sources_are_classified_in_order_and_none_is_dropped() -> TestResult {
    let properties = serde_json::from_value(json!({
        "sources": [
            { "id": "web", "resource": "https://example.test/report" },
            { "id": "card", "resource": "card" },
            { "id": "scope", "resource": "all queries in project X" },
            { "id": "bare" },
            { "id": "up", "resource": "../sources/report-pdf.md" }
        ]
    }))?;
    let note = WorkspacePath::try_from("notes/plan.md".to_owned())?;
    let path = |text: &str| WorkspacePath::try_from(text.to_owned());
    assert_eq!(
        note_sources(&properties, &note),
        vec![
            NoteSource::Uncited(UncitedSource {
                resource: "https://example.test/report".to_owned(),
                reason: UncitedReason::External,
            }),
            NoteSource::Candidates {
                resource: "card".to_owned(),
                paths: vec![path("notes/card.md")?, path("card.md")?],
            },
            NoteSource::Uncited(UncitedSource {
                resource: "all queries in project X".to_owned(),
                reason: UncitedReason::Scope,
            }),
            NoteSource::Uncited(UncitedSource {
                resource: String::new(),
                reason: UncitedReason::NotFound,
            }),
            NoteSource::Candidates {
                resource: "../sources/report-pdf.md".to_owned(),
                paths: vec![path("sources/report-pdf.md")?],
            },
        ]
    );
    let bare = serde_json::from_value(json!({ "sources": { "resource": "card" } }))?;
    assert_eq!(note_sources(&bare, &note).len(), 1);
    assert_eq!(
        note_sources(&serde_json::from_value(json!({}))?, &note),
        Vec::new()
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/records.rs"]
mod records;
