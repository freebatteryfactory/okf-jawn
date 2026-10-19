//! Cleanup wire compatibility and scope follow lifecycle design section 3.1 and rulings 1, 2, 7.
//! Literal wire values exercise the new declarations without constructing new Rust types.
use check::{TestResult, some};
use okf_jawn_contract::import::{Job, JobKind};
use okf_jawn_contract::metadata::{OperationName, operations};
use okf_jawn_contract::purge::PurgeReport;
use serde_json::{Value, json};

fn wire_job(kind: &str) -> Value {
    json!({
        "id": "66666666-6666-4666-8666-666666666666",
        "kind": kind, "state": "succeeded", "progress": 100, "attempt": 1,
        "warnings": [], "item_ids": []
    })
}

fn field(raw: &mut Value, name: &str, value: Value) -> TestResult {
    some(raw.as_object_mut(), "the job object")?.insert(name.to_owned(), value);
    Ok(())
}

fn report_field(raw: &mut Value, name: &str, value: Value) -> TestResult {
    let report = some(raw.get_mut("clean_up"), "the clean-up report")?;
    some(report.as_object_mut(), "the report object")?.insert(name.to_owned(), value);
    Ok(())
}

#[test]
fn clean_up_is_tenant_work_started_by_the_existing_200_discard_operation() -> TestResult {
    let kind: JobKind = serde_json::from_value(json!("clean_up"))?;
    assert!(JobKind::ALL.contains(&kind));
    assert!(kind.is_tenant());
    assert_eq!(kind.started_by(), OperationName::DiscardUpload);
    let table = operations();
    assert_eq!(table.len(), 83, "R1 adds no application operation");
    let discard = table
        .iter()
        .find(|entry| entry.id == "discard_upload")
        .ok_or("the existing discard_upload operation")?;
    assert_eq!(discard.success_status, 200);
    assert_eq!(discard.rules.starts_job, Some(kind));
    let no_workspace: Job = serde_json::from_value(wire_job("clean_up"))?;
    no_workspace.check()?;
    let mut wrong_scope = wire_job("clean_up");
    field(
        &mut wrong_scope,
        "workspace_id",
        json!("11111111-1111-4111-8111-111111111111"),
    )?;
    let wrong_scope: Job = serde_json::from_value(wrong_scope)?;
    assert!(wrong_scope.check().is_err());
    Ok(())
}

#[test]
fn clean_up_counts_round_trip_as_strings_and_a_bounded_file_count() -> TestResult {
    let mut raw = wire_job("clean_up");
    field(
        &mut raw,
        "clean_up",
        json!({
            "objects_removed": "18446744073709551616",
            "objects_kept": "7",
            "upload_files_removed": 2
        }),
    )?;
    let job: Job = serde_json::from_value(raw.clone())?;
    assert_eq!(serde_json::to_value(job)?, raw);
    let schema = serde_json::to_value(schemars::schema_for!(Job))?;
    assert!(schema.pointer("/properties/clean_up").is_some());
    let mut unknown = raw.clone();
    report_field(&mut unknown, "unexpected", json!(true))?;
    assert!(serde_json::from_value::<Job>(unknown).is_err());
    let mut numeric = raw.clone();
    report_field(&mut numeric, "objects_removed", json!(1))?;
    assert!(serde_json::from_value::<Job>(numeric).is_err());
    report_field(&mut raw, "upload_files_removed", json!(4294967296_u64))?;
    assert!(serde_json::from_value::<Job>(raw).is_err());
    Ok(())
}

#[test]
fn historical_jobs_without_clean_up_still_decode_and_omit_the_absent_field() -> TestResult {
    let mut raw = wire_job("import");
    field(
        &mut raw,
        "workspace_id",
        json!("11111111-1111-4111-8111-111111111111"),
    )?;
    let job: Job = serde_json::from_value(raw.clone())?;
    job.check()?;
    let encoded = serde_json::to_value(job)?;
    assert_eq!(encoded.get("clean_up"), None);
    assert_eq!(encoded, raw);
    Ok(())
}

#[test]
fn purge_count_descriptions_state_the_approved_sweep_and_workspace_revision_meanings() -> TestResult
{
    let schema = serde_json::to_value(schemars::schema_for!(PurgeReport))?;
    assert_eq!(
        schema.pointer("/properties/objects_kept_shared/description"),
        Some(&json!(
            "Stored objects kept because a surviving record, retained history, pending put or running\nwrite still references, pends or pins them, as a decimal string. Includes objects the\ntarget never referenced."
        ))
    );
    assert_eq!(
        schema.pointer("/properties/revisions_invalidated/description"),
        Some(&json!(
            "Revisions the purge's records name for the workspace, including earlier item-purge\nmappings. Every revision of a purged workspace is invalidated, whether counted or not."
        ))
    );
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
