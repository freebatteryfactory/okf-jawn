//! Unit tests of the purge's object step against the in-flight pins, which are private: they
//! hold a pin of the target the way a write that began before the purge holds one. They return
//! `ApiError` like core's unit tests.

use std::time::{Duration, Instant};

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{Digest, MutationId, PurgeId, TenantId};
use okf_jawn_contract::purge::PurgeTarget;
use okf_jawn_core::jobs::{NewPurge, RecordStore};
use okf_jawn_core::storage::{
    BlobStore, NewWorkspace, Provenance, Purger, StorageScope, WorkspaceCatalog,
};
use rusqlite::OptionalExtension;

use crate::storage::Storage;

fn fault(message: &dyn std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, message.to_string())
}

fn person() -> Provenance {
    Provenance {
        subject: "ana".to_owned(),
        route: AccessRoute::LocalOwner,
        client_id: None,
    }
}

fn tenant() -> Result<TenantId, ApiError> {
    TenantId::try_from("local".to_owned()).map_err(|error| fault(&error))
}

fn mutation(n: u128) -> MutationId {
    MutationId(uuid::Uuid::from_u128(n))
}

fn purge_id() -> PurgeId {
    PurgeId(uuid::Uuid::from_u128(60))
}

async fn create(storage: &Storage, n: u128, name: &str) -> Result<StorageScope, ApiError> {
    let created = storage
        .catalog()
        .create(
            &tenant()?,
            mutation(n),
            NewWorkspace {
                name: name.to_owned(),
                description: String::new(),
                creator: person(),
            },
        )
        .await?;
    Ok(StorageScope {
        tenant_id: tenant()?,
        workspace_id: created.id,
    })
}

/// Record the purge of `target`, as core does before it calls the purger.
async fn record_purge(storage: &Storage, target: &StorageScope) -> Result<(), ApiError> {
    storage
        .records()
        .create_purge(
            &tenant()?,
            mutation(50),
            NewPurge {
                id: purge_id(),
                target: PurgeTarget::Workspace {
                    workspace_id: target.workspace_id,
                },
                initiator: person(),
            },
        )
        .await?;
    Ok(())
}

/// Wait until the purge has recorded `step` as done.
async fn until_step(storage: &Storage, step: i64) -> Result<(), ApiError> {
    let started = Instant::now();
    loop {
        let purge = purge_id().0.to_string();
        let found: Option<i64> = storage
            .records
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT step FROM purge_progress WHERE purge_id = ?1",
                        [purge],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| fault(&error))
            })
            .await?;
        if found.is_some_and(|found| found >= step) {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(60) {
            return Err(fault(&"the purge never reached the step"));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Whether the tenant's object store holds `digest`.
async fn held(storage: &Storage, scope: &StorageScope, digest: &Digest) -> Result<bool, ApiError> {
    match storage.blobs().open(scope, digest, 0, 1).await {
        Ok(_) => Ok(true),
        Err(error) if error.code == ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Ruling 2 (window 1): a write of the target that pinned before the purge was recorded and
/// stores its bytes only later (an upload completion still retaining) is waited for, and its
/// bytes go with the target once its record write has failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_object_step_waits_for_a_write_of_the_target_that_began_before_the_purge()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let target = create(&storage, 1, "Trip").await?;
    let late = b"zanzibar bytes a running write stores late";
    let digest = Digest::try_from(crate::seam::hex(&{
        use sha2::Digest as _;
        sha2::Sha256::digest(late)
    }))
    .map_err(|error| fault(&error))?;
    let (tenant_key, workspace_key) = crate::db::scope_key(&target);
    let running = storage
        .blobs
        .pins()
        .pin(&tenant_key, &workspace_key, &digest)?;
    record_purge(&storage, &target).await?;
    let purging = {
        let purger = storage.purger();
        let workspace = target.workspace_id;
        tokio::spawn(async move {
            purger
                .purge_workspace(&tenant()?, mutation(61), purge_id(), workspace)
                .await
        })
    };
    until_step(&storage, 5).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !purging.is_finished(),
        "the purge does not delete the target's objects while its write runs"
    );
    // The write retains its bytes, then its record is refused: the pin is abandoned.
    storage
        .blobs
        .put_for(
            &target.tenant_id,
            Box::pin(std::io::Cursor::new(late.to_vec())),
            1 << 20,
            None,
        )
        .await?;
    running.abandon()?;
    let report = purging.await.map_err(|error| fault(&error))??;
    assert!(
        !held(&storage, &target, &digest).await?,
        "the late bytes go with the target: {report:?}"
    );
    assert_eq!(report.objects_removed, "1");
    Ok(())
}
