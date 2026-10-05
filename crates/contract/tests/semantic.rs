//! Semantic controls for serialization and validation; not whole-product acceptance.

use okf_jawn_contract::history::CommitRequest;
use okf_jawn_contract::identity::{At, Digest, Revision, WorkspaceId, WorkspacePath};
use okf_jawn_contract::import::{Job, JobKind};
use okf_jawn_contract::metadata::operations;
use okf_jawn_contract::read::ReadItemRequest;
use okf_jawn_contract::views::ViewDocument;
use serde_json::json;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

/// Wire names of every job kind, one per operation that starts a job.
const JOB_KINDS: &[&str] = &[
    "import",
    "redigest",
    "export_workspace",
    "backup_workspace",
    "restore_workspace",
    "rebuild_index",
    "export_view",
];

fn binding(name: &str, workspace: &str) -> serde_json::Value {
    json!({
        "name": name,
        "source": {
            "workspace_id": workspace,
            "item_id": "22222222-2222-4222-8222-222222222222",
            "path": "data/metrics.csv",
            "revision": "a".repeat(40),
            "selection": {"kind": "all"}
        },
        "units": {},
        "transforms": []
    })
}

#[test]
fn revision_does_not_accept_a_selector() -> Result<(), Box<dyn Error>> {
    assert!(Revision::try_from("latest".to_owned()).is_err());
    assert!(Revision::try_from("A".repeat(40)).is_err());
    let revision = Revision::try_from("a".repeat(40))?;
    assert_eq!(revision.as_str(), "a".repeat(40));
    let selector: At = serde_json::from_value(json!({"kind":"latest"}))?;
    assert_eq!(selector, At::Latest);
    Ok(())
}

#[test]
fn workspace_paths_reject_parent_and_git_traversal() -> Result<(), Box<dyn Error>> {
    for path in [
        "../client",
        "/client",
        "a/../b",
        "a//b",
        "C:/docs",
        ".git/config",
        "a\\b",
    ] {
        assert!(WorkspacePath::try_from(path.to_owned()).is_err(), "{path}");
    }
    assert_eq!(
        WorkspacePath::try_from("Clients/one.md".to_owned())?.as_str(),
        "Clients/one.md"
    );
    Ok(())
}

#[test]
fn optional_cursor_is_omitted_or_null_without_changing_semantics() -> Result<(), Box<dyn Error>> {
    let raw = json!({"workspace_id":"11111111-1111-4111-8111-111111111111",
        "item_id":"22222222-2222-4222-8222-222222222222", "at":{"kind":"latest"},
        "view":"text", "selection":{"kind":"all"}, "max_bytes":4096,"max_images":0});
    let missing: ReadItemRequest = serde_json::from_value(raw.clone())?;
    let mut explicit = raw;
    explicit
        .as_object_mut()
        .ok_or("fixture must be object")?
        .insert("cursor".to_owned(), serde_json::Value::Null);
    let nullable: ReadItemRequest = serde_json::from_value(explicit)?;
    assert_eq!(missing.cursor, nullable.cursor);
    assert!(serde_json::to_value(missing)?.get("cursor").is_none());
    Ok(())
}

#[test]
fn content_identity_cannot_be_a_filename() -> Result<(), Box<dyn Error>> {
    assert!(Digest::try_from("FINAL.pdf".to_owned()).is_err());
    assert_eq!(Digest::try_from("b".repeat(64))?.as_str(), "b".repeat(64));
    Ok(())
}

