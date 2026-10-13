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

/// The bytes of the target's large upload; each run of them carries the marker `zanzibar`.
const UPLOADED: &[u8] = b"zanzibar upload bytes ";

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
/// bytes, named by its pending put, go with the target once its record write has failed.
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
    // It passed its second refusal check, which recorded its pending put.
    {
        let digest = digest.clone();
        storage
            .records
            .transaction(move |transaction| {
                crate::puts::record(transaction, &tenant_key, &workspace_key, &digest)
            })
            .await?;
    }
    record_purge(&storage, &target).await?;
    let purging = spawn_purge(&storage, &target);
    // The objects step drains the target's writes: it waits for the running write's pin.
    until_held(&storage, &target).await?;
    assert!(
        !purging.is_finished(),
        "the purge does not delete the target's objects while its write runs"
    );
    // The write retains its bytes, then its record is refused, and the write ends.
    storage
        .blobs
        .put_for(
            &target.tenant_id,
            Box::pin(std::io::Cursor::new(late.to_vec())),
            1 << 20,
            None,
        )
        .await?;
    running.end()?;
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
    holding.end()?;
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
    holding.end()?;
    purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the object the import's card names stays"
    );
    let head = storage.versions().head(&survivor).await?;
    storage.versions().show(&survivor, &head, item(13)).await?;
    Ok(())
}

/// Re-review probe Q1 (window 2 with a job), held deterministically: the survivor's job stores
/// an object identical to one of the target's before the purge, and completes (its output row
/// lands, its pending put is deleted) while the purge is held inside its objects step, after it
/// read every surviving reference. The object stays: the survivor's pending put, read by the
/// step, kept it. (Was a timing test in tests/purge.rs; flaky on a fast runner.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_output_completed_inside_the_objects_step_keeps_its_object() -> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let shared = b"commonplace identical bytes";
    let (target, survivor, _, digest) = target_with(&storage, "local", shared).await?;
    let job_scope = okf_jawn_core::jobs::JobScope::Workspace(survivor.clone());
    let records = storage.records();
    let job = records
        .create_job(
            &job_scope,
            okf_jawn_core::jobs::NewJob {
                mutation_id: mutation(30),
                initiator: person(),
                spec: okf_jawn_core::jobs::JobSpec::BackupWorkspace,
            },
        )
        .await?;
    let claimed = records
        .claim_job(job.id)
        .await?
        .ok_or_else(|| fault(&"the job was not claimed"))?;
    let stored = put(&storage, &survivor, shared).await?;
    assert_eq!(stored, digest);
    record_purge(&storage, &target).await?;
    let holding = hold_the_object_step(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    records
        .complete_job(okf_jawn_core::jobs::JobCompletion {
            lease: claimed.lease,
            revision: None,
            item_ids: Vec::new(),
            artifact: None,
            restore: None,
            outputs: vec![stored],
            warnings: Vec::new(),
        })
        .await?;
    holding.end()?;
    let report = purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the survivor's completed job output stays: {report:?}"
    );
    Ok(())
}

/// Re-review probe Q2 (window 2 with a commit), held deterministically: the survivor stores an
/// object identical to one of the target's before the purge and commits the card naming it
/// while the purge is held inside its objects step. The object stays. (Was a timing test in
/// tests/purge.rs.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_object_a_commit_names_inside_the_objects_step_stays() -> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let shared = b"commonplace identical bytes";
    let (target, survivor, survivor_head, digest) = target_with(&storage, "local", shared).await?;
    let stored = put(&storage, &survivor, shared).await?;
    record_purge(&storage, &target).await?;
    let holding = hold_the_object_step(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    commit(
        &storage,
        &survivor,
        4,
        &survivor_head,
        vec![card(item(12), "sources/b.md", &stored)?],
    )
    .await?;
    holding.end()?;
    let report = purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the object the survivor's new card names stays: {report:?}"
    );
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
    holding.end()?;
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

