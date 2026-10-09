//! `RecordStore` against the real records database: jobs and their leases, artifacts, purge
//! records, derived objects, reviews and receipts.
#![cfg(feature = "runtime")]

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::identity::{Digest, Revision, TenantId};
use okf_jawn_contract::import::JobState;
use okf_jawn_contract::purge::{PurgeState, PurgeTarget};
use okf_jawn_contract::workspace::ArtifactKind;
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
        .fail_job(claimed.lease, "the index file was busy".to_owned(), true)
        .await?;
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(
        some(failed.error, "the failure")?.code,
        ErrorCode::Unavailable
    );
    let queued = records.retry_job(&scope, uuid!(9)?, job.id).await?;
    assert_eq!(queued.state, JobState::Queued);
    let claimed = some(records.claim_job(job.id).await?, "the second claim")?;
    records
        .fail_job(claimed.lease, "broken".to_owned(), false)
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

#[path = "../../../tests/support/check.rs"]
mod check;
