//! `RecordStore` against the real records database: jobs and their leases, artifacts, purge
//! records, derived objects, reviews and receipts.
#![cfg(feature = "runtime")]

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{Digest, Revision, TenantId};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::purge::{PurgeState, PurgeTarget};
use okf_jawn_contract::workspace::{ArtifactKind, BackupReport};
use okf_jawn_core::jobs::{
    DerivedKind, DerivedObject, JobCompletion, JobScope, JobSpec, NewArtifact, NewJob, NewPurge,
    RecordStore,
};
use okf_jawn_core::storage::{ObjectInfo, Page, Provenance, StorageScope};
use okf_jawn_storage::Storage;
use rusqlite::Connection;
use serde_json::json;

use check::{TestResult, err_of, some};

/// The identity numbered `n`, of the type the context names.
macro_rules! uuid {
    ($n:expr) => {
        serde_json::from_value(json!(uuid_text($n)))
    };
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// A fixed UUID spelling numbered `n`.
fn uuid_text(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::try_from("local".to_owned())?)
}

fn scope() -> Fallible<StorageScope> {
    Ok(StorageScope {
        tenant_id: tenant()?,
        workspace_id: uuid!(1)?,
    })
}

fn workspace_scope() -> Fallible<JobScope> {
    Ok(JobScope::Workspace(scope()?))
}

