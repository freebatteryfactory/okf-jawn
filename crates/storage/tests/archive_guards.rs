//! The two store guards of an archived workspace (contract ruling 25), against the real records
//! database: an archive is refused while an import, redigest or restore job of the workspace is
//! queued or running, and no such job is inserted for an archived workspace; an archive and an
//! import's insert racing end with exactly one of them refused.
#![cfg(feature = "runtime")]

use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::ConversionSettings;
use okf_jawn_contract::identity::{MutationId, Revision, TenantId, UploadId};
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::jobs::{JobCompletion, JobScope, JobSpec, NewJob, RecordStore};
use okf_jawn_core::storage::{
    NewWorkspace, Provenance, StorageScope, WorkspaceArchive, WorkspaceCatalog,
};
use okf_jawn_storage::Storage;
use serde_json::json;

use check::{TestResult, err_of, some};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// How many times the race runs.
const ROUNDS: u32 = 40;

fn mutation(n: u32) -> Fallible<MutationId> {
    Ok(serde_json::from_value(json!(format!(
        "00000000-0000-4000-8000-{n:012}"
    )))?)
}

fn owner() -> Provenance {
    Provenance {
        subject: "owner".to_owned(),
        route: okf_jawn_contract::access::AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::try_from("local".to_owned())?)
}

/// A new workspace created under mutation `n`, and its scope.
async fn create(storage: &Storage, n: u32) -> Fallible<(Workspace, StorageScope)> {
    let created = storage
        .catalog()
        .create(
            &tenant()?,
            mutation(n)?,
            NewWorkspace {
                name: format!("workspace {n}"),
                description: String::new(),
                creator: owner(),
            },
        )
        .await?;
    let scope = StorageScope {
        tenant_id: tenant()?,
        workspace_id: created.id,
    };
    Ok((created, scope))
}

/// Every job kind, by a specification of it at `base`.
fn specs(base: &Revision) -> Fallible<Vec<(bool, JobSpec)>> {
    let item = serde_json::from_value(json!("00000000-0000-4000-8000-000000000099"))?;
    let upload: UploadId = serde_json::from_value(json!("00000000-0000-4000-8000-000000000098"))?;
    let digest = okf_jawn_contract::identity::Digest::try_from("d".repeat(64))?;
    // Whether the kind changes the workspace's content: an import, a redigest, a restore.
    Ok(vec![
        (
            true,
            JobSpec::Import {
                base_revision: base.clone(),
                upload_ids: vec![upload],
                destination: None,
                apply_naming_rules: false,
                settings: ConversionSettings::default(),
            },
        ),
        (
            true,
            JobSpec::Redigest {
                item_id: item,
                base_revision: base.clone(),
                settings: ConversionSettings::default(),
                pages: None,
            },
        ),
        (
            true,
            JobSpec::RestoreWorkspace {
                upload_id: upload,
                archive: digest,
                editors: vec!["owner".to_owned()],
            },
        ),
        (
            false,
            JobSpec::ExportWorkspace {
                revision: base.clone(),
                include_history: false,
            },
        ),
        (false, JobSpec::BackupWorkspace),
        (false, JobSpec::RebuildIndex),
        (
            false,
            JobSpec::ExportView {
                item_id: item,
                revision: base.clone(),
            },
        ),
    ])
}

async fn archive(
    storage: &Storage,
    scope: &StorageScope,
    head: &Revision,
    n: u32,
) -> Result<Workspace, ApiError> {
    storage
        .catalog()
        .archive(
            scope,
            mutation(n).map_err(|error| ApiError::new(ErrorCode::Internal, error.to_string()))?,
            WorkspaceArchive {
                expected_head: head.clone(),
                author: owner(),
            },
        )
        .await
}

fn job(n: u32, spec: JobSpec) -> Fallible<NewJob> {
    Ok(NewJob {
        mutation_id: mutation(n)?,
        initiator: owner(),
        spec,
    })
}

