//! Unit tests of the catalog's metadata change against a commit, driven in both orders by the
//! test-only pause points (`crate::pause`) instead of by timing. A metadata change takes the
//! lock every commit takes, so neither lands inside the other's head comparison. They return
//! `ApiError` like the other storage unit tests.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use okf_jawn_contract::access::AccessRoute;
use okf_jawn_contract::common::Warning;
use okf_jawn_contract::error::{ApiError, ErrorCode, ErrorDetail};
use okf_jawn_contract::identity::{ItemId, MutationId, Revision, TenantId, WorkspacePath};
use okf_jawn_contract::item::ItemKind;
use okf_jawn_contract::workspace::Workspace;
use okf_jawn_core::storage::{
    CandidateCheck, CommitChanges, Committed, NewWorkspace, Provenance, StorageScope, TreeEdit,
    VersionStore, WorkspaceCatalog, WorkspaceUpdate,
};
use tokio::task::JoinHandle;

use super::change_point;
use crate::git::commit_locked_point;
use crate::storage::Storage;

/// Accepts every candidate.
struct Accept;

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