fn owner() -> Provenance {
    Provenance {
        subject: "owner".to_owned(),
        route: okf_jawn_contract::access::AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn new_job(mutation: u32, spec: JobSpec) -> Fallible<NewJob> {
    Ok(NewJob {
        mutation_id: uuid!(mutation)?,
        initiator: owner(),
        spec,
    })
}

fn digest(fill: char) -> Fallible<Digest> {
    Ok(Digest::try_from(fill.to_string().repeat(64))?)
}

fn revision(fill: char) -> Fallible<Revision> {
    Ok(Revision::try_from(fill.to_string().repeat(40))?)
}

fn completion(lease: okf_jawn_core::jobs::JobLease) -> JobCompletion {
    JobCompletion {
        lease,
        revision: None,
        item_ids: Vec::new(),
        artifact: None,
        restore: None,
        backup: None,
        outputs: Vec::new(),
        warnings: Vec::new(),
    }
}

fn expire_job_leases(storage: &Storage) -> TestResult {
    let connection = Connection::open(storage.data().records_path())?;
    connection.execute(
        "UPDATE jobs SET lease_expires_ms = 0 WHERE state = 'running'",
        [],
    )?;
    Ok(())
}

#[tokio::test]
async fn a_repeated_job_mutation_returns_the_first_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let first = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let again = records
        .create_job(&scope, new_job(5, JobSpec::BackupWorkspace)?)
        .await?;
    assert_eq!(again.id, first.id);
    assert_eq!(again.kind, first.kind);
    assert_eq!(first.workspace_id, Some(crate::scope()?.workspace_id));
    let listed = records
        .list_jobs(
            &scope,
            Page {
                cursor: None,
                limit: 10,
            },
        )
        .await?;
    assert_eq!(listed.items.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_job_that_does_not_fit_its_scope_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let error = err_of(
        records
            .create_job(
                &workspace_scope()?,
                new_job(5, JobSpec::BackupInstallation)?,
            )
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal);
    let tenant_job = records
        .create_job(
            &JobScope::Tenant(tenant()?),
            new_job(6, JobSpec::BackupInstallation)?,
        )
        .await?;
    assert_eq!(tenant_job.workspace_id, None);
    let error = err_of(records.get_job(&workspace_scope()?, tenant_job.id).await)?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_claim_returns_the_specification_initiator_and_mutation_unchanged() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let spec = JobSpec::ExportWorkspace {
        revision: revision('a')?,
        include_history: true,
    };
    let job = records
        .create_job(&workspace_scope()?, new_job(5, spec.clone())?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    assert_eq!(claimed.spec, spec);
    assert_eq!(claimed.initiator, owner());
    assert_eq!(claimed.mutation_id, uuid!(5)?);
    assert_eq!(claimed.scope, workspace_scope()?);
    assert_eq!(claimed.job.state, JobState::Running);
    assert_eq!(claimed.lease.attempt, 1);
    assert!(
        records.claim_job(job.id).await?.is_none(),
        "a live claim is not claimed twice"
    );
    Ok(())
}

#[tokio::test]
async fn only_the_current_unexpired_claim_completes_a_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let job = records
        .create_job(&workspace_scope()?, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let first = some(records.claim_job(job.id).await?, "the first claim")?;
    expire_job_leases(&storage)?;
    let error = err_of(records.complete_job(completion(first.lease.clone())).await)?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "an expired claim cannot complete"
    );
    assert_eq!(records.expire_leases().await?, 1);
    let second = some(records.claim_job(job.id).await?, "the second claim")?;
    assert_eq!(second.lease.attempt, 2);
    assert_ne!(second.lease.token, first.lease.token);
    let error = err_of(records.update_progress(&first.lease, 10).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    let progressed = records.update_progress(&second.lease, 40).await?;
    assert_eq!(progressed.progress, 40);
    let done = records.complete_job(completion(second.lease)).await?;
    assert_eq!(done.state, JobState::Succeeded);
    assert_eq!(done.progress, 100);
    assert_eq!(records.pending_jobs().await?.len(), 0);
    Ok(())
}

#[tokio::test]
async fn a_lapsed_claim_neither_renews_nor_fails_its_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    // The lease lapsed; no other worker has reclaimed the job yet.
    expire_job_leases(&storage)?;
    let renewed = err_of(records.update_progress(&claimed.lease, 30).await)?;
    assert_eq!(
        renewed.code,
        ErrorCode::Conflict,
        "a lapsed claim cannot renew"
    );
    let failed = err_of(
        records
            .fail_job(
                claimed.lease.clone(),
                ApiError::new(ErrorCode::Internal, "stalled"),
                false,
            )
            .await,
    )?;
    assert_eq!(
        failed.code,
        ErrorCode::Conflict,
        "a lapsed claim cannot fail"
    );
    assert_eq!(
        records.expire_leases().await?,
        1,
        "the job is still reclaimable, neither resurrected nor failed"
    );
    let again = some(records.claim_job(job.id).await?, "the reclaim")?;
    assert_eq!(again.lease.attempt, 2);
    Ok(())
}

#[tokio::test]
async fn a_cancelled_job_is_seen_by_the_heartbeat() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    let cancelled = records.cancel_job(&scope, uuid!(9)?, job.id).await?;
    assert_eq!(cancelled.state, JobState::Cancelled);
    let seen = records.update_progress(&claimed.lease, 50).await?;
    assert_eq!(seen.state, JobState::Cancelled);
    let error = err_of(records.complete_job(completion(claimed.lease)).await)?;
    assert_eq!(error.code, ErrorCode::Conflict);
    // The same cancel request again makes no second transition.
    let again = records.cancel_job(&scope, uuid!(9)?, job.id).await?;
    assert_eq!(again.state, JobState::Cancelled);
    let error = err_of(records.cancel_job(&scope, uuid!(10)?, job.id).await)?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "a cancelled job is not cancelled again"
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_job_is_retried_once_per_mutation_and_only_when_retryable() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    let failed = records
        .fail_job(
            claimed.lease,
            ApiError::new(ErrorCode::Unavailable, "the index file was busy"),
            true,
        )
        .await?;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(
        some(failed.error, "the failure")?.code,
        ErrorCode::Unavailable
    );
    assert_eq!(failed.retryable, Some(true));
    let queued = records.retry_job(&scope, uuid!(9)?, job.id).await?;
    assert_eq!(queued.state, JobState::Queued);
    let claimed = some(records.claim_job(job.id).await?, "the second claim")?;
    records
        .fail_job(
            claimed.lease,
            ApiError::new(ErrorCode::Internal, "broken"),
            false,
        )
        .await?;
    // Repeating the first retry request does not queue the job a second time.
    let repeated = records.retry_job(&scope, uuid!(9)?, job.id).await?;
    assert_eq!(repeated.state, JobState::Failed);
    let error = err_of(records.retry_job(&scope, uuid!(10)?, job.id).await)?;
    assert_eq!(
        error.code,
        ErrorCode::Conflict,
        "a non-retryable failure is not retried"
    );
    Ok(())
}

/// A job's failure is stored as its handler gave it, and its instants and retry flag follow
/// its state: created when accepted, started at each claim, finished when it succeeds, fails
/// or is cancelled, `retryable` exactly while it is failed, and a retry or a released lease
/// clearing what a waiting job does not have.
#[tokio::test]
async fn a_jobs_failure_instants_and_retry_flag_follow_its_state() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let created = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let accepted = some(created.created_at.clone(), "created_at")?;
    assert!(created.started_at.is_none() && created.finished_at.is_none());
    assert!(created.retryable.is_none());
    let claimed = some(records.claim_job(created.id).await?, "a claim")?;
    let started = some(claimed.job.started_at.clone(), "started_at")?;
    assert!(started >= accepted);
    assert!(claimed.job.finished_at.is_none());
    // The refusal is kept as given: code, message, field and detail.
    let head = Revision::try_from("c".repeat(40))?;
    let refusal = ApiError::new(ErrorCode::Conflict, "the card path is taken")
        .with_field("/upload_ids/1")
        .with_detail(ErrorDetail::StaleBase { head });
    let failed = records
        .fail_job(claimed.lease, refusal.clone(), true)
        .await?;
    let stored = records.get_job(&scope, created.id).await?;
    for job in [&failed, &stored] {
        assert_eq!(job.state, JobState::Failed);
        let error = some(job.error.clone(), "the failure")?;
        assert_eq!(
            serde_json::to_value(&error)?,
            serde_json::to_value(&refusal)?
        );
        assert_eq!(job.retryable, Some(true));
        assert!(some(job.finished_at.clone(), "finished_at")? >= started);
        assert_eq!(job.created_at.as_ref(), Some(&accepted));
    }
    // A retry queues it again: no start, no finish, no retry flag; acceptance kept.
    let queued = records.retry_job(&scope, uuid!(9)?, created.id).await?;
    assert_eq!(queued.state, JobState::Queued);
    assert!(queued.started_at.is_none() && queued.finished_at.is_none());
    assert!(queued.retryable.is_none() && queued.error.is_none());
    assert_eq!(queued.created_at.as_ref(), Some(&accepted));
    let claimed = some(records.claim_job(created.id).await?, "the second claim")?;
    assert!(claimed.job.started_at.is_some());
    let report = BackupReport { rows_left_out: 2 };
    let done = records
        .complete_job(JobCompletion {
            backup: Some(report.clone()),
            ..completion(claimed.lease)
        })
        .await?;
    assert_eq!(done.state, JobState::Succeeded);
    assert!(done.finished_at.is_some() && done.retryable.is_none());
    // The completion's report is kept on the job.
    assert_eq!(done.backup.as_ref(), Some(&report));
    let stored = records.get_job(&scope, created.id).await?;
    assert_eq!(stored.finished_at, done.finished_at);
    assert_eq!(stored.backup, Some(report));
    // A released lease returns the job to waiting without its start.
    let lapsed = records
        .create_job(&scope, new_job(6, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(lapsed.id).await?, "a claim")?;
    assert!(claimed.job.started_at.is_some());
    expire_job_leases(&storage)?;
    assert_eq!(records.expire_leases().await?, 1);
    let waiting = records.get_job(&scope, lapsed.id).await?;
    assert_eq!(waiting.state, JobState::Queued);
    assert!(waiting.started_at.is_none(), "{:?}", waiting.started_at);
    // A cancelled job is finished, and carries no retry flag: only a failure does (review N4).
    let cancelled = records.cancel_job(&scope, uuid!(10)?, lapsed.id).await?;
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert!(cancelled.finished_at.is_some());
    assert!(cancelled.retryable.is_none(), "{:?}", cancelled.retryable);
    let stored = records.get_job(&scope, lapsed.id).await?;
    assert!(stored.retryable.is_none(), "{:?}", stored.retryable);
    Ok(())
}

#[tokio::test]
async fn an_artifact_is_recorded_once_and_shown_on_its_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::BackupWorkspace)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    let artifact = NewArtifact {
        kind: ArtifactKind::WorkspaceBackup,
        object: ObjectInfo {
            digest: digest('b')?,
            size: 42,
        },
        media_type: "application/zip".to_owned(),
        created_by_job: job.id,
    };
    let recorded = records
        .record_artifact(&scope, uuid!(5)?, artifact.clone())
        .await?;
    let repeated = records
        .record_artifact(&scope, uuid!(5)?, artifact.clone())
        .await?;
    assert_eq!(repeated, recorded);
    let tenant_kind = NewArtifact {
        kind: ArtifactKind::InstallationBackup,
        ..artifact
    };
    let error = err_of(
        records
            .record_artifact(&scope, uuid!(6)?, tenant_kind)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::Internal);
    let error = err_of(
        records
            .get_artifact(&JobScope::Tenant(tenant()?), recorded.id)
            .await,
    )?;
    assert_eq!(error.code, ErrorCode::NotFound);
    let mut done = completion(claimed.lease);
    done.artifact = Some(recorded.id);
    let job = records.complete_job(done).await?;
    let shown = some(job.artifact, "the artifact on the job")?;
    assert_eq!(shown.artifact_id, recorded.id);
    assert_eq!(shown.size, "42");
    assert_eq!(
        shown.download_path,
        format!(
            "/api/workspaces/{}/artifacts/{}",
            crate::scope()?.workspace_id.0,
            recorded.id.0
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_completion_attaches_only_an_artifact_its_own_job_created() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let other = records
        .create_job(&scope, new_job(4, JobSpec::BackupWorkspace)?)
        .await?;
    let foreign = records
        .record_artifact(
            &scope,
            uuid!(6)?,
            NewArtifact {
                kind: ArtifactKind::WorkspaceBackup,
                object: ObjectInfo {
                    digest: digest('c')?,
                    size: 7,
                },
                media_type: "application/zip".to_owned(),
                created_by_job: other.id,
            },
        )
        .await?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::BackupWorkspace)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    let mut done = completion(claimed.lease.clone());
    done.artifact = Some(foreign.id);
    let error = err_of(records.complete_job(done).await)?;
    assert_eq!(error.code, ErrorCode::Internal, "{}", error.message);
    let unchanged = records.get_job(&scope, job.id).await?;
    assert_eq!(unchanged.state, JobState::Running);
    assert!(
        unchanged.artifact.is_none(),
        "another job's artifact is not attached"
    );
    Ok(())
}