#[tokio::test]
async fn an_archive_is_refused_while_a_content_job_is_queued_or_running_and_only_then() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let (workspace, _) = create(&storage, 1).await?;
    for (index, (changes_content, spec)) in specs(&workspace.head)?.into_iter().enumerate() {
        let n = u32::try_from(index)?.saturating_mul(10).saturating_add(100);
        let (created, scope) = create(&storage, n).await?;
        let queued = records
            .create_job(
                &JobScope::Workspace(scope.clone()),
                job(n.saturating_add(1), spec.clone())?,
            )
            .await?;
        let kind = format!("{:?}", spec.kind());
        let refused = archive(&storage, &scope, &created.head, n.saturating_add(2)).await;
        if changes_content {
            let refused = err_of(refused)?;
            assert_eq!(refused.code, ErrorCode::Conflict, "{kind} queued");
            assert!(
                refused.message.contains(&queued.id.0.to_string()),
                "names the job"
            );
            assert_eq!(
                storage.catalog().open(&scope).await?.archived_at,
                None,
                "{kind}: nothing archived"
            );
            // Running holds the archive off too.
            let claimed = some(records.claim_job(queued.id).await?, "the claim")?;
            let refused =
                err_of(archive(&storage, &scope, &created.head, n.saturating_add(3)).await)?;
            assert_eq!(refused.code, ErrorCode::Conflict, "{kind} running");
            // Once it ends, the workspace is archived.
            records
                .complete_job(JobCompletion {
                    lease: claimed.lease,
                    revision: None,
                    item_ids: Vec::new(),
                    artifact: None,
                    restore: None,
                    backup: None,
                    outputs: Vec::new(),
                    warnings: Vec::new(),
                })
                .await?;
            let archived = archive(&storage, &scope, &created.head, n.saturating_add(4)).await?;
            assert!(archived.archived_at.is_some(), "{kind} done");
        } else {
            let archived = refused?;
            assert!(
                archived.archived_at.is_some(),
                "{kind} does not change content"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_cancelled_content_job_lets_the_workspace_be_archived() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let (created, scope) = create(&storage, 1).await?;
    let (_, import) = some(specs(&created.head)?.into_iter().next(), "the import")?;
    let queued = records
        .create_job(&JobScope::Workspace(scope.clone()), job(2, import)?)
        .await?;
    records
        .cancel_job(&JobScope::Workspace(scope.clone()), mutation(3)?, queued.id)
        .await?;
    let archived = archive(&storage, &scope, &created.head, 4).await?;
    assert!(archived.archived_at.is_some());
    Ok(())
}

#[tokio::test]
async fn an_archived_workspace_accepts_no_import_redigest_or_restore_job() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let (created, scope) = create(&storage, 1).await?;
    let archived = archive(&storage, &scope, &created.head, 2).await?;
    let at = some(archived.archived_at.clone(), "the archive instant")?;
    for (index, (changes_content, spec)) in specs(&created.head)?.into_iter().enumerate() {
        let n = u32::try_from(index)?.saturating_add(10);
        let kind = format!("{:?}", spec.kind());
        let outcome = records
            .create_job(&JobScope::Workspace(scope.clone()), job(n, spec)?)
            .await;
        if changes_content {
            let refused = err_of(outcome)?;
            assert_eq!(refused.code, ErrorCode::Conflict, "{kind}");
            assert!(
                matches!(
                    refused.detail.as_deref(),
                    Some(ErrorDetail::WorkspaceArchived { archived_at }) if *archived_at == at
                ),
                "{kind}: {refused:?}"
            );
        } else {
            let accepted = outcome?;
            assert_eq!(accepted.workspace_id, Some(scope.workspace_id), "{kind}");
        }
    }
    // Unarchived, the workspace takes an import again.
    storage
        .catalog()
        .unarchive(&scope, mutation(30)?, owner())
        .await?;
    let (_, import) = some(specs(&created.head)?.into_iter().next(), "the import")?;
    records
        .create_job(&JobScope::Workspace(scope.clone()), job(31, import)?)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_repeated_job_mutation_is_refused_too_once_the_workspace_is_archived() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let (created, scope) = create(&storage, 1).await?;
    let (_, import) = some(specs(&created.head)?.into_iter().next(), "the import")?;
    let first = records
        .create_job(&JobScope::Workspace(scope.clone()), job(2, import.clone())?)
        .await?;
    records
        .cancel_job(&JobScope::Workspace(scope.clone()), mutation(3)?, first.id)
        .await?;
    archive(&storage, &scope, &created.head, 4).await?;
    let refused = err_of(
        records
            .create_job(&JobScope::Workspace(scope.clone()), job(2, import)?)
            .await,
    )?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    Ok(())
}

/// A failed content job holds no archive off (it is neither queued nor running), so a retry
/// could queue it again after the workspace was archived; the retry's own transaction refuses
/// that, as an insert's does, and the job stays failed. A job that does not change content is
/// queued again (dispatch, not the store, refuses `retry_job` in an archived workspace).
#[tokio::test]
async fn a_failed_content_job_is_not_queued_again_once_the_workspace_is_archived() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let records = storage.records();
    let (workspace, _) = create(&storage, 1).await?;
    for (index, (changes_content, spec)) in specs(&workspace.head)?.into_iter().enumerate() {
        let n = u32::try_from(index)?.saturating_mul(10).saturating_add(200);
        let (created, scope) = create(&storage, n).await?;
        let jobs = JobScope::Workspace(scope.clone());
        let kind = format!("{:?}", spec.kind());
        let queued = records
            .create_job(&jobs, job(n.saturating_add(1), spec)?)
            .await?;
        let claimed = some(records.claim_job(queued.id).await?, "the claim")?;
        records
            .fail_job(
                claimed.lease,
                ApiError::new(ErrorCode::Unavailable, "the converter stopped"),
                true,
            )
            .await?;
        let archived = archive(&storage, &scope, &created.head, n.saturating_add(2)).await?;
        let at = some(archived.archived_at.clone(), "the archive instant")?;
        let retried = records
            .retry_job(&jobs, mutation(n.saturating_add(3))?, queued.id)
            .await;
        if changes_content {
            let refused = err_of(retried)?;
            assert_eq!(refused.code, ErrorCode::Conflict, "{kind}");
            assert!(
                matches!(
                    refused.detail.as_deref(),
                    Some(ErrorDetail::WorkspaceArchived { archived_at }) if *archived_at == at
                ),
                "{kind}: {refused:?}"
            );
            let kept = records.get_job(&jobs, queued.id).await?;
            assert_eq!(
                kept.state,
                okf_jawn_contract::import::JobState::Failed,
                "{kind}: still failed"
            );
        } else {
            let again = retried?;
            assert_eq!(
                again.state,
                okf_jawn_contract::import::JobState::Queued,
                "{kind}: queued again"
            );
        }
    }
    Ok(())
}

