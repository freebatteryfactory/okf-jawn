//! Unit tests of the catalog's metadata change against a commit and of an archive against a
//! content job's insert, each pair driven in both orders by the test-only pause points
//! (`crate::pause`) and the workspace's write lock instead of by timing. A metadata change takes
//! the lock every commit takes, so neither lands inside the other's head comparison; an archive
//! and an import's insert are each one records transaction, so whichever commits first decides
//! the other's outcome. They return `ApiError` like the other storage unit tests.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::extraction::ConversionSettings;
use okf_jawn_contract::identity::{
    ItemId, MutationId, Revision, TenantId, UploadId, WorkspacePath,
};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::jobs::{JobScope, JobSpec, NewJob, RecordStore};
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, Committed, NewWorkspace, Provenance, StorageScope, TreeEdit,
    VersionStore, WorkspaceArchive, WorkspaceCatalog, WorkspaceUpdate,
};
use tokio::task::JoinHandle;

use super::change_point;
use crate::git::commit_locked_point;
use crate::storage::Storage;

/// Accepts every candidate.
struct Accept;

/// A thread holding a workspace's write lock, and the sender that makes it let go.
type Holder = (
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<Result<(), ApiError>>,
);

/// The longest a test waits for a write to reach the workspace's lock.
const LOCK_WAIT: Duration = Duration::from_secs(30);

impl CandidateCheck for Accept {
    fn check(
        &self,
        _root: &std::path::Path,
        _rewritten: &[WorkspacePath],
    ) -> Result<Vec<Warning>, ApiError> {
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

fn mutation(n: u128) -> MutationId {
    MutationId(uuid::Uuid::from_u128(n))
}

/// A workspace of the tenant `tenant`, each test's own, and its head.
async fn create(storage: &Storage, tenant: &str) -> Result<(StorageScope, Revision), ApiError> {
    let tenant = TenantId::try_from(tenant.to_owned()).map_err(|error| fault(&error))?;
    let created = storage
        .catalog()
        .create(
            &tenant,
            mutation(1),
            NewWorkspace {
                name: "Notes".to_owned(),
                description: String::new(),
                creator: person(),
            },
        )
        .await?;
    Ok((
        StorageScope {
            tenant_id: tenant,
            workspace_id: created.id,
        },
        created.head,
    ))
}

/// A commit of one new note under mutation `n`, made against `head`.
fn spawn_commit(
    storage: &Storage,
    scope: &StorageScope,
    head: &Revision,
    n: u128,
) -> Result<JoinHandle<Result<Committed, ApiError>>, ApiError> {
    let edit = TreeEdit::CreateItem {
        item_id: ItemId(uuid::Uuid::from_u128(n)),
        path: WorkspacePath::try_from(format!("notes/{n}.md")).map_err(|error| fault(&error))?,
        title: None,
        type_name: "Note".to_owned(),
        kind: ItemKind::Note,
        body: "text".to_owned(),
        properties: BTreeMap::new(),
    };
    let versions = storage.versions();
    let scope = scope.clone();
    let changes = CommitChanges {
        mutation_id: mutation(n),
        expected_head: head.clone(),
        author: person(),
        message: "A note".to_owned(),
        edits: vec![edit],
    };
    Ok(tokio::spawn(async move {
        versions.commit(&scope, changes, Arc::new(Accept)).await
    }))
}

/// A rename of the workspace under mutation `n`, made against `head`.
fn spawn_update(
    storage: &Storage,
    scope: &StorageScope,
    head: &Revision,
    n: u128,
) -> JoinHandle<Result<Workspace, ApiError>> {
    let catalog = storage.catalog();
    let scope = scope.clone();
    let update = WorkspaceUpdate {
        expected_head: head.clone(),
        name: "Renamed".to_owned(),
        description: String::new(),
        author: person(),
    };
    tokio::spawn(async move { catalog.update(&scope, mutation(n), update).await })
}

/// An archive of the workspace under mutation `n`, made against `head`.
fn spawn_archive(
    storage: &Storage,
    scope: &StorageScope,
    head: &Revision,
    n: u128,
) -> JoinHandle<Result<Workspace, ApiError>> {
    let catalog = storage.catalog();
    let scope = scope.clone();
    let archive = WorkspaceArchive {
        expected_head: head.clone(),
        author: person(),
    };
    tokio::spawn(async move { catalog.archive(&scope, mutation(n), archive).await })
}

/// An import job of the workspace under mutation `n`, made against `head`.
fn import(head: &Revision, n: u128) -> NewJob {
    NewJob {
        mutation_id: mutation(n),
        initiator: person(),
        spec: JobSpec::Import {
            base_revision: head.clone(),
            upload_ids: vec![UploadId(uuid::Uuid::from_u128(99))],
            destination: None,
            apply_naming_rules: false,
            settings: ConversionSettings::default(),
        },
    }
}

/// Wait until `users` callers hold or wait for `scope`'s write lock.
async fn until_lock_users(
    storage: &Storage,
    scope: &StorageScope,
    users: usize,
) -> Result<(), ApiError> {
    let started = Instant::now();
    while storage.repositories.lock_users(scope)? < users {
        if started.elapsed() > LOCK_WAIT {
            return Err(fault(&format!(
                "no {users} callers of the workspace's lock: a write did not wait for it"
            )));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
}

/// Hold `scope`'s workspace write lock on a thread until the returned sender is sent to.
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

/// Commit first: a rename made against the head a commit is about to move waits for the
/// workspace's lock while the commit holds it, then finds the moved head and is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_metadata_change_waits_for_a_commit_holding_the_lock_and_sees_its_head()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (scope, head) = create(&storage, "commit-first").await?;
    let (arrival, release) = crate::pause::hold(commit_locked_point(
        &storage.repositories,
        &scope,
        mutation(10),
    ))?;
    let committing = spawn_commit(&storage, &scope, &head, 10)?;
    crate::pause::arrived(arrival).await?;
    // The commit holds the lock; the rename begins and waits for it.
    let renaming = spawn_update(&storage, &scope, &head, 11);
    until_lock_users(&storage, &scope, 2).await?;
    release.send(()).map_err(|error| fault(&error))?;
    let committed = committing.await.map_err(|error| fault(&error))??;
    assert_ne!(committed.revision, head);
    let refused = match renaming.await.map_err(|error| fault(&error))? {
        Ok(renamed) => return Err(fault(&format!("renamed at a stale base: {renamed:?}"))),
        Err(refused) => refused,
    };
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        matches!(
            refused.detail.as_deref(),
            Some(ErrorDetail::StaleBase { head }) if *head == committed.revision
        ),
        "{refused:?}"
    );
    assert_eq!(storage.catalog().open(&scope).await?.name, "Notes");
    Ok(())
}

/// Rename first: a commit begun while a rename holds the workspace's lock and its records
/// transaction, between its head comparison and its write, lands only after the rename, on the
/// head the rename answered. (The commit waits at its first records read, before the lock;
/// the commit-first test is the one that fails without the rename's lock.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_begun_while_a_metadata_change_holds_the_lock_lands_after_it()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (scope, head) = create(&storage, "rename-first").await?;
    let (arrival, release) =
        crate::pause::hold(change_point(&storage.repositories, &scope, mutation(11)))?;
    let renaming = spawn_update(&storage, &scope, &head, 11);
    crate::pause::arrived(arrival).await?;
    // The rename compared the head and holds the lock and the transaction; the commit begins.
    let mut committing = spawn_commit(&storage, &scope, &head, 10)?;
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut committing).await;
    assert!(waited.is_err(), "the commit landed inside the rename");
    release.send(()).map_err(|error| fault(&error))?;
    let renamed = renaming.await.map_err(|error| fault(&error))??;
    assert_eq!(renamed.head, head, "the head the rename was made at");
    assert_eq!(renamed.name, "Renamed");
    let committed = committing.await.map_err(|error| fault(&error))??;
    assert_ne!(committed.revision, head);
    let now = storage.catalog().open(&scope).await?;
    assert_eq!(now.head, committed.revision);
    assert_eq!(now.name, "Renamed");
    Ok(())
}