#[test]
fn view_document_six_component_round_trips_with_deny_unknown_fields() -> Result<(), Box<dyn Error>>
{
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/views/view-document-six-component.json");
    let raw = fs::read_to_string(&path)?;
    let parsed: ViewDocument = serde_json::from_str(&raw)?;
    assert_eq!(parsed.schema_version, 1);
    assert_eq!(
        parsed.grammar,
        okf_jawn_contract::views::RenderGrammar::JsonRender
    );
    assert_eq!(parsed.spec.get("root"), Some(&json!("root")));
    assert_eq!(
        parsed.spec.pointer("/elements/root/type").cloned(),
        Some(json!("Stack"))
    );
    let reserialized = serde_json::to_value(&parsed)?;
    let again: ViewDocument = serde_json::from_value(reserialized.clone())?;
    assert_eq!(serde_json::to_value(&again)?, reserialized);
    let mut unknown = serde_json::from_str::<serde_json::Value>(&raw)?;
    unknown
        .as_object_mut()
        .ok_or("fixture must be object")?
        .insert("unexpected_field".to_owned(), json!(true));
    assert!(serde_json::from_value::<ViewDocument>(unknown).is_err());
    Ok(())
}

#[test]
fn a_snapshot_request_carries_no_expected_head() -> Result<(), Box<dyn Error>> {
    let request = json!({
        "workspace_id": "11111111-1111-4111-8111-111111111111",
        "item_ids": ["22222222-2222-4222-8222-222222222222"],
        "message": "Quarterly numbers",
        "idempotency_key": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    });
    let decoded: CommitRequest = serde_json::from_value(request.clone())?;
    assert_eq!(decoded.item_ids.len(), 1);
    let mut with_head = request;
    with_head
        .as_object_mut()
        .ok_or("request must be an object")?
        .insert("expected_head".to_owned(), json!("a".repeat(40)));
    assert!(
        serde_json::from_value::<CommitRequest>(with_head).is_err(),
        "head movement alone is not a Snapshot precondition"
    );
    Ok(())
}

#[test]
fn every_job_states_its_kind() -> Result<(), Box<dyn Error>> {
    let mut job = json!({
        "id": "66666666-6666-4666-8666-666666666666",
        "workspace_id": "11111111-1111-4111-8111-111111111111",
        "kind": "import",
        "state": "queued",
        "progress": 0,
        "attempt": 1,
        "warnings": [],
        "item_ids": []
    });
    let decoded: Job = serde_json::from_value(job.clone())?;
    assert_eq!(decoded.kind, JobKind::Import);
    for kind in JOB_KINDS {
        *job.get_mut("kind").ok_or("kind field")? = json!(kind);
        let decoded: Job = serde_json::from_value(job.clone())?;
        assert_eq!(serde_json::to_value(decoded.kind)?, json!(kind));
    }
    job.as_object_mut()
        .ok_or("job must be an object")?
        .remove("kind");
    assert!(
        serde_json::from_value::<Job>(job).is_err(),
        "a job without a kind must not decode"
    );
    Ok(())
}

#[test]
fn every_operation_that_starts_a_job_has_a_kind() {
    let starters: Vec<&str> = operations()
        .iter()
        .filter(|operation| operation.success_status == 202 && operation.id != "retry_job")
        .map(|operation| operation.id)
        .collect();
    assert_eq!(
        starters.len(),
        JOB_KINDS.len(),
        "202 operations: {starters:?}"
    );
}

#[test]
fn a_view_reports_the_bindings_outside_its_workspace() -> Result<(), Box<dyn Error>> {
    let home = "11111111-1111-4111-8111-111111111111";
    let elsewhere = "33333333-3333-4333-8333-333333333333";
    let view: ViewDocument = serde_json::from_value(json!({
        "schema_version": 1,
        "title": "Metrics",
        "description": "Quarterly metrics",
        "mode": "pinned",
        "grammar": "vega_lite",
        "bindings": [binding("local", home), binding("foreign", elsewhere)],
        "spec": {}
    }))?;
    let workspace: WorkspaceId = serde_json::from_value(json!(home))?;
    let outside: Vec<&str> = view
        .bindings_outside(workspace)
        .into_iter()
        .map(|found| found.name.as_str())
        .collect();
    assert_eq!(outside, ["foreign"]);
    let other: WorkspaceId = serde_json::from_value(json!(elsewhere))?;
    let outside_other: Vec<&str> = view
        .bindings_outside(other)
        .into_iter()
        .map(|found| found.name.as_str())
        .collect();
    assert_eq!(outside_other, ["local"]);
    Ok(())
}