#[tokio::test]
async fn a_removal_record_is_unique_while_unfinished_and_carries_a_tenant_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let target = PurgeTarget::Workspace {
        workspace_id: uuid!(1)?,
    };
    let purge = NewPurge {
        id: uuid!(20)?,
        target: target.clone(),
        initiator: owner(),
    };
    let first = records
        .create_purge(&tenant()?, uuid!(21)?, purge.clone())
        .await?;
    assert_eq!(first.state, PurgeState::Requested);
    let job = records
        .get_job(&JobScope::Tenant(tenant()?), first.job_id)
        .await?;
    assert_eq!(job.workspace_id, None);
    let other = NewPurge {
        id: uuid!(22)?,
        ..purge.clone()
    };
    let resumed = records
        .create_purge(&tenant()?, uuid!(23)?, other.clone())
        .await?;
    assert_eq!(
        resumed.id, first.id,
        "the unfinished purge of the target is returned"
    );
    let mut completed = first.clone();
    completed.state = PurgeState::Completed;
    records.update_purge(&tenant()?, completed).await?;
    let next = records.create_purge(&tenant()?, uuid!(23)?, other).await?;
    assert_eq!(next.id, uuid!(22)?);
    assert_eq!(
        records.get_purge(&tenant()?, first.id).await?.state,
        PurgeState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn a_removal_record_is_stamped_once_when_its_completion_is_recorded() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let first = records
        .create_purge(
            &tenant()?,
            uuid!(21)?,
            NewPurge {
                id: uuid!(20)?,
                target: PurgeTarget::Workspace {
                    workspace_id: uuid!(1)?,
                },
                initiator: owner(),
            },
        )
        .await?;
    assert_eq!(first.completed_at, None);
    let mut running = first.clone();
    running.state = PurgeState::Running;
    running.completed_at = Some(okf_jawn_contract::identity::Timestamp::try_from(
        "2020-01-01T00:00:00.000Z".to_owned(),
    )?);
    let stored = records.update_purge(&tenant()?, running).await?;
    assert_eq!(
        stored.completed_at, None,
        "a purge that has not completed carries no completion time"
    );
    let mut completed = first.clone();
    completed.state = PurgeState::Completed;
    let stamped = records.update_purge(&tenant()?, completed.clone()).await?;
    let at = some(stamped.completed_at.clone(), "the completion time")?;
    assert!(at >= first.requested_at, "stamped from the store's clock");
    // A later clock reading differs, so a re-stamp would show.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let again = records.update_purge(&tenant()?, completed).await?;
    assert_eq!(
        again.completed_at,
        Some(at.clone()),
        "recording the completion again keeps the first stamp"
    );
    assert_eq!(
        records.get_purge(&tenant()?, first.id).await?.completed_at,
        Some(at)
    );
    Ok(())
}

