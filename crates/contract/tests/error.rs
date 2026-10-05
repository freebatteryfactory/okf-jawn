//! `ApiError` stays small enough to return by value and keeps typed context on the wire.

use std::error::Error;

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::history::FileChangeKind;
use serde_json::json;

// Clippy's `result_large_err` fires at 128 bytes; every `Result<_, ApiError>` relies on this.
const _: () = assert!(std::mem::size_of::<ApiError>() < 128);

#[test]
fn typed_detail_is_unchanged_on_the_wire() -> Result<(), Box<dyn Error>> {
    let connector_id = serde_json::from_value(json!("44444444-4444-4444-8444-444444444444"))?;
    let error = ApiError::new(ErrorCode::AlreadyIssued, "Connector already issued")
        .with_detail(ErrorDetail::AlreadyIssued { connector_id });
    let detail: &ErrorDetail = error.detail.as_deref().ok_or("detail must be present")?;
    assert_eq!(detail, &ErrorDetail::AlreadyIssued { connector_id });
    assert_eq!(
        serde_json::to_value(&error)?,
        json!({
            "code": "already_issued",
            "message": "Connector already issued",
            "detail": {
                "kind": "already_issued",
                "connector_id": "44444444-4444-4444-8444-444444444444"
            }
        })
    );
    Ok(())
}

#[test]
fn with_field_names_the_input_to_correct() -> Result<(), Box<dyn Error>> {
    let error = ApiError::new(ErrorCode::InvalidInput, "path must be relative").with_field("/path");
    assert_eq!(error.field.as_deref(), Some("/path"));
    assert_eq!(
        serde_json::to_value(&error)?.get("field"),
        Some(&json!("/path"))
    );
    Ok(())
}

#[test]
fn not_implemented_is_a_wire_code_of_its_own() -> Result<(), Box<dyn Error>> {
    assert_eq!(
        serde_json::to_value(ErrorCode::NotImplemented)?,
        json!("not_implemented")
    );
    let decoded: ErrorCode = serde_json::from_value(json!("not_implemented"))?;
    assert_eq!(decoded, ErrorCode::NotImplemented);
    Ok(())
}

#[test]
fn a_draft_conflict_lists_every_conflicting_item_with_typed_changes() -> Result<(), Box<dyn Error>>
{
    let wire = json!({
        "code": "conflict",
        "message": "Two selected items changed after their drafts were based",
        "detail": {
            "kind": "draft_conflict",
            "items": [
                {
                    "item_id": "22222222-2222-4222-8222-222222222222",
                    "draft_base": "a".repeat(40),
                    "current_revision": "b".repeat(40),
                    "deleted": false,
                    "changes": [{
                        "old_path": "notes/a.md",
                        "new_path": "notes/a.md",
                        "patch": "@@ -1 +1 @@
-old
+new
",
                        "binary": false,
                        "kind": "modified"
                    }]
                },
                {
                    "item_id": "33333333-3333-4333-8333-333333333333",
                    "draft_base": "a".repeat(40),
                    "current_revision": "b".repeat(40),
                    "deleted": true,
                    "changes": [{
                        "old_path": "notes/b.md",
                        "patch": "",
                        "binary": false,
                        "kind": "removed"
                    }]
                }
            ]
        }
    });
    let error: ApiError = serde_json::from_value(wire.clone())?;
    let Some(ErrorDetail::DraftConflict { items }) = error.detail.as_deref() else {
        return Err("expected a draft_conflict detail".into());
    };
    let deleted: Vec<bool> = items.iter().map(|item| item.deleted).collect();
    assert_eq!(deleted, [false, true]);
    let kinds: Vec<&FileChangeKind> = items
        .iter()
        .flat_map(|item| &item.changes)
        .map(|change| &change.kind)
        .collect();
    assert_eq!(kinds, [&FileChangeKind::Modified, &FileChangeKind::Removed]);
    assert_eq!(serde_json::to_value(&error)?, wire);
    Ok(())
}

#[test]
fn an_untyped_diff_is_not_a_draft_conflict() {
    let old = json!({
        "kind": "draft_conflict",
        "item_id": "22222222-2222-4222-8222-222222222222",
        "draft_base": "a".repeat(40),
        "current_revision": "b".repeat(40),
        "diff": {"anything": true}
    });
    assert!(serde_json::from_value::<ErrorDetail>(old).is_err());
}
