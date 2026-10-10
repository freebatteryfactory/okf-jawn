//! Unit tests of the purge's object step against the in-flight pins, which are private: they
//! hold a pin of the target the way a write that began before the purge holds one, which makes
//! the step wait (`Sweep::drain`) after it read every surviving reference and before it
//! deletes anything. They return `ApiError` like core's unit tests.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::identity::{
    Digest, ItemId, MutationId, PurgeId, Revision, TenantId, WorkspacePath,
};
use okf_jawn_contract::purge::{PurgeReport, PurgeTarget};
use okf_jawn_core::jobs::{NewPurge, RecordStore};
use okf_jawn_core::storage::{
    Backups, BlobStore, CandidateCheck, CommitChanges, NewWorkspace, Provenance, Purger,
    SourceCard, StorageScope, TreeEdit, VersionStore, WorkspaceCatalog,
};
use okf_jawn_core::uploads::{NewUpload, UploadStore};
use rusqlite::OptionalExtension;
use serde_json::json;
use tokio::task::JoinHandle;

use crate::pins::Pinned;
use crate::storage::Storage;

/// Accepts every candidate.
struct Accept;

/// A thread holding a workspace's write lock, and the sender that makes it let go.
type Holder = (
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<Result<(), ApiError>>,
);

impl CandidateCheck for Accept {
    fn check(&self, _root: &std::path::Path) -> Result<Vec<Warning>, ApiError> {
        Ok(Vec::new())
    }
}

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

fn tenant(name: &str) -> Result<TenantId, ApiError> {
    TenantId::try_from(name.to_owned()).map_err(|error| fault(&error))
}

fn mutation(n: u128) -> MutationId {
    MutationId(uuid::Uuid::from_u128(n))
}

fn item(n: u128) -> ItemId {
    ItemId(uuid::Uuid::from_u128(n))
}

fn purge_id() -> PurgeId {
    PurgeId(uuid::Uuid::from_u128(60))
}

fn digest_of(bytes: &[u8]) -> Result<Digest, ApiError> {
    use sha2::Digest as _;
    Digest::try_from(crate::seam::hex(&sha2::Sha256::digest(bytes))).map_err(|error| fault(&error))
}

async fn create(
    storage: &Storage,
    tenant_name: &str,
    n: u128,
    name: &str,
) -> Result<(StorageScope, Revision), ApiError> {
    let created = storage
        .catalog()
        .create(
            &tenant(tenant_name)?,
            mutation(n),
            NewWorkspace {
                name: name.to_owned(),
                description: String::new(),
                creator: person(),
            },
        )
        .await?;
    let scope = StorageScope {
        tenant_id: tenant(tenant_name)?,
        workspace_id: created.id,
    };
    Ok((scope, created.head))
}

async fn put(storage: &Storage, scope: &StorageScope, bytes: &[u8]) -> Result<Digest, ApiError> {
    let object = storage
        .blobs()
        .put(
            scope,
            Box::pin(std::io::Cursor::new(bytes.to_vec())),
            1 << 20,
            None,
        )
        .await?;
    Ok(object.digest)
}

/// A source card for `object`.
fn card(item: ItemId, at: &str, object: &Digest) -> Result<TreeEdit, ApiError> {
    let appearance = serde_json::from_value(json!({
        "object": object.as_str(),
        "names": [{
            "filename": "scan.pdf",
            "folder": "",
            "observed_at": "2026-10-09T12:00:00.000Z",
            "supplied_by": "ana"
        }],
        "media_type": "application/pdf",
        "size": "10",
        "metadata": {},
        "extraction": {
            "outcome": { "status": "unsupported" },
            "text_origin": "none",
            "corrected": false,
            "warnings": []
        }
    }))
    .map_err(|error| fault(&error))?;
    Ok(TreeEdit::WriteSourceCard(Box::new(SourceCard {
        item_id: item,
        path: WorkspacePath::try_from(at.to_owned()).map_err(|error| fault(&error))?,
        title: "scan.pdf".to_owned(),
        type_name: "Source".to_owned(),
        body: String::new(),
        properties: BTreeMap::new(),
        appearance,
    })))
}

async fn commit(
    storage: &Storage,
    scope: &StorageScope,
    n: u128,
    head: &Revision,
    edits: Vec<TreeEdit>,
) -> Result<Revision, ApiError> {
    Ok(storage
        .versions()
        .commit(
            scope,
            CommitChanges {
                mutation_id: mutation(n),
                expected_head: head.clone(),
                author: person(),
                message: "Seed".to_owned(),
                edits,
            },
            Arc::new(Accept),
        )
        .await?
        .revision)
}