/// Wait until a caller of scope's write lock besides the test's own holder has taken the lock's
/// handle: it passed every check it makes before the lock, and it waits for the lock.
async fn until_waiting_for_the_lock(
    storage: &Storage,
    scope: &StorageScope,
) -> Result<(), ApiError> {
    let started = Instant::now();
    while storage.repositories.lock_users(scope)? < 2 {
        if started.elapsed() > Duration::from_secs(120) {
            return Err(fault(&"no write began to wait for the workspace's lock"));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
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
    // The commit passed its first check and waits for the lock.
    until_waiting_for_the_lock(&storage, &target).await?;
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
    // The purge waits for the lock to plan.
    until_waiting_for_the_lock(&storage, &target).await?;
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

/// Ruling 4 (M8): an import that fails after it put its objects back, before its history and
/// rows land (the workspace is not blank), leaves their pending puts, so a purge of the
/// workspace deletes the bytes, which nothing else names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_import_leaves_pending_puts_a_purge_deletes() -> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let imported = b"zanzibar imported bytes";
    let (source, source_head) = create(&storage, "local", 70, "Source").await?;
    let original = put(&storage, &source, imported).await?;
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
    let (target, head) = create(&storage, "elsewhere", 1, "Copy").await?;
    commit(
        &storage,
        &target,
        2,
        &head,
        vec![card(
            item(14),
            "sources/own.md",
            &digest_of(b"its own card")?,
        )?],
    )
    .await?;
    let uploaded = storage
        .blobs()
        .put(
            &target,
            Box::pin(std::io::Cursor::new(bytes)),
            1 << 30,
            None,
        )
        .await?;
    let failed = storage
        .backups()
        .restore_import(
            &target,
            uploaded.digest,
            mutation(73),
            vec!["ana".to_owned()],
        )
        .await;
    match failed {
        Ok(report) => return Err(fault(&format!("the import was meant to fail: {report:?}"))),
        Err(error) => assert_eq!(error.code, ErrorCode::Conflict, "only a blank one: {error}"),
    }
    assert!(
        held(&storage, &target, &original).await?,
        "the object was put back"
    );
    record_purge(&storage, &target).await?;
    let report = spawn_purge(&storage, &target)
        .await
        .map_err(|error| fault(&error))??;
    assert!(
        !held(&storage, &target, &original).await?,
        "the failed import's bytes go with the workspace: {report:?}"
    );
    Ok(())
}

/// The pending puts of `digest` in `scope`.
async fn pending_of(
    storage: &Storage,
    scope: &StorageScope,
    digest: &Digest,
) -> Result<i64, ApiError> {
    let (tenant, workspace) = crate::db::scope_key(scope);
    let digest = digest.as_str().to_owned();
    storage
        .records
        .call(move |connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM pending_puts
                     WHERE tenant_id = ?1 AND workspace_id = ?2 AND digest = ?3",
                    [tenant, workspace, digest],
                    |row| row.get(0),
                )
                .map_err(|error| fault(&error))
        })
        .await
}

/// Write a workspace archive of `source` and put its bytes into `target`, as an upload of it
/// would; returns the digest to import.
async fn archive_into(
    storage: &Storage,
    source: &StorageScope,
    target: &StorageScope,
) -> Result<Digest, ApiError> {
    let archive = storage
        .backups()
        .write_workspace_archive(source, mutation(72))
        .await?;
    let bytes = std::fs::read(
        storage
            .blobs
            .local_path(&source.tenant_id, &archive.digest)?,
    )
    .map_err(|error| fault(&error))?;
    let uploaded = storage
        .blobs()
        .put(target, Box::pin(std::io::Cursor::new(bytes)), 1 << 30, None)
        .await?;
    Ok(uploaded.digest)
}

