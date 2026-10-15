//! Stored wire types read the records written before a field was added to them.
//!
//! `Job`, `Event`, `Proposal` (with the changes inside it), `Comment` and the `Upload` a
//! `create_upload` answer keeps in the mutation ledger are stored as they serialize, and refuse
//! unknown fields. A field added to one of them is optional and left out when absent (the
//! general rule of contract batch 2), so a row, a ledger response or an archive written before
//! the field existed still reads, and reads back unchanged: each record below is written in the
//! shape the type had before the field, decoded, checked to hold nothing for the new fields,
//! and serialized again to the very same JSON. Each new field is then shown on the wire under
//! its name when it is present.

use check::{TestResult, some};
use okf_jawn_contract::events::Event;
use okf_jawn_contract::import::{Job, Upload};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::proposal::{Change, Comment, Proposal};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const ITEM: &str = "22222222-2222-4222-8222-222222222222";
const JOB: &str = "66666666-6666-4666-8666-666666666666";
const UPLOAD: &str = "77777777-7777-4777-8777-777777777777";
const PROPOSAL: &str = "99999999-9999-4999-8999-999999999999";
const AT: &str = "2026-10-08T14:03:07.250Z";

fn revision() -> String {
    "a".repeat(40)
}

/// Decode `record`, then serialize it again: it must be the same JSON, so nothing was added.
fn reads_unchanged<T: DeserializeOwned + Serialize>(
    record: &Value,
) -> Result<T, Box<dyn std::error::Error>> {
    let decoded: T = serde_json::from_value(record.clone())?;
    assert_eq!(
        &serde_json::to_value(&decoded)?,
        record,
        "read back unchanged"
    );
    Ok(decoded)
}

/// `record` with `field` set to `value`, decoded and serialized again with the field in place.
fn carries<T: DeserializeOwned + Serialize>(
    record: &Value,
    field: &str,
    value: &Value,
) -> Result<T, Box<dyn std::error::Error>> {
    let mut with = record.clone();
    some(with.as_object_mut(), "a record is an object")?.insert(field.to_owned(), value.clone());
    let decoded: T = serde_json::from_value(with.clone())?;
    let written = serde_json::to_value(&decoded)?;
    assert_eq!(
        written.get(field),
        Some(value),
        "{field} is written under its name"
    );
    assert_eq!(written, with);
    Ok(decoded)
}

fn job_before() -> Value {
    json!({
        "id": JOB,
        "workspace_id": WORKSPACE,
        "kind": "restore_workspace",
        "state": "failed",
        "progress": 40,
        "attempt": 2,
        "warnings": [],
        "error": {"code": "internal", "message": "the archive did not read"},
        "item_ids": [],
        "restore": {"items": 3, "drafts_restored": 1, "drafts_unassigned": 0}
    })
}

fn event_before() -> Value {
    json!({
        "id": "e-1",
        "workspace_id": WORKSPACE,
        "kind": "proposal_updated",
        "at": AT,
        "revision": revision()
    })
}

fn proposal_before() -> Value {
    json!({
        "id": PROPOSAL,
        "workspace_id": WORKSPACE,
        "base_revision": revision(),
        "proposal_revision": "b".repeat(40),
        "content_digest": "c".repeat(64),
        "title": "Add a note",
        "description": "",
        "changes": [
            {"kind": "create", "path": "notes/a.md", "type_name": "note", "body": "# A",
             "properties": {}},
            {"kind": "archive", "item_id": ITEM}
        ],
        "kind": "content",
        "status": "open",
        "created_by": "agent",
        "created_via": "mcp_delegation",
        "created_at": AT
    })
}

fn comment_before() -> Value {
    json!({"id": "c-1", "author": "alice", "text": "Why this path?", "created_at": AT})
}

fn upload_before() -> Value {
    json!({
        "id": UPLOAD,
        "workspace_id": WORKSPACE,
        "upload_path": format!("/api/workspaces/{WORKSPACE}/uploads/{UPLOAD}/content"),
        "received_bytes": "0",
        "complete": false
    })
}