/// Record the purge of `target`, as core does before it calls the purger.
async fn record_purge(storage: &Storage, target: &StorageScope) -> Result<(), ApiError> {
    storage
        .records()
        .create_purge(
            &target.tenant_id,
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

/// Run the purge of `target` in the background.
fn spawn_purge(
    storage: &Storage,
    target: &StorageScope,
) -> JoinHandle<Result<PurgeReport, ApiError>> {
    let purger = storage.purger();
    let target = target.clone();
    tokio::spawn(async move {
        purger
            .purge_workspace(
                &target.tenant_id,
                mutation(61),
                purge_id(),
                target.workspace_id,
            )
            .await
    })
}

/// A pin of the target as a write that began before the purge holds one: the purge's object
/// step waits for it after it read every surviving reference. It names no stored object.
fn hold_the_object_step(storage: &Storage, target: &StorageScope) -> Result<Pinned, ApiError> {
    let (tenant, workspace) = crate::db::scope_key(target);
    storage
        .blobs
        .pins()
        .pin(&tenant, &workspace, &digest_of(b"no such object")?)
}

/// Wait until the purge's object step waits for the target's pins.
async fn until_held(storage: &Storage, target: &StorageScope) -> Result<(), ApiError> {
    let (tenant, workspace) = crate::db::scope_key(target);
    let started = Instant::now();
    while !storage.blobs.pins().draining(&tenant, &workspace)? {
        if started.elapsed() > Duration::from_secs(120) {
            return Err(fault(&"the object step never began to wait"));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
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

/// The target and a survivor of tenant `tenant_name`; the target's card names `shared`.
async fn target_with(
    storage: &Storage,
    tenant_name: &str,
    shared: &[u8],
) -> Result<(StorageScope, StorageScope, Revision, Digest), ApiError> {
    let (target, head) = create(storage, tenant_name, 1, "Trip").await?;
    let (survivor, survivor_head) = create(storage, tenant_name, 2, "Garden").await?;
    let digest = put(storage, &target, shared).await?;
    commit(
        storage,
        &target,
        3,
        &head,
        vec![card(item(11), "sources/a.md", &digest)?],
    )
    .await?;
    Ok((target, survivor, survivor_head, digest))
}

/// Ruling 2 (window 1): a write of the target that pinned before the purge was recorded and
/// stores its bytes only later (an upload completion still retaining) is waited for, and its
/// bytes go with the target once its record write has failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_object_step_waits_for_a_write_of_the_target_that_began_before_the_purge()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, "local", 1, "Trip").await?;
    let late = b"zanzibar bytes a running write stores late";
    let digest = digest_of(late)?;
    let (tenant_key, workspace_key) = crate::db::scope_key(&target);
    let running = storage
        .blobs
        .pins()
        .pin(&tenant_key, &workspace_key, &digest)?;
    record_purge(&storage, &target).await?;
    let purging = spawn_purge(&storage, &target);
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

/// Ruling 1 (window 2): a survivor's put that pins and releases entirely between the object
/// step's reference read and its delete (it stores an object identical to the target's and
/// commits the card naming it) does not lose its object.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_that_pins_and_releases_between_the_reference_read_and_the_delete_keeps_its_object()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let shared = b"commonplace identical bytes";
    let (target, survivor, survivor_head, digest) = target_with(&storage, "local", shared).await?;
    record_purge(&storage, &target).await?;
    let holding = hold_the_object_step(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    let stored = put(&storage, &survivor, shared).await?;
    commit(
        &storage,
        &survivor,
        4,
        &survivor_head,
        vec![card(item(12), "sources/b.md", &stored)?],
    )
    .await?;
    holding.release()?;
    let report = purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the object the survivor's new card names stays"
    );
    assert_eq!(report.objects_kept_shared, "1");
    Ok(())
}

/// Ruling 3, the brief's window-2 scenario exactly: another workspace's import stores an
/// object identical to one of the target's inside the object step, and its history and rows
/// land after the step read every surviving reference. The import pins each object until its
/// records are durable, so the object stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_of_another_workspace_inside_the_object_step_keeps_its_objects()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let shared = b"commonplace imported bytes";
    // The archive comes from a workspace of another tenant, whose objects are its own.
    let (source, source_head) = create(&storage, "local", 70, "Source").await?;
    let original = put(&storage, &source, shared).await?;
    commit(
        &storage,
        &source,
        71,
        &source_head,
        vec![card(item(13), "sources/c.md", &original)?],
    )
    .await?;
    let archive = storage
        .backups()
        .write_workspace_archive(&source, mutation(72))
        .await?;
    let bytes = std::fs::read(
        storage
            .blobs
            .local_path(&source.tenant_id, &archive.digest)?,
    )
    .map_err(|error| fault(&error))?;
    let (target, survivor, _, digest) = target_with(&storage, "elsewhere", shared).await?;
    let uploaded = storage
        .blobs()
        .put(
            &survivor,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    record_purge(&storage, &target).await?;
    let holding = hold_the_object_step(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    storage
        .backups()
        .restore_import(
            &survivor,
            uploaded.digest,
            mutation(73),
            vec!["ana".to_owned()],
        )
        .await?;
    holding.release()?;
    purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the object the import's card names stays"
    );
    let head = storage.versions().head(&survivor).await?;
    storage.versions().show(&survivor, &head, item(13)).await?;
    Ok(())
}

/// Ruling 3: another workspace's upload completion inside the object step stores an object
/// identical to one of the target's; it pins the object until its record is durable, so the
/// object stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_completion_of_another_workspace_inside_the_object_step_keeps_its_object()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let shared = b"commonplace uploaded bytes";
    let (target, survivor, _, digest) = target_with(&storage, "local", shared).await?;
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &survivor,
            mutation(80),
            NewUpload {
                filename: "scan.pdf".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(shared.len()).map_err(|error| fault(&error))?,
                expected_sha256: None,
                supplied_by: person(),
            },
        )
        .await?;
    uploads
        .put_content(
            &survivor,
            slot.id,
            Box::pin(std::io::Cursor::new(shared.to_vec())),
            1 << 20,
        )
        .await?;
    record_purge(&storage, &target).await?;
    let holding = hold_the_object_step(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    let completed = uploads.complete(&survivor, slot.id, digest.clone()).await?;
    assert!(completed.object.is_some(), "the upload's record landed");
    holding.release()?;
    purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the object the survivor's upload names stays"
    );
    Ok(())
}

/// Hold `scope`'s workspace write lock on a thread until `release` receives; returns once it
/// is held.
fn hold_the_workspace(storage: &Storage, scope: &StorageScope) -> Result<Holder, ApiError> {
    let (held, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
    let holder = {
        let repositories = storage.repositories.clone();
        let scope = scope.clone();
        std::thread::spawn(move || {
            repositories.locked(&scope, || {
                held.0.send(()).map_err(|error| fault(&error))?;
                release.1.recv().map_err(|error| fault(&error))
            })
        })
    };
    held.1.recv().map_err(|error| fault(&error))?;
    Ok((release.0, holder))
}

/// A Git write that passed the purge refusal before the purge was recorded, and waits for the
/// workspace's lock while it is recorded, is refused under the lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_waiting_for_the_workspace_lock_when_the_purge_is_recorded_is_refused()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, "local", 1, "Trip").await?;
    let (release, holder) = hold_the_workspace(&storage, &target)?;
    let committing = {
        let versions = storage.versions();
        let target = target.clone();
        let edits = vec![card(item(11), "sources/a.md", &digest_of(b"zanzibar")?)?];
        tokio::spawn(async move {
            versions
                .commit(
                    &target,
                    CommitChanges {
                        mutation_id: mutation(3),
                        expected_head: head,
                        author: person(),
                        message: "Late".to_owned(),
                        edits,
                    },
                    Arc::new(Accept),
                )
                .await
        })
    };
    // The commit passes the first check at once and then waits for the lock.
    tokio::time::sleep(Duration::from_millis(500)).await;
    record_purge(&storage, &target).await?;
    release.send(()).map_err(|error| fault(&error))?;
    holder
        .join()
        .map_err(|_| fault(&"the lock holder stopped"))??;
    let error = match committing.await.map_err(|error| fault(&error))? {
        Ok(committed) => return Err(fault(&format!("the commit landed: {committed:?}"))),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(error.message.contains("purged by purge"), "{error}");
    Ok(())
}

/// The plan reads the workspace under its write lock, so it waits for a Git write that holds
/// the lock (and may land a commit) to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_plan_waits_for_a_write_that_holds_the_workspace_lock() -> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, "local", 1, "Trip").await?;
    record_purge(&storage, &target).await?;
    let (release, holder) = hold_the_workspace(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let purge = purge_id().0.to_string();
    let planned: bool = storage
        .records
        .call(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM purge_progress WHERE purge_id = ?1)",
                    [purge],
                    |row| row.get(0),
                )
                .map_err(|error| fault(&error))
        })
        .await?;
    assert!(
        !planned,
        "no plan is recorded while a write holds the workspace"
    );
    release.send(()).map_err(|error| fault(&error))?;
    holder
        .join()
        .map_err(|_| fault(&"the lock holder stopped"))??;
    purging.await.map_err(|error| fault(&error))??;
    Ok(())
}