/// Archive first: an import's insert begun while the archive holds its transaction (its checks
/// made, its write not yet) waits for it, and is then refused for the archived workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_import_inserted_while_an_archive_holds_its_transaction_is_refused()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (scope, head) = create(&storage, "archive-first").await?;
    let (arrival, release) =
        crate::pause::hold(change_point(&storage.repositories, &scope, mutation(20)))?;
    let archiving = spawn_archive(&storage, &scope, &head, 20);
    crate::pause::arrived(arrival).await?;
    let mut inserting = {
        let records = storage.records();
        let job_scope = JobScope::Workspace(scope.clone());
        let job = import(&head, 21);
        tokio::spawn(async move { records.create_job(&job_scope, job).await })
    };
    // The insert cannot begin its transaction while the archive holds the records database.
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut inserting).await;
    assert!(
        waited.is_err(),
        "the insert ran inside the archive's transaction"
    );
    release.send(()).map_err(|error| fault(&error))?;
    let archived = archiving.await.map_err(|error| fault(&error))??;
    let at = archived
        .archived_at
        .ok_or_else(|| fault(&"the archive set no instant"))?;
    let refused = match inserting.await.map_err(|error| fault(&error))? {
        Ok(job) => return Err(fault(&format!("an archived workspace took {job:?}"))),
        Err(refused) => refused,
    };
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        matches!(
            refused.detail.as_deref(),
            Some(ErrorDetail::WorkspaceArchived { archived_at }) if *archived_at == at
        ),
        "{refused:?}"
    );
    Ok(())
}

/// Insert first: an archive begun before an import's insert, and still waiting for the
/// workspace's lock when the insert commits, finds the queued import and is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_archive_that_comes_after_an_import_inserted_meanwhile_is_refused()
-> Result<(), ApiError> {
    let directory = tempfile::tempdir().map_err(|error| fault(&error))?;
    let storage = Storage::open(directory.path())?;
    let (scope, head) = create(&storage, "insert-first").await?;
    let (release, holder) = hold_the_workspace(&storage, &scope)?;
    let archiving = spawn_archive(&storage, &scope, &head, 30);
    until_lock_users(&storage, &scope, 2).await?;
    // The archive waits for the lock; the import's insert takes no lock and commits now.
    let queued = storage
        .records()
        .create_job(&JobScope::Workspace(scope.clone()), import(&head, 31))
        .await?;
    release.send(()).map_err(|error| fault(&error))?;
    holder
        .join()
        .map_err(|_| fault(&"the lock holder stopped"))??;
    let refused = match archiving.await.map_err(|error| fault(&error))? {
        Ok(archived) => {
            return Err(fault(&format!(
                "archived over a queued import: {archived:?}"
            )));
        }
        Err(refused) => refused,
    };
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&queued.id.0.to_string()),
        "names the job: {}",
        refused.message
    );
    assert_eq!(storage.catalog().open(&scope).await?.archived_at, None);
    Ok(())
}