#[tokio::test]
async fn derived_objects_are_unique_per_item_revision_and_digest() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let object = DerivedObject {
        item_id: uuid!(3)?,
        revision: revision('c')?,
        digest: digest('d')?,
        kind: DerivedKind::Dataset,
        media_type: "application/json".to_owned(),
    };
    let first = records
        .record_derived_object(&scope()?, object.clone())
        .await?;
    let again = records
        .record_derived_object(
            &scope()?,
            DerivedObject {
                media_type: "text/plain".to_owned(),
                ..object.clone()
            },
        )
        .await?;
    assert_eq!(again, first);
    let found = records
        .derived_object(&scope()?, object.item_id, &object.revision, &object.digest)
        .await?;
    assert_eq!(found, Some(first));
    assert_eq!(
        records
            .revision_mapping(&scope()?, &object.revision)
            .await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn reviews_and_write_receipts_are_unique_on_their_mutation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let source = json!({
        "workspace_id": uuid_text(1),
        "item_id": uuid_text(3),
        "path": "notes/a.md",
        "revision": revision('a')?,
        "selection": { "kind": "all" }
    });
    let review = |id: u32| -> Fallible<okf_jawn_contract::review::Review> {
        Ok(serde_json::from_value(json!({
            "id": uuid_text(id),
            "source": source,
            "content_digest": digest('e')?,
            "reviewer_subject": "owner",
            "reviewed_at": "2026-10-09T12:00:00.000Z",
            "coverage": "current"
        }))?)
    };
    let first = records
        .insert_review(&scope()?, uuid!(30)?, review(40)?)
        .await?;
    let again = records
        .insert_review(&scope()?, uuid!(30)?, review(41)?)
        .await?;
    assert_eq!(again.id, first.id);
    let listed = records.list_reviews(&scope()?, uuid!(3)?).await?;
    assert_eq!(listed.len(), 1);
    let receipt = |id: u32| -> Fallible<okf_jawn_contract::events::Receipt> {
        Ok(serde_json::from_value(json!({
            "id": uuid_text(id),
            "workspace_id": uuid_text(1),
            "operation_id": "get_item",
            "principal_subject": "owner",
            "route": "local_owner",
            "sources": [source],
            "returned_at": "2026-10-09T12:00:00.000Z",
            "audience": "human_display"
        }))?)
    };
    let written = records
        .insert_receipt(&scope()?, Some(uuid!(31)?), receipt(50)?)
        .await?;
    let replayed = records
        .insert_receipt(&scope()?, Some(uuid!(31)?), receipt(51)?)
        .await?;
    assert_eq!(replayed.id, written.id);
    records
        .insert_receipt(&scope()?, None, receipt(52)?)
        .await?;
    records
        .insert_receipt(&scope()?, None, receipt(53)?)
        .await?;
    assert_eq!(
        records.get_receipt(&scope()?, uuid!(53)?).await?.id,
        uuid!(53)?
    );
    let error = err_of(records.get_receipt(&scope()?, uuid!(51)?).await)?;
    assert_eq!(error.code, ErrorCode::NotFound);
    Ok(())
}