/// Ruling B3 (imports): an import lands only the pending puts of the archive's objects that the
/// restored workspace names. An object the archive holds for a row an import does not copy (a
/// completed upload of the source) keeps its pending put, so a purge of the workspace deletes
/// it, while the object its restored card names lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_keeps_the_pending_put_of_an_object_no_imported_record_names()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (source, source_head) = create(&storage, "local", 70, "Source").await?;
    let carded = put(&storage, &source, b"zanzibar carded bytes").await?;
    commit(
        &storage,
        &source,
        71,
        &source_head,
        vec![card(item(13), "sources/c.md", &carded)?],
    )
    .await?;
    let only_uploaded = b"zanzibar bytes only an upload names";
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &source,
            mutation(74),
            NewUpload {
                filename: "loose.bin".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(only_uploaded.len()).map_err(|error| fault(&error))?,
                expected_sha256: None,
                supplied_by: person(),
            },
        )
        .await?;
    uploads
        .put_content(
            &source,
            slot.id,
            Box::pin(std::io::Cursor::new(only_uploaded.to_vec())),
            1 << 20,
        )
        .await?;
    let loose = digest_of(only_uploaded)?;
    uploads.complete(&source, slot.id, loose.clone()).await?;
    let (target, _) = create(&storage, "elsewhere", 1, "Copy").await?;
    let archive = archive_into(&storage, &source, &target).await?;
    storage
        .backups()
        .restore_import(&target, archive, mutation(73), vec!["ana".to_owned()])
        .await?;
    assert!(
        held(&storage, &target, &loose).await?,
        "the archive held it"
    );
    assert_eq!(
        pending_of(&storage, &target, &carded).await?,
        0,
        "the restored card names it"
    );
    assert_eq!(
        pending_of(&storage, &target, &loose).await?,
        1,
        "no imported record names it"
    );
    record_purge(&storage, &target).await?;
    let report = spawn_purge(&storage, &target)
        .await
        .map_err(|error| fault(&error))??;
    assert!(
        !held(&storage, &target, &loose).await?,
        "the purge deletes it: {report:?}"
    );
    assert!(!held(&storage, &target, &carded).await?, "{report:?}");
    Ok(())
}

/// Round 4 re-review N1 (probe
/// `rr4_an_import_keeps_the_pending_put_of_an_asset_only_an_uncopied_record_names`), as B1-R
/// round 2 rules it: the source's completed upload holds JSON naming an asset, and no card
/// names the upload as its conversion record, so nothing ever opens it for references. The
/// archive copies the upload (a completed upload's object is required) but not the asset;
/// before round 2 it copied the asset too, followed from the upload. The import, which copies
/// no upload row, leaves the upload a pending put, and the workspace's purge deletes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_keeps_the_pending_put_of_an_asset_only_an_uncopied_record_names()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (source, _) = create(&storage, "local", 70, "Source").await?;
    let asset = put(
        &storage,
        &source,
        b"zanzibar asset only a loose record names",
    )
    .await?;
    let record_bytes = format!("{{\"assets\":[\"{}\"]}}", asset.as_str()).into_bytes();
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            &source,
            mutation(74),
            NewUpload {
                filename: "record.json".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(record_bytes.len()).map_err(|error| fault(&error))?,
                expected_sha256: None,
                supplied_by: person(),
            },
        )
        .await?;
    uploads
        .put_content(
            &source,
            slot.id,
            Box::pin(std::io::Cursor::new(record_bytes.clone())),
            1 << 20,
        )
        .await?;
    let record = digest_of(&record_bytes)?;
    uploads.complete(&source, slot.id, record.clone()).await?;
    let (target, _) = create(&storage, "elsewhere", 1, "Copy").await?;
    let archive = archive_into(&storage, &source, &target).await?;
    storage
        .backups()
        .restore_import(&target, archive, mutation(73), vec!["ana".to_owned()])
        .await?;
    assert!(
        !held(&storage, &target, &asset).await?,
        "the archive copied an asset only an upload names"
    );
    assert_eq!(
        pending_of(&storage, &target, &record).await?,
        1,
        "no imported record names the upload"
    );
    assert_eq!(
        pending_of(&storage, &target, &asset).await?,
        0,
        "the asset was never put back"
    );
    record_purge(&storage, &target).await?;
    let report = spawn_purge(&storage, &target)
        .await
        .map_err(|error| fault(&error))??;
    assert!(!held(&storage, &target, &asset).await?, "{report:?}");
    assert!(!held(&storage, &target, &record).await?, "{report:?}");
    Ok(())
}