/// A smoke test: whichever side the scheduler lets win a round, exactly one of the two is
/// refused, and the winner is what the store holds. It does not show that both orders occur
/// (one side may win every round); each order is driven explicitly, with its outcome, by the
/// crate's unit tests of the catalog (`catalog::tests`, an archive first and an insert first).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_archive_and_an_import_racing_end_with_exactly_one_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(directory.path())?;
    let mut archives_won = 0_u32;
    let mut imports_won = 0_u32;
    for round in 0..ROUNDS {
        let n = round.saturating_mul(10).saturating_add(1000);
        let (created, scope) = create(&storage, n).await?;
        let (_, import) = some(specs(&created.head)?.into_iter().next(), "the import")?;
        let catalog = storage.catalog();
        let records = storage.records();
        let archiving = {
            let scope = scope.clone();
            let head = created.head.clone();
            let archive_mutation = mutation(n.saturating_add(1))?;
            tokio::spawn(async move {
                catalog
                    .archive(
                        &scope,
                        archive_mutation,
                        WorkspaceArchive {
                            expected_head: head,
                            author: owner(),
                        },
                    )
                    .await
            })
        };
        let inserting = {
            let job_scope = JobScope::Workspace(scope.clone());
            let new_job = job(n.saturating_add(2), import)?;
            tokio::spawn(async move { records.create_job(&job_scope, new_job).await })
        };
        let (archived, inserted) = (archiving.await?, inserting.await?);
        match (archived, inserted) {
            (Ok(workspace), Err(refused)) => {
                assert!(workspace.archived_at.is_some());
                assert!(matches!(
                    refused.detail.as_deref(),
                    Some(ErrorDetail::WorkspaceArchived { .. })
                ));
                archives_won = archives_won.saturating_add(1);
            }
            (Err(refused), Ok(job)) => {
                assert_eq!(refused.code, ErrorCode::Conflict);
                assert!(refused.message.contains(&job.id.0.to_string()));
                assert_eq!(storage.catalog().open(&scope).await?.archived_at, None);
                imports_won = imports_won.saturating_add(1);
            }
            (archived, inserted) => {
                return Err(format!(
                    "round {round}: exactly one must win, got {archived:?} and {inserted:?}"
                )
                .into());
            }
        }
    }
    assert_eq!(archives_won.saturating_add(imports_won), ROUNDS);
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