/// Whether the job's row is sealed, read from its column.
fn sealed(storage: &Storage, job: okf_jawn_contract::identity::JobId) -> Fallible<bool> {
    let connection = Connection::open(storage.data().records_path())?;
    Ok(connection.query_row(
        "SELECT sealed FROM jobs WHERE job_id = ?1",
        [job.0.to_string()],
        |row| row.get(0),
    )?)
}

/// How many cancellations and retries were recorded.
fn controls(storage: &Storage) -> Fallible<i64> {
    let connection = Connection::open(storage.data().records_path())?;
    Ok(connection.query_row("SELECT count(*) FROM job_controls", [], |row| row.get(0))?)
}

/// R-I11: a cancellation before the seal takes effect, and the seal then returns the job as
/// it is, cancelled and unsealed, so its handler stops.
#[tokio::test]
async fn a_job_cancelled_before_its_seal_stays_cancelled_and_unsealed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    let cancelled = records.cancel_job(&scope, uuid!(9)?, job.id).await?;
    assert_eq!(cancelled.state, JobState::Cancelled);
    let seen = records.seal(&claimed.lease).await?;
    assert_eq!(seen.state, JobState::Cancelled);
    assert!(!sealed(&storage, job.id)?);
    Ok(())
}

/// R-I11: once a running job sealed itself, its cancellation is refused with `Conflict`,
/// recording nothing, while it runs and once its lease lapsed and it is queued again; it keeps
/// its seal through the reclaim and completes. Sealing again changes nothing.
#[tokio::test]
async fn a_sealed_job_refuses_its_cancellation_until_it_finishes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    assert!(!sealed(&storage, job.id)?, "a claim alone does not seal");
    assert_eq!(records.seal(&claimed.lease).await?.state, JobState::Running);
    assert_eq!(records.seal(&claimed.lease).await?.state, JobState::Running);
    assert!(sealed(&storage, job.id)?);
    let refused = err_of(records.cancel_job(&scope, uuid!(9)?, job.id).await)?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains("point of no return"),
        "{}",
        refused.message
    );
    assert_eq!(
        controls(&storage)?,
        0,
        "a refused cancellation records nothing"
    );
    assert_eq!(
        records.update_progress(&claimed.lease, 30).await?.state,
        JobState::Running,
        "the heartbeat still sees it running"
    );
    // Its attempt dies: the lease lapses and the job is queued again, still sealed.
    expire_job_leases(&storage)?;
    assert_eq!(records.expire_leases().await?, 1);
    let queued = err_of(records.cancel_job(&scope, uuid!(10)?, job.id).await)?;
    assert_eq!(queued.code, ErrorCode::Conflict, "{}", queued.message);
    assert!(queued.message.contains("point of no return"));
    let again = some(records.claim_job(job.id).await?, "the reclaim")?;
    assert!(sealed(&storage, job.id)?);
    let done = records.complete_job(completion(again.lease)).await?;
    assert_eq!(done.state, JobState::Succeeded);
    // Finished: refused as any finished job is.
    let finished = err_of(records.cancel_job(&scope, uuid!(11)?, job.id).await)?;
    assert!(
        finished.message.contains("cannot be cancelled"),
        "{}",
        finished.message
    );
    Ok(())
}