/// A source card for `object` whose extraction names the conversion record `record`.
fn card_with_record(
    item: ItemId,
    at: &str,
    object: &Digest,
    record: &Digest,
) -> Result<TreeEdit, ApiError> {
    let mut edit = card(item, at, object)?;
    if let TreeEdit::WriteSourceCard(source) = &mut edit {
        let mut appearance =
            serde_json::to_value(&source.appearance).map_err(|error| fault(&error))?;
        if let Some(extraction) = appearance
            .get_mut("extraction")
            .and_then(serde_json::Value::as_object_mut)
        {
            extraction.insert("digest".to_owned(), json!(record.as_str()));
        }
        source.appearance = serde_json::from_value(appearance).map_err(|error| fault(&error))?;
    }
    Ok(edit)
}

/// Ruling 4 (mutant X05): an import lands the pending puts of the archive's objects that the
/// restored workspace names: through its history (a card's original and the conversion record
/// its extraction names), through its rows (a derived dataset), and through that conversion
/// record (its asset).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_lands_the_pending_puts_of_the_objects_the_restored_workspace_names()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (source, source_head) = create(&storage, "local", 70, "Source").await?;
    let original = put(&storage, &source, b"zanzibar imported original").await?;
    let asset = put(&storage, &source, b"zanzibar imported asset").await?;
    // A conversion record names each asset as an object with its `digest`
    // (`ConversionRecord::assets`), and only that field is read.
    let record = put(
        &storage,
        &source,
        format!("{{\"assets\":[{{\"digest\":\"{}\"}}]}}", asset.as_str()).as_bytes(),
    )
    .await?;
    let revision = commit(
        &storage,
        &source,
        71,
        &source_head,
        vec![card_with_record(
            item(13),
            "sources/c.md",
            &original,
            &record,
        )?],
    )
    .await?;
    let dataset = put(&storage, &source, b"zanzibar imported dataset").await?;
    storage
        .records()
        .record_derived_object(
            &source,
            okf_jawn_core::jobs::DerivedObject {
                item_id: item(13),
                revision,
                digest: dataset.clone(),
                kind: okf_jawn_core::jobs::DerivedKind::Dataset,
                media_type: "text/csv".to_owned(),
            },
        )
        .await?;
    let (target, _) = create(&storage, "elsewhere", 1, "Copy").await?;
    let archive = archive_into(&storage, &source, &target).await?;
    storage
        .backups()
        .restore_import(&target, archive, mutation(73), vec!["ana".to_owned()])
        .await?;
    for (digest, what) in [
        (&original, "the card's original"),
        (&record, "the conversion record"),
        (&asset, "the record's asset"),
        (&dataset, "the derived dataset"),
    ] {
        assert!(
            held(&storage, &target, digest).await?,
            "{what} was put back"
        );
        assert_eq!(pending_of(&storage, &target, digest).await?, 0, "{what}");
    }
    Ok(())
}

/// A candidate of `target` on `head` under proposal 20, mutation `n`.
async fn candidate(
    storage: &Storage,
    target: &StorageScope,
    n: u128,
    head: &Revision,
) -> Result<Revision, ApiError> {
    storage
        .versions()
        .create_candidate(
            target,
            okf_jawn_contract::identity::ProposalId(uuid::Uuid::from_u128(20)),
            okf_jawn_core::storage::CandidateChanges {
                mutation_id: mutation(n),
                base: head.clone(),
                author: person(),
                message: "Propose".to_owned(),
                edits: vec![card(item(13), "sources/c.md", &digest_of(b"zanzibar")?)?],
            },
            Arc::new(Accept),
        )
        .await
}

/// Assert that a Git write waiting for the workspace's lock while the purge is recorded is
/// refused under the lock: `write` starts it while the test holds the lock.
async fn refused_under_the_lock<T: std::fmt::Debug + Send + 'static>(
    storage: &Storage,
    target: &StorageScope,
    write: JoinHandle<Result<T, ApiError>>,
    holder: Holder,
) -> Result<(), ApiError> {
    let (release, holding) = holder;
    // The write passed its first check and waits for the lock.
    until_waiting_for_the_lock(storage, target).await?;
    record_purge(storage, target).await?;
    release.send(()).map_err(|error| fault(&error))?;
    holding
        .join()
        .map_err(|_| fault(&"the lock holder stopped"))??;
    let error = match write.await.map_err(|error| fault(&error))? {
        Ok(written) => return Err(fault(&format!("the write landed: {written:?}"))),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(error.message.contains("purged by purge"), "{error}");
    Ok(())
}

/// Ruling 4 (M6a): a candidate waiting for the workspace's lock when the purge is recorded is
/// refused under the lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_candidate_waiting_for_the_workspace_lock_when_the_purge_is_recorded_is_refused()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, "local", 1, "Trip").await?;
    let holder = hold_the_workspace(&storage, &target)?;
    let writing = {
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move { candidate(&storage, &target, 4, &head).await })
    };
    refused_under_the_lock(&storage, &target, writing, holder).await
}

