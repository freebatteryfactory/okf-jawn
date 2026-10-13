//! The mutation ledger against the real records database.
//!
//! `mutation_lease_compare_and_set_*` tests are the receipt of `mutation-lease-compare-and-set`.
//! A lease's expiry is simulated by moving its stored expiry into the past through a second
//! SQLite connection, the same effect as waiting out `MUTATION_LEASE_SECONDS`.
#![cfg(feature = "runtime")]

use check::{TestResult, err_of};
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{Digest, WorkspaceId};
use okf_jawn_contract::metadata::OperationName;
use okf_jawn_core::mutations::{BeginOutcome, MutationKey, MutationLease, MutationStore};
use okf_jawn_storage::Storage;
use rusqlite::Connection;
use serde_json::json;

const HOME: &str = "11111111-1111-4111-8111-111111111111";
const ELSEWHERE: &str = "22222222-2222-4222-8222-222222222222";

fn key(client: Option<&str>) -> Result<MutationKey, Box<dyn std::error::Error>> {
    Ok(MutationKey {
        tenant_id: serde_json::from_value(json!("local"))?,
        subject: "owner".to_owned(),
        client_id: client.map(str::to_owned),
        operation: OperationName::CreateWorkspace,
        key: serde_json::from_value(json!("00000000-0000-4000-8000-000000000001"))?,
    })
}

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn workspace(id: &str) -> Result<WorkspaceId, serde_json::Error> {
    serde_json::from_value(json!(id))
}

/// The workspace the ledger row of `lease`'s mutation records.
fn recorded(storage: &Storage, lease: MutationLease) -> Result<Option<String>, rusqlite::Error> {
    Connection::open(storage.data().records_path())?.query_row(
        "SELECT workspace_id FROM mutations WHERE mutation_id = ?1",
        [lease.mutation_id.0.to_string()],
        |row| row.get(0),
    )
}

/// Move every live lease's expiry into the past.
fn expire_leases(storage: &Storage) -> TestResult {
    let connection = Connection::open(storage.data().records_path())?;
    connection.execute("UPDATE mutations SET lease_expires_ms = 0", [])?;
    Ok(())
}

/// Move every finished row's finish time `days` days into the past.
fn age_finished_rows(storage: &Storage, days: i64) -> TestResult {
    let connection = Connection::open(storage.data().records_path())?;
    connection.execute(
        "UPDATE mutations SET finished_ms = finished_ms - ?1, lease_expires_ms = 0",
        [days.saturating_mul(86_400_000)],
    )?;
    Ok(())
}

fn new_lease(outcome: BeginOutcome) -> Result<MutationLease, String> {
    match outcome {
        BeginOutcome::New(lease) => Ok(lease),
        other => Err(format!("expected a new lease, got {other:?}")),
    }
}

fn abandoned_lease(outcome: BeginOutcome) -> Result<MutationLease, String> {
    match outcome {
        BeginOutcome::Abandoned { lease } => Ok(lease),
        other => Err(format!("expected an abandoned mutation, got {other:?}")),
    }
}

fn replayed(outcome: BeginOutcome) -> Result<serde_json::Value, String> {
    match outcome {
        BeginOutcome::Replay(stored) => Ok(stored.body),
        other => Err(format!("expected a replay, got {other:?}")),
    }
}

/// The first attempt, then a takeover after its lease expired: two grants of one mutation.
async fn superseded(
    storage: &Storage,
) -> Result<(MutationLease, MutationLease), Box<dyn std::error::Error>> {
    let ledger = storage.mutations();
    let key = key(None)?;
    let first = new_lease(ledger.begin(&key, &digest('a')?, None).await?)?;
    expire_leases(storage)?;
    let second = abandoned_lease(ledger.begin(&key, &digest('a')?, None).await?)?;
    assert_eq!(second.mutation_id, first.mutation_id);
    assert_ne!(second.token, first.token);
    Ok((first, second))
}