/// R-I11: a sealed job that failed and is retried keeps its seal: its next attempt resumes the
/// same work, and its cancellation is still refused.
#[tokio::test]
async fn a_retried_sealed_job_is_still_sealed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let claimed = some(records.claim_job(job.id).await?, "a claim")?;
    records.seal(&claimed.lease).await?;
    records
        .fail_job(
            claimed.lease,
            ApiError::new(ErrorCode::Unavailable, "the disk was busy"),
            true,
        )
        .await?;
    assert_eq!(
        records.retry_job(&scope, uuid!(9)?, job.id).await?.state,
        JobState::Queued
    );
    let refused = err_of(records.cancel_job(&scope, uuid!(10)?, job.id).await)?;
    assert!(refused.message.contains("point of no return"));
    Ok(())
}

/// Only the job's current, unexpired claim of a running job seals it: a lapsed claim and a
/// superseded token are refused with `Conflict`, and nothing is sealed.
#[tokio::test]
async fn only_the_current_unexpired_claim_seals_a_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let scope = workspace_scope()?;
    let job = records
        .create_job(&scope, new_job(5, JobSpec::RebuildIndex)?)
        .await?;
    let first = some(records.claim_job(job.id).await?, "a claim")?;
    expire_job_leases(&storage)?;
    let lapsed = err_of(records.seal(&first.lease).await)?;
    assert_eq!(lapsed.code, ErrorCode::Conflict);
    assert!(!sealed(&storage, job.id)?);
    assert_eq!(records.expire_leases().await?, 1);
    let second = some(records.claim_job(job.id).await?, "the reclaim")?;
    let superseded = err_of(records.seal(&first.lease).await)?;
    assert_eq!(superseded.code, ErrorCode::Conflict);
    assert!(!sealed(&storage, job.id)?);
    records.seal(&second.lease).await?;
    assert!(sealed(&storage, job.id)?);
    Ok(())
}

/// Records schema 4 adds the seal: a records database at schema 3 is backed up and then
/// migrated, and every job it held is unsealed (before this step nothing sealed a job).
#[tokio::test]
async fn a_records_database_at_schema_3_migrates_its_jobs_unsealed_after_a_backup() -> TestResult {
    let directory = tempfile::tempdir()?;
    let job = {
        let storage = Storage::open(directory.path())?;
        let job = storage
            .records()
            .create_job(&workspace_scope()?, new_job(5, JobSpec::RebuildIndex)?)
            .await?;
        job.id
    };
    let records = directory.path().join("records.sqlite");
    Connection::open(&records)?
        .execute_batch("ALTER TABLE jobs DROP COLUMN sealed; PRAGMA user_version = 3;")?;
    let storage = Storage::open(directory.path())?;
    assert!(!sealed(&storage, job)?);
    let backups: Vec<String> = std::fs::read_dir(storage.data().pre_migration_path())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("records-v3-"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    let version: i64 =
        Connection::open(&records)?.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(version, 4);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