/// Ruling 4 (M6b): a promotion waiting for the workspace's lock when the purge is recorded is
/// refused under the lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_promotion_waiting_for_the_workspace_lock_when_the_purge_is_recorded_is_refused()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, "local", 1, "Trip").await?;
    let proposed = candidate(&storage, &target, 4, &head).await?;
    let holder = hold_the_workspace(&storage, &target)?;
    let writing = {
        let versions = storage.versions();
        let target = target.clone();
        tokio::spawn(async move {
            versions
                .promote_candidate(
                    &target,
                    okf_jawn_core::storage::Promotion {
                        mutation_id: mutation(5),
                        proposal_id: okf_jawn_contract::identity::ProposalId(
                            uuid::Uuid::from_u128(20),
                        ),
                        expected_head: head,
                        candidate: proposed,
                        approver: person(),
                        message: "Accept".to_owned(),
                    },
                )
                .await
        })
    };
    refused_under_the_lock(&storage, &target, writing, holder).await
}

/// Ruling 4 (M5): an indexing reads and stores under the workspace's write lock, so it waits
/// while another write holds the lock (the purge's repository step deletes the repository
/// under it, then removes the index entries again).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_indexing_waits_for_the_workspace_lock() -> Result<(), ApiError> {
    use okf_jawn_core::search::SearchIndex;
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, head) = create(&storage, "local", 1, "Trip").await?;
    let (release, holding) = hold_the_workspace(&storage, &target)?;
    let indexing = {
        let search = storage.search();
        let target = target.clone();
        tokio::spawn(async move { search.index_revision(&target, head).await })
    };
    until_waiting_for_the_lock(&storage, &target).await?;
    assert!(
        !indexing.is_finished(),
        "no indexing runs while another write holds the workspace"
    );
    release.send(()).map_err(|error| fault(&error))?;
    holding
        .join()
        .map_err(|_| fault(&"the lock holder stopped"))??;
    indexing.await.map_err(|error| fault(&error))??;
    Ok(())
}

/// Ruling 4 (M2): a purge deletes an object while it holds the pin table's lock, so a put
/// cannot pin between the touched check and the delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_put_cannot_pin_while_the_objects_step_deletes() -> Result<(), ApiError> {
    let pins = crate::pins::Pins::default();
    let digest = digest_of(b"commonplace identical bytes")?;
    let (sweep, _) = pins.sweep("local", "target")?;
    let pinning = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let deleted = sweep.delete_unless_touched(&digest, || {
        let (pins, digest, flag) = (pins.clone(), digest.clone(), Arc::clone(&pinning));
        let putting = std::thread::spawn(move || {
            let pinned = pins.pin("local", "survivor", &digest);
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            pinned.map(|_| ())
        });
        std::thread::sleep(Duration::from_millis(300));
        let raced = pinning.load(std::sync::atomic::Ordering::SeqCst);
        // Detach the thread: it pins once the lock is released.
        drop(putting);
        if raced {
            return Err(fault(&"a put pinned while the delete ran"));
        }
        Ok(true)
    })?;
    assert!(deleted, "the untouched object was deleted");
    Ok(())
}