#[test]
fn a_job_stored_before_its_times_retry_flag_and_reports_reads_unchanged() -> TestResult {
    let job: Job = reads_unchanged(&job_before())?;
    assert!(job.created_at.is_none() && job.started_at.is_none() && job.finished_at.is_none());
    assert!(job.retryable.is_none() && job.backup.is_none());
    let restore = some(job.restore, "the restore report")?;
    assert!(restore.rows_left_out.is_none());
    for (field, value) in [
        ("created_at", json!(AT)),
        ("started_at", json!(AT)),
        ("finished_at", json!(AT)),
        ("retryable", json!(false)),
        ("backup", json!({"rows_left_out": 2})),
    ] {
        let _job: Job = carries(&job_before(), field, &value)?;
    }
    let mut counted = job_before();
    *some(counted.pointer_mut("/restore"), "the restore report")? =
        json!({"items": 3, "drafts_restored": 1, "drafts_unassigned": 0, "rows_left_out": 4});
    let job: Job = reads_unchanged(&counted)?;
    assert_eq!(
        some(job.restore, "the restore report")?.rows_left_out,
        Some(4)
    );
    Ok(())
}

#[test]
fn an_event_stored_before_its_proposal_and_count_reads_unchanged() -> TestResult {
    let event: Event = reads_unchanged(&event_before())?;
    assert!(event.proposal_id.is_none() && event.count.is_none());
    let named: Event = carries(&event_before(), "proposal_id", &json!(PROPOSAL))?;
    assert!(named.proposal_id.is_some());
    let counted: Event = carries(&event_before(), "count", &json!(12))?;
    assert_eq!(counted.count, Some(12));
    Ok(())
}

#[test]
fn a_proposal_stored_before_its_warnings_reads_unchanged_with_its_changes() -> TestResult {
    let proposal: Proposal = reads_unchanged(&proposal_before())?;
    assert!(proposal.warnings.is_empty());
    assert!(
        matches!(proposal.changes.first(), Some(Change::Create { .. })),
        "the stored create change reads as it was written"
    );
    let warned: Proposal = carries(
        &proposal_before(),
        "warnings",
        &json!([{"code": "missing_description", "message": "no description"}]),
    )?;
    assert_eq!(warned.warnings.len(), 1);
    Ok(())
}

/// Ruling 29: a `create` change stored before `item_kind` reads as a note and is written back
/// without the field; one naming a View keeps it.
#[test]
fn a_create_change_stored_before_its_item_kind_reads_as_a_note() -> TestResult {
    let before = json!({"kind": "create", "path": "notes/a.md", "type_name": "note",
        "body": "# A", "properties": {}});
    let change: Change = reads_unchanged(&before)?;
    assert_eq!(change.created_kind(), Some(ItemKind::Note));
    let view: Change = carries(&before, "item_kind", &json!("view"))?;
    assert_eq!(view.created_kind(), Some(ItemKind::View));
    Ok(())
}

#[test]
fn a_comment_stored_before_its_source_reads_unchanged() -> TestResult {
    let comment: Comment = reads_unchanged(&comment_before())?;
    assert!(comment.source.is_none());
    let cited: Comment = carries(
        &comment_before(),
        "source",
        &json!({
            "workspace_id": WORKSPACE,
            "item_id": ITEM,
            "path": "notes/a.md",
            "revision": revision(),
            "selection": {"kind": "lines", "range": {"start": 2, "end": 4}}
        }),
    )?;
    assert!(cited.source.is_some());
    Ok(())
}

#[test]
fn an_upload_answer_kept_in_the_ledger_before_its_new_fields_reads_unchanged() -> TestResult {
    let upload: Upload = reads_unchanged(&upload_before())?;
    assert!(upload.filename.is_none() && upload.relative_path.is_none());
    assert!(upload.size.is_none() && upload.created_at.is_none() && upload.consumed_by.is_none());
    for (field, value) in [
        ("filename", json!("Report.PDF")),
        ("relative_path", json!("2026/q3")),
        ("size", json!("2048")),
        ("created_at", json!(AT)),
        ("consumed_by", json!(JOB)),
    ] {
        let _upload: Upload = carries(&upload_before(), field, &value)?;
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
