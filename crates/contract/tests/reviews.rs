//! Imported review claims and the item content digest keep one wire shape.

use okf_jawn_contract::item::ItemDocument;
use okf_jawn_contract::review::{ImportedClaim, ListReviewsResponse};
use serde_json::{Value, json};
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn item_document() -> Value {
    json!({
        "summary": {
            "id": "00000000-0000-0000-0000-00000000000a",
            "path": "notes/plan.md",
            "title": "Plan",
            "description": "",
            "type_name": "Note",
            "kind": "note",
            "revision": "a".repeat(40),
            "status": "stable",
            "archived": false,
        },
        "body": "text",
        "properties": {},
        "content_digest": "b".repeat(64),
    })
}

#[test]
fn an_imported_claim_round_trips_as_written() -> TestResult {
    let claim: ImportedClaim =
        serde_json::from_value(json!({ "by": "human:walter", "at": "last Tuesday" }))?;
    assert_eq!(claim.by.as_deref(), Some("human:walter"));
    assert_eq!(claim.at.as_deref(), Some("last Tuesday"));
    let again: ImportedClaim = serde_json::from_value(serde_json::to_value(&claim)?)?;
    assert_eq!(again, claim);
    let bare: ImportedClaim = serde_json::from_value(json!({}))?;
    assert_eq!(serde_json::to_value(&bare)?, json!({}));
    Ok(())
}

#[test]
fn an_imported_claim_refuses_unknown_fields() {
    let refused =
        serde_json::from_value::<ImportedClaim>(json!({ "by": "human:walter", "extra": 1 }));
    assert!(refused.is_err());
}

#[test]
fn a_review_list_without_imported_claims_reads_as_empty() -> TestResult {
    let list: ListReviewsResponse = serde_json::from_value(json!({ "items": [] }))?;
    assert_eq!(list.imported, Vec::<ImportedClaim>::new());
    assert_eq!(serde_json::to_value(&list)?, json!({ "items": [] }));
    let claimed: ListReviewsResponse =
        serde_json::from_value(json!({ "items": [], "imported": [{ "by": "x" }] }))?;
    assert_eq!(claimed.imported.len(), 1);
    Ok(())
}

#[test]
fn an_item_document_requires_its_content_digest() -> TestResult {
    let complete: ItemDocument = serde_json::from_value(item_document())?;
    assert_eq!(complete.content_digest.as_str(), "b".repeat(64));
    let mut without = item_document();
    if let Some(object) = without.as_object_mut() {
        object.remove("content_digest");
    }
    assert!(serde_json::from_value::<ItemDocument>(without).is_err());
    Ok(())
}