#[tokio::test]
async fn mutation_ledger_replays_a_completed_response() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let lease = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    ledger.complete(lease, json!({ "answer": 1 }), None).await?;
    let body = replayed(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(body, json!({ "answer": 1 }));
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_conflicts_when_the_key_is_reused_with_another_body() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    match ledger.begin(&key(None)?, &digest('b')?, None).await? {
        BeginOutcome::Conflict { operation } => {
            assert_eq!(operation, OperationName::CreateWorkspace);
        }
        other => return Err(format!("expected a conflict, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_reports_a_live_lease_as_in_progress() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let lease = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    match ledger.begin(&key(None)?, &digest('a')?, None).await? {
        BeginOutcome::InProgress {
            mutation_id,
            retry_after,
        } => {
            assert_eq!(mutation_id, lease.mutation_id);
            assert!((1..=120).contains(&retry_after), "{retry_after}");
        }
        other => return Err(format!("expected in progress, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_resumes_a_released_attempt_at_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let first = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    ledger.release(first).await?;
    let second = abandoned_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(second.mutation_id, first.mutation_id);
    assert_ne!(second.token, first.token);
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_keeps_a_crashed_attempt_until_it_is_reconciled() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let first = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    // Neither completed nor released, and far past the retention window.
    age_finished_rows(&storage, 30)?;
    let second = abandoned_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(second.mutation_id, first.mutation_id);
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_drops_finished_rows_after_seven_days() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let completed = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    ledger.complete(completed, json!({}), None).await?;
    age_finished_rows(&storage, 6)?;
    replayed(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    age_finished_rows(&storage, 2)?;
    let fresh = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_ne!(fresh.mutation_id, completed.mutation_id);
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_separates_a_connector_from_its_subject() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let direct = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    let connector = new_lease(
        ledger
            .begin(&key(Some("claude"))?, &digest('b')?, None)
            .await?,
    )?;
    assert_ne!(direct.mutation_id, connector.mutation_id);
    Ok(())
}

#[tokio::test]
async fn mutation_lease_compare_and_set_after_the_takeover_completes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let (stale, current) = superseded(&storage).await?;
    ledger
        .complete(current, json!({ "by": "takeover" }), None)
        .await?;
    let error = err_of(ledger.complete(stale, json!({ "by": "stale" }), None).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    ledger.release(stale).await?;
    let body = replayed(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(body, json!({ "by": "takeover" }));
    Ok(())
}

#[tokio::test]
async fn mutation_lease_compare_and_set_before_the_takeover_completes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let (stale, current) = superseded(&storage).await?;
    let error = err_of(ledger.complete(stale, json!({ "by": "stale" }), None).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    ledger.release(stale).await?;
    // The stale release ended nothing: the takeover still holds a live lease.
    match ledger.begin(&key(None)?, &digest('a')?, None).await? {
        BeginOutcome::InProgress { mutation_id, .. } => {
            assert_eq!(mutation_id, current.mutation_id);
        }
        other => return Err(format!("expected in progress, got {other:?}").into()),
    }
    ledger
        .complete(current, json!({ "by": "takeover" }), None)
        .await?;
    let body = replayed(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(body, json!({ "by": "takeover" }));
    Ok(())
}

#[tokio::test]
async fn mutation_lease_compare_and_set_release_of_a_completed_mutation_is_a_no_op() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let lease = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    ledger
        .complete(lease, json!({ "kept": true }), None)
        .await?;
    ledger.release(lease).await?;
    // Completing again under the same grant keeps the first response.
    ledger
        .complete(lease, json!({ "kept": false }), None)
        .await?;
    let body = replayed(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(body, json!({ "kept": true }));
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_keeps_the_workspace_its_first_begin_named() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let first = new_lease(
        ledger
            .begin(&key(None)?, &digest('a')?, Some(workspace(HOME)?))
            .await?,
    )?;
    assert_eq!(recorded(&storage, first)?, Some(HOME.to_owned()));
    ledger.release(first).await?;
    // The same key and digest are the same request: a resumed begin changes nothing.
    let resumed = abandoned_lease(
        ledger
            .begin(&key(None)?, &digest('a')?, Some(workspace(ELSEWHERE)?))
            .await?,
    )?;
    // A completion's workspace is recorded only when begin recorded none.
    let named = Some(workspace(ELSEWHERE)?);
    ledger.complete(resumed, json!({}), named).await?;
    assert_eq!(recorded(&storage, resumed)?, Some(HOME.to_owned()));
    Ok(())
}

#[tokio::test]
async fn mutation_ledger_records_the_workspace_a_completion_names_when_begin_had_none() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let ledger = storage.mutations();
    let lease = new_lease(ledger.begin(&key(None)?, &digest('a')?, None).await?)?;
    assert_eq!(recorded(&storage, lease)?, None);
    let named = Some(workspace(HOME)?);
    ledger.complete(lease, json!({ "id": HOME }), named).await?;
    assert_eq!(recorded(&storage, lease)?, Some(HOME.to_owned()));
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