/// Re-review probe U1 (M1): a survivor's write still running when the sweep begins, which
/// ends and lands its card only after the reference read, keeps its object, and the report
/// counts it as kept (fix round 5, P4: the kept count covers pins).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_survivor_write_running_when_the_sweep_begins_keeps_its_object() -> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let shared = b"commonplace identical bytes";
    let (target, survivor, survivor_head, digest) = target_with(&storage, "local", shared).await?;
    let (tenant_key, survivor_key) = crate::db::scope_key(&survivor);
    let running = storage
        .blobs
        .pins()
        .pin(&tenant_key, &survivor_key, &digest)?;
    record_purge(&storage, &target).await?;
    let holding = hold_the_object_step(&storage, &target)?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    running.end()?;
    commit(
        &storage,
        &survivor,
        4,
        &survivor_head,
        vec![card(item(12), "sources/b.md", &digest)?],
    )
    .await?;
    holding.end()?;
    let report = purging.await.map_err(|error| fault(&error))??;
    assert!(
        held(&storage, &survivor, &digest).await?,
        "the survivor's object stays: {report:?}"
    );
    // No durable reference the step read names it: only the pin kept it, and it is counted.
    assert_eq!(report.objects_kept_shared, "1", "{report:?}");
    Ok(())
}

/// Ruling 5 (the drain bound; re-review probe U2, with the limit shortened to 3 s): a write of
/// the target that never ends makes the purge stop with `Unavailable` and delete nothing; once
/// it ends, repeating the purge completes it and deletes its bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drain_limit_stops_the_purge_with_unavailable_and_a_repeat_resumes_it()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, "local", 1, "Trip").await?;
    let late = b"zanzibar bytes of a stuck write";
    let digest = digest_of(late)?;
    let (tenant_key, workspace_key) = crate::db::scope_key(&target);
    let stuck = storage
        .blobs
        .pins()
        .pin(&tenant_key, &workspace_key, &digest)?;
    {
        let digest = digest.clone();
        storage
            .records
            .transaction(move |transaction| {
                crate::puts::record(transaction, &tenant_key, &workspace_key, &digest)
            })
            .await?;
    }
    storage
        .blobs
        .put_for(
            &target.tenant_id,
            Box::pin(std::io::Cursor::new(late.to_vec())),
            1 << 20,
            None,
        )
        .await?;
    storage.blobs.pins().shorten_drain(Duration::from_secs(3))?;
    record_purge(&storage, &target).await?;
    match spawn_purge(&storage, &target)
        .await
        .map_err(|error| fault(&error))?
    {
        Ok(report) => return Err(fault(&format!("the purge completed: {report:?}"))),
        Err(error) => assert_eq!(error.code, ErrorCode::Unavailable, "{error}"),
    }
    assert!(
        held(&storage, &target, &digest).await?,
        "nothing deleted yet"
    );
    stuck.end()?;
    let report = spawn_purge(&storage, &target)
        .await
        .map_err(|error| fault(&error))??;
    assert!(
        !held(&storage, &target, &digest).await?,
        "the stuck write's bytes go: {report:?}"
    );
    Ok(())
}

/// An upload slot of `target`, under mutation 8, that has received `bytes` in full.
async fn received_upload(
    storage: &Storage,
    target: &StorageScope,
    bytes: &[u8],
) -> Result<okf_jawn_contract::identity::UploadId, ApiError> {
    let uploads = storage.uploads();
    let slot = uploads
        .create(
            target,
            mutation(8),
            NewUpload {
                filename: "big.bin".to_owned(),
                relative_path: String::new(),
                expected_size: u64::try_from(bytes.len()).map_err(|error| fault(&error))?,
                expected_sha256: None,
                supplied_by: person(),
            },
        )
        .await?;
    uploads
        .put_content(
            target,
            slot.id,
            Box::pin(std::io::Cursor::new(bytes.to_vec())),
            1 << 28,
        )
        .await?;
    Ok(slot.id)
}

/// Complete upload `slot` of `target` with `digest` in the background.
fn spawn_completion(
    storage: &Storage,
    target: &StorageScope,
    slot: okf_jawn_contract::identity::UploadId,
    digest: &Digest,
) -> JoinHandle<Result<okf_jawn_core::uploads::UploadRecord, ApiError>> {
    let (uploads, target, digest) = (storage.uploads(), target.clone(), digest.clone());
    tokio::spawn(async move { uploads.complete(&target, slot, digest).await })
}

/// The files under `directory` that hold the marker `zanzibar` (the lock file excepted: this
/// process holds it, and it is empty).
fn marked_files(directory: &std::path::Path) -> Result<Vec<String>, ApiError> {
    let marker = b"zanzibar";
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|error| fault(&error))? {
        let path = entry.map_err(|error| fault(&error))?.path();
        if path.file_name().is_some_and(|name| name == "okf-jawn.lock") {
            continue;
        }
        if path.is_dir() {
            found.extend(marked_files(&path)?);
        } else if std::fs::read(&path)
            .map_err(|error| fault(&error))?
            .windows(marker.len())
            .any(|window| window == marker)
        {
            found.push(path.display().to_string());
        }
    }
    Ok(found)
}

/// Assert that `result` is the purge's refusal: `Conflict` naming the purge.
fn assert_refused<T: std::fmt::Debug>(result: Result<T, ApiError>) -> Result<(), ApiError> {
    let error = match result {
        Ok(value) => return Err(fault(&format!("not refused: {value:?}"))),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::Conflict, "{error}");
    assert!(
        error.message.contains(&purge_id().0.to_string()),
        "the refusal names the purge: {error}"
    );
    Ok(())
}

/// Re-review probe Q3 (window 1, an upload completion in flight), held deterministically: the
/// completion passed its first refusal and staged and hashed the part file before the purge was
/// recorded, and is held there, before its pin, while the whole purge runs (an upload
/// completion is no job, so no drain covers it). Released, it is refused by the purge check
/// that follows its pin, and no byte of the target remains. (Was a timing test in
/// tests/purge.rs, `purge_leaves_nothing_of_an_upload_completion_in_flight`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_completion_staged_before_the_purge_is_refused_and_leaves_nothing()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, "local", 1, "Trip").await?;
    create(&storage, "local", 2, "Garden").await?;
    let bytes = UPLOADED.repeat(1 << 16);
    let slot = received_upload(&storage, &target, &bytes).await?;
    let digest = digest_of(&bytes)?;
    let (arrival, release) = crate::pause::hold(storage.blobs.pause_point(&digest, "received"))?;
    let finishing = spawn_completion(&storage, &target, slot, &digest);
    crate::pause::arrived(arrival).await?;
    record_purge(&storage, &target).await?;
    let report = spawn_purge(&storage, &target)
        .await
        .map_err(|error| fault(&error))??;
    release.send(()).map_err(|error| fault(&error))?;
    assert_refused(finishing.await.map_err(|error| fault(&error))?)?;
    assert!(
        !held(&storage, &target, &digest).await?,
        "the completion stored nothing: {report:?}"
    );
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "no byte of the target's upload remains"
    );
    Ok(())
}

/// Re-review probe Q3-hard, held deterministically: the completion is past its second refusal
/// (pinned, its pending put recorded) when the purge is recorded, and is held before it
/// retains. The purge's objects step waits for its pin. Released, the completion retains its
/// bytes, its record is refused in its transaction and its pin ends; the purge then deletes the
/// bytes its pending put names. Nothing of the target stays. (Was a timing test in
/// tests/purge.rs, `purge_leaves_nothing_of_an_upload_completion_past_its_second_check`, which
/// failed under load.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_completion_pinned_before_the_purge_is_waited_for_and_its_bytes_removed()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (target, _) = create(&storage, "local", 1, "Trip").await?;
    create(&storage, "local", 2, "Garden").await?;
    let bytes = UPLOADED.repeat(1 << 16);
    let slot = received_upload(&storage, &target, &bytes).await?;
    let digest = digest_of(&bytes)?;
    let (arrival, release) = crate::pause::hold(storage.blobs.pause_point(&digest, "pinned"))?;
    let finishing = spawn_completion(&storage, &target, slot, &digest);
    crate::pause::arrived(arrival).await?;
    record_purge(&storage, &target).await?;
    let purging = spawn_purge(&storage, &target);
    until_held(&storage, &target).await?;
    assert!(
        !purging.is_finished(),
        "the objects step waits for the completion's pin"
    );
    release.send(()).map_err(|error| fault(&error))?;
    assert_refused(finishing.await.map_err(|error| fault(&error))?)?;
    let report = purging.await.map_err(|error| fault(&error))??;
    assert!(
        !held(&storage, &target, &digest).await?,
        "the retained bytes go with the target: {report:?}"
    );
    assert_eq!(
        marked_files(directory.path())?,
        Vec::<String>::new(),
        "no byte of the target's upload remains"
    );
    Ok(())
}
